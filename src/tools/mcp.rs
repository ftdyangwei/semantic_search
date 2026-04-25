use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::anyhow;
use async_trait::async_trait;
use clap::Parser;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, ErrorCode, ErrorData as McpError, Implementation, ProtocolVersion,
        ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};

use crate::{
    common::logger::init_logger,
    embedding::utils::OnnxRuntimeConfig,
    resources::{
        data_dir::platform_log_path,
        project_paths::{normalize_project_root, project_path_key},
    },
    tools::{
        mcp_registry::{
            McpSharedConfig, ProjectRegistry, RegistryCommandHandler,
            mcp_storage_options_for_project_root,
        },
        service::{
            CommandResponse, IndexProgressRequest, LayerSelector, ModelTypeArg, OutputFormat,
            ResolvedConfig, SearchRequest, StartIndexRequest, StopIndexRequest,
            dispatch_execute_index_progress, dispatch_execute_search, dispatch_execute_start_index,
            dispatch_execute_stop_index, validate_project_exists,
        },
    },
};

use ort::session::builder::GraphOptimizationLevel;

#[derive(Debug, Clone, Parser)]
#[command(name = "semantic-search-mcp")]
#[command(about = "MCP server exposing semantic index and search tools")]
pub struct McpServerCli {
    #[arg(long, env = "SEMANTIC_SEARCH_ONNX_RUNTIME")]
    pub onnx_runtime_path: Option<PathBuf>,

    #[arg(long, env = "SEMANTIC_SEARCH_MODEL")]
    pub model_path: Option<PathBuf>,

    #[arg(long, env = "SEMANTIC_SEARCH_TOKENIZER")]
    pub tokenizer_path: Option<PathBuf>,

    #[arg(long, env = "SEMANTIC_SEARCH_MODEL_TYPE", value_enum, default_value_t = ModelTypeArg::Veso)]
    pub model_type: ModelTypeArg,

    #[arg(long, env = "SEMANTIC_SEARCH_EMBEDDING_DIM", default_value_t = 768)]
    pub dim: usize,

    #[arg(long, env = "SEMANTIC_SEARCH_BATCH_SIZE", default_value_t = 32)]
    pub batch_size: usize,

    #[arg(long, env = "SEMANTIC_SEARCH_THREADS", default_value_t = 1)]
    pub num_threads: usize,

    #[arg(long, env = "SEMANTIC_SEARCH_INTRA_THREADS", default_value_t = 1)]
    pub intra_threads: usize,

    #[arg(long, env = "SEMANTIC_SEARCH_LOG_LEVEL", default_value = "info")]
    pub log_level: String,
}

impl McpServerCli {
    pub fn resolve(&self) -> anyhow::Result<ResolvedConfig> {
        let log_path = match std::env::var_os("SEMANTIC_SEARCH_LOG_PATH") {
            Some(p) => PathBuf::from(p),
            None => platform_log_path()?,
        };

        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }

        init_logger(
            log_path
                .to_str()
                .ok_or_else(|| anyhow!("log path must be valid UTF-8"))?,
            &self.log_level,
        )?;

        let model_type: crate::embedding::utils::EmbeddingModelType = self.model_type.into();
        let runtime_path = match &self.onnx_runtime_path {
            Some(p) => p.to_string_lossy().to_string(),
            None => crate::resources::paths::default_onnxruntime_path()?
                .to_string_lossy()
                .to_string(),
        };
        let model_path = match &self.model_path {
            Some(p) => p.clone(),
            None => crate::resources::paths::default_embedding_model_path(model_type.clone())?,
        };
        let tokenizer_path = match &self.tokenizer_path {
            Some(p) => p.clone(),
            None => crate::resources::paths::default_tokenizer_path(model_type)?,
        };

