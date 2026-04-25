//! MCP 多工程：`规范化路径键 -> ManagerBackend` 惰性注册表。
//!
//! 每个工程的存储路径由 [`mcp_storage_options_for_project_root`] 计算：
//! - 基础路径来自 [`platform_project_default_paths`](crate::resources::project_paths::platform_project_default_paths)；
//! - 若设置了 `SEMANTIC_SEARCH_PROJECT`，且当前工程的规范化键与其一致，则可用
//!   `SEMANTIC_SEARCH_INDEX_DB` / `SEMANTIC_SEARCH_VECTOR_DB` 覆盖对应路径；
//! - 其它工程仅使用平台数据目录下按工程隔离的默认布局，避免全局 db 环境变量误伤多工程场景。

use std::{collections::HashMap, path::Path, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    embedding::utils::OnnxRuntimeConfig,
    resources::project_paths::{
        normalize_project_root, platform_project_default_paths, project_path_key,
    },
    storage::manager::StorageOptions,
    tools::service::{ManagerBackend, OutputFormat, ResolvedConfig},
};

/// 与工程根路径无关、可被多个 `SemanticSearchManager` 共享的 MCP 启动配置。
/// 仅保留 ONNX / embedding 级别的共享参数，不再缓存「默认工程键」或「默认工程存储路径」。
pub struct McpSharedConfig {
    pub onnx_runtime_path: String,
    pub onnx_intra_threads: usize,
    pub embedding_options: crate::embedding::utils::EmbeddingOptions,
}

/// 计算该工程的存储路径，并确保相关目录存在。
pub fn mcp_storage_options_for_project_root(project_root: &Path) -> anyhow::Result<StorageOptions> {
    let canon = normalize_project_root(project_root)?;
    let base = platform_project_default_paths(&canon)?;

    let index_db_path = base.index_db_path;
    let vector_db_path = base.vector_db_path;

    if let Some(parent) = index_db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(&vector_db_path)?;

    Ok(StorageOptions {
        index_db_path,
        vector_db_path,
    })
}

/// 惰性创建并缓存每个工程对应的 [`ManagerBackend`]。
pub struct ProjectRegistry {
    map: Mutex<HashMap<String, Arc<ManagerBackend>>>,
}

impl ProjectRegistry {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }

    /// 按 [`crate::resources::project_paths::project_path_key`] 查找或创建 backend。
    /// `project_root` 可为任意解析到同一规范化键的路径表示。
    pub async fn get_or_create(
        &self,
        project_root: &Path,
        shared: &McpSharedConfig,
    ) -> anyhow::Result<Arc<ManagerBackend>> {
        let key = project_path_key(project_root)?;
        let mut guard = self.map.lock().await;
        if let Some(b) = guard.get(&key) {
            return Ok(Arc::clone(b));
        }

        let canon = normalize_project_root(project_root)?;
        let storage = mcp_storage_options_for_project_root(&canon)?;

        let resolved = ResolvedConfig {
            onnx_runtime_config: OnnxRuntimeConfig {
                runtime_path: shared.onnx_runtime_path.clone(),
                intra_threads: shared.onnx_intra_threads,
                optimization_level: ort::session::builder::GraphOptimizationLevel::Level1,
            },
            embedding_options: shared.embedding_options.clone(),
            output: OutputFormat::Json,
            // MCP registry 下所有工程共用同一日志文件；具体路径由 data_dir::platform_log_path 决定。
            log_path: crate::resources::data_dir::platform_log_path()?,
        };

        let backend = ManagerBackend::new(&canon, storage.clone(), &resolved).await?;
        let arc = Arc::new(backend);
        guard.insert(key, Arc::clone(&arc));
        Ok(arc)
    }
}

/// 供 `SemanticSearchMcp` 持有；`McpCommandExecutor` 在 `mcp.rs` 中为其实现。
pub struct RegistryCommandHandler {
    pub registry: Arc<ProjectRegistry>,
    pub shared: Arc<McpSharedConfig>,
}

impl RegistryCommandHandler {
    pub fn new(registry: Arc<ProjectRegistry>, shared: Arc<McpSharedConfig>) -> Self {
        Self { registry, shared }
    }
}