        Ok(ResolvedConfig {
            onnx_runtime_config: OnnxRuntimeConfig {
                runtime_path,
                intra_threads: self.intra_threads,
                optimization_level: GraphOptimizationLevel::Level1,
            },
            embedding_options: crate::embedding::utils::EmbeddingOptions {
                model_type: self.model_type.into(),
                model_path,
                tokenizer_path,
                dim: self.dim,
                batch_size: self.batch_size,
                num_threads: self.num_threads,
            },
            output: OutputFormat::Json,
            log_path,
        })
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IndexToolRequest {
    #[schemars(description = "Index layer to build: file, symbol, content, or all (file+symbol)")]
    pub layer: Option<IndexLayerArg>,
    #[schemars(
        description = "Optional absolute path to repository root; if omitted, uses SEMANTIC_SEARCH_PROJECT"
    )]
    pub project: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct StartIndexToolRequest {
    #[schemars(description = "Index layer to build: file, symbol, content, or all (file+symbol)")]
    pub layer: Option<IndexLayerArg>,
    #[schemars(
        description = "Optional absolute path to repository root; if omitted, uses SEMANTIC_SEARCH_PROJECT"
    )]
    pub project: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IndexProgressToolRequest {
    #[schemars(
        description = "Optional absolute path to repository root; if omitted, uses SEMANTIC_SEARCH_PROJECT"
    )]
    pub project: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct StopIndexToolRequest {
    #[schemars(
        description = "Optional absolute path to repository root; if omitted, uses SEMANTIC_SEARCH_PROJECT"
    )]
    pub project: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchToolRequest {
    #[schemars(description = "Semantic query string")]
    pub query: String,
    #[schemars(
        description = "Optional absolute path to repository root; if omitted, uses SEMANTIC_SEARCH_PROJECT"
    )]
    pub project: Option<String>,
    #[schemars(description = "Search layer: file, symbol, content, or all (file+symbol)")]
    pub layer: Option<IndexLayerArg>,
    #[schemars(description = "Maximum number of hits to return")]
    pub limit: Option<usize>,
    #[schemars(description = "Minimum similarity threshold")]
    pub threshold: Option<f32>,
    #[schemars(description = "Optional relative paths to filter by")]
    pub paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IndexLayerArg {
    File,
    Symbol,
    Content,
    All,
}

impl From<IndexLayerArg> for LayerSelector {
    fn from(value: IndexLayerArg) -> Self {
        match value {
            IndexLayerArg::File => LayerSelector::File,
            IndexLayerArg::Symbol => LayerSelector::Symbol,
            IndexLayerArg::Content => LayerSelector::Content,
            IndexLayerArg::All => LayerSelector::All,
        }
    }
}

pub struct SemanticSearchMcp {
    tool_router: ToolRouter<SemanticSearchMcp>,
    executor: Arc<dyn McpCommandExecutor>,
}

#[async_trait]
trait McpCommandExecutor: Send + Sync {
    async fn execute_start_index(
        &self,
        request: StartIndexRequest,
    ) -> anyhow::Result<CommandResponse>;
    async fn execute_index_progress(
        &self,
        request: IndexProgressRequest,
    ) -> anyhow::Result<CommandResponse>;
    async fn execute_stop_index(
        &self,
        request: StopIndexRequest,
    ) -> anyhow::Result<CommandResponse>;
    async fn execute_search(&self, request: SearchRequest) -> anyhow::Result<CommandResponse>;
}

#[async_trait]
impl McpCommandExecutor for RegistryCommandHandler {
    async fn execute_start_index(
        &self,
        request: StartIndexRequest,
    ) -> anyhow::Result<CommandResponse> {
        let path = PathBuf::from(&request.project);
        let backend = self.registry.get_or_create(&path, &self.shared).await?;
        dispatch_execute_start_index(backend.as_ref(), request).await
    }

    async fn execute_index_progress(
        &self,
        request: IndexProgressRequest,
    ) -> anyhow::Result<CommandResponse> {
        let path = PathBuf::from(&request.project);
        let backend = self.registry.get_or_create(&path, &self.shared).await?;
        dispatch_execute_index_progress(backend.as_ref(), request).await
    }

    async fn execute_stop_index(
        &self,
        request: StopIndexRequest,
    ) -> anyhow::Result<CommandResponse> {
        let path = PathBuf::from(&request.project);
        let backend = self.registry.get_or_create(&path, &self.shared).await?;
        dispatch_execute_stop_index(backend.as_ref(), request).await
    }

    async fn execute_search(&self, request: SearchRequest) -> anyhow::Result<CommandResponse> {
        let path = PathBuf::from(&request.project);
        let backend = self.registry.get_or_create(&path, &self.shared).await?;
        dispatch_execute_search(backend.as_ref(), request).await
    }
}

/// 将 tool 可选 `project` 解析为与 [`project_path_key`] 一致的字符串，供 registry 与 JSON 的 `project` 字段共用。
fn resolve_mcp_project_key(tool_project: Option<&str>) -> anyhow::Result<String> {
    match tool_project {
        Some(s) => {
            let p = Path::new(s);
            validate_project_exists(p)?;
            project_path_key(p)
        }
        None => {
            let project_raw = std::env::var_os("SEMANTIC_SEARCH_PROJECT").ok_or_else(|| {
                anyhow!(
                    "SEMANTIC_SEARCH_PROJECT environment variable is not set; either export it or pass `project` in the tool request"
                )
            })?;
            let project_path = PathBuf::from(project_raw);
            validate_project_exists(&project_path)?;
            project_path_key(&project_path)
        }
    }
}

#[tool_router]
impl SemanticSearchMcp {
    pub async fn from_config(config: ResolvedConfig) -> anyhow::Result<Self> {
        let shared = Arc::new(McpSharedConfig {
            onnx_runtime_path: config.onnx_runtime_config.runtime_path.clone(),
            onnx_intra_threads: config.onnx_runtime_config.intra_threads,
            embedding_options: config.embedding_options.clone(),
        });
        let registry = Arc::new(ProjectRegistry::new());
        let handler = RegistryCommandHandler::new(Arc::clone(&registry), Arc::clone(&shared));
        Ok(Self {
            tool_router: Self::tool_router(),
            executor: Arc::new(handler),
        })
    }

    #[tool(description = "Start indexing in background (non-blocking)")]
    pub async fn start_index(
        &self,
        request: Parameters<StartIndexToolRequest>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(request) = request;
        let project_key = resolve_mcp_project_key(request.project.as_deref()).map_err(mcp_error)?;
        let response = self
            .executor
            .execute_start_index(StartIndexRequest {
                project: project_key,
                layer: request.layer.unwrap_or(IndexLayerArg::All).into(),
            })
            .await
            .map_err(mcp_error)?;
        Ok(json_tool_result(response))
    }

    #[tool(description = "Get current indexing progress and status")]
    pub async fn index_progress(
        &self,
        request: Parameters<IndexProgressToolRequest>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(req) = request;
        let project_key = resolve_mcp_project_key(req.project.as_deref()).map_err(mcp_error)?;
        let response = self
            .executor
            .execute_index_progress(IndexProgressRequest {
                project: project_key,
            })
            .await
            .map_err(mcp_error)?;
        Ok(json_tool_result(response))
    }

    #[tool(description = "Stop/cancel current indexing task")]
    pub async fn stop_index(
        &self,
        request: Parameters<StopIndexToolRequest>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(req) = request;
        let project_key = resolve_mcp_project_key(req.project.as_deref()).map_err(mcp_error)?;
        let response = self
            .executor
            .execute_stop_index(StopIndexRequest {
                project: project_key,
            })
            .await
            .map_err(mcp_error)?;
        Ok(json_tool_result(response))
    }

    #[tool(description = "Search the configured repository semantic index")]
    pub async fn search(
        &self,
        request: Parameters<SearchToolRequest>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(request) = request;
        let project_key = resolve_mcp_project_key(request.project.as_deref()).map_err(mcp_error)?;
        let response = self
            .executor
            .execute_search(SearchRequest {
                project: project_key,
                query: request.query,
                layer: request.layer.unwrap_or(IndexLayerArg::Symbol).into(),
                limit: request.limit.unwrap_or(10),
                threshold: request.threshold.unwrap_or(0.5),
                paths: request.paths.unwrap_or_default(),
            })
            .await
            .map_err(mcp_error)?;
        Ok(json_tool_result(response))
    }
}

#[tool_handler]
impl ServerHandler for SemanticSearchMcp {
    fn get_info(&self) -> ServerInfo {
        // rmcp 0.12.x 中 `ServerInfo` 是 `InitializeResult` 的别名，没有 `new()/with_*()` 这些 builder API。
        // 这里直接按协议结构体字段构造即可。
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Use `start_index` before `search` for a new repository. Pass optional `project` (absolute repo root) when working with multiple directories; otherwise the server reads SEMANTIC_SEARCH_PROJECT from the environment. For code questions (where/how/who calls what), prefer `search`. Searching during indexing may return partial results.".to_string(),
            ),
        }
    }
}

fn json_tool_result(response: crate::tools::service::CommandResponse) -> CallToolResult {
    let json = response
        .render(OutputFormat::Json)
        .unwrap_or_else(|err| format!("{{\"error\":\"{}\"}}", err));
    CallToolResult::success(vec![Content::text(json)])
}

fn mcp_error(error: anyhow::Error) -> McpError {
    McpError {
        code: ErrorCode(-32603),
        message: Cow::from(error.to_string()),
        data: None,
    }
}

pub async fn run_mcp_server(cli: McpServerCli) -> anyhow::Result<()> {
    let config = cli.resolve()?;
    let service = SemanticSearchMcp::from_config(config)
        .await?
        .serve(stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockExecutor;

    #[async_trait]
    impl McpCommandExecutor for MockExecutor {
        async fn execute_start_index(
            &self,
            request: StartIndexRequest,
        ) -> anyhow::Result<CommandResponse> {
            Ok(CommandResponse::StartIndex(
                crate::tools::service::StartIndexResponse {
                    project: request.project,
                    layers: vec![request.layer.to_layers()[0].to_string()],
                    status: crate::tools::service::IndexRunStatus::Running,
                    usage_hint: "hint".to_string(),
                },
            ))
        }

        async fn execute_index_progress(
            &self,
            request: IndexProgressRequest,
        ) -> anyhow::Result<CommandResponse> {
            Ok(CommandResponse::IndexProgress(
                crate::tools::service::IndexProgressResponse {
                    project: request.project,
                    status: crate::tools::service::IndexRunStatus::Running,
                    progress: crate::index::utils::IndexProgress {
                        total_file_count: 1,
                        total_symbol_count: 1,
                        handled_file_count: 0,
                        handled_symbol_count: 0,
                    },
                    last_error: None,
                    usage_hint: "hint".to_string(),
                },
            ))
        }

        async fn execute_stop_index(
            &self,
            request: StopIndexRequest,
        ) -> anyhow::Result<CommandResponse> {
            Ok(CommandResponse::StopIndex(
                crate::tools::service::StopIndexResponse {
                    project: request.project,
                    status: crate::tools::service::IndexRunStatus::Cancelled,
                    usage_hint: "hint".to_string(),
                },
            ))
        }

        async fn execute_search(&self, request: SearchRequest) -> anyhow::Result<CommandResponse> {
            let layer_label = match request.layer {
                crate::tools::service::LayerSelector::File => "file",
                crate::tools::service::LayerSelector::Symbol => "symbol",
                crate::tools::service::LayerSelector::Content => "content",
                crate::tools::service::LayerSelector::All => "all",
            };
            Ok(CommandResponse::Search(
                crate::tools::service::SearchResponse {
                    project: request.project,
                    query: request.query,
                    layer: layer_label.to_string(),
                    limit: request.limit,
                    threshold: request.threshold,
                    usage_hint: "hint".to_string(),
                    results: vec![],
                },
            ))
        }
    }

    fn test_server() -> SemanticSearchMcp {
        SemanticSearchMcp {
            tool_router: SemanticSearchMcp::tool_router(),
            executor: Arc::new(MockExecutor),
        }
    }

    #[test]
    fn tool_router_registers_index_and_search() {
        let server = test_server();
        let tools = server.tool_router.list_all();
        let names = tools
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        // 阻塞式 `index` 已并入 `start_index` 语义，路由不再单独注册名为 index 的工具。
        assert!(names.contains(&"start_index".to_string()));
        assert!(names.contains(&"index_progress".to_string()));
        assert!(names.contains(&"stop_index".to_string()));
        assert!(names.contains(&"search".to_string()));
    }

    #[tokio::test]
    async fn search_tool_returns_json_text_payload() {
        let server = test_server();
        let dir = std::env::temp_dir().join(format!("mcp_search_tool_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let dir_s = dir.to_string_lossy().to_string();
        let result = server
            .search(Parameters(SearchToolRequest {
                query: "audio player".to_string(),
                project: Some(dir_s),
                layer: Some(IndexLayerArg::Symbol),
                limit: Some(5),
                threshold: Some(0.4),
                paths: Some(vec!["src/a.ts".to_string()]),
            }))
            .await
            .expect("search tool");

        let rendered = format!("{result:?}");
        assert!(rendered.contains("audio player"));
        assert!(rendered.contains("symbol"));
        assert!(rendered.contains("hint"));
    }
}
