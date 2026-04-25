use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use clap::ValueEnum;
use ort::session::builder::GraphOptimizationLevel;
use parking_lot::Mutex;
use serde::Serialize;

use crate::{
    common::data::{IndexType, QueryResult},
    embedding::utils::{EmbeddingModelType, EmbeddingOptions, OnnxRuntimeConfig},
    index::utils::{CancelToken, IndexProgress, ProgressReporter, SimpleCancelToken},
    manager::SemanticSearchManager,
    metrics::profiler::IndexProfiler,
    storage::manager::StorageOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ModelTypeArg {
    Baize,
    Veso,
}

impl From<ModelTypeArg> for EmbeddingModelType {
    fn from(value: ModelTypeArg) -> Self {
        match value {
            ModelTypeArg::Baize => EmbeddingModelType::Baize,
            ModelTypeArg::Veso => EmbeddingModelType::Veso,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LayerSelector {
    File,
    Symbol,
    Content,
    All,
}

impl LayerSelector {
    pub fn to_layers(self) -> Vec<IndexType> {
        match self {
            Self::File => vec![IndexType::File],
            Self::Symbol => vec![IndexType::Symbol],
            Self::Content => vec![IndexType::Content],
            Self::All => vec![IndexType::File, IndexType::Symbol],
        }
    }
}

pub struct ResolvedConfig {
    /// ONNX Runtime 相关配置。
    pub onnx_runtime_config: OnnxRuntimeConfig,
    /// 嵌入模型与 tokenizer 配置。
    pub embedding_options: EmbeddingOptions,
    /// 日志输出格式。
    pub output: OutputFormat,
    /// 全局日志文件路径（例如 `${DATA_DIR}/semantic_search/running.log`）。
    pub log_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StartIndexRequest {
    pub project: String,
    pub layer: LayerSelector,
}

#[derive(Debug, Clone)]
pub struct StopIndexRequest {
    pub project: String,
}

#[derive(Debug, Clone)]
pub struct IndexProgressRequest {
    pub project: String,
}

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub project: String,
    pub query: String,
    pub layer: LayerSelector,
    pub limit: usize,
    pub threshold: f32,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexRunStatus {
    NotStarted,
    Running,
    Completed,
    Cancelled,
    Error,
}

#[async_trait]
pub trait SemanticSearchBackend: Send + Sync {
    async fn start_indexing(&self, layers: &[IndexType]) -> anyhow::Result<IndexRunStatus>;
    async fn index_progress(&self) -> anyhow::Result<IndexProgressResponse>;
    async fn stop_indexing(&self) -> anyhow::Result<IndexRunStatus>;
    async fn search_layers(
        &self,
        query: &str,
        limit: usize,
        threshold: f32,
        layers: &[IndexType],
        paths: Vec<String>,
    ) -> anyhow::Result<Vec<QueryResult>>;
}

pub struct ManagerBackend {
    manager: Arc<SemanticSearchManager>,
    session: Arc<Mutex<IndexSession>>,
    project: String,
}

struct IndexSession {
    status: IndexRunStatus,
    progress: IndexProgress,
    last_error: Option<String>,
    cancel_token: Arc<SimpleCancelToken>,
}

impl Default for IndexSession {
    fn default() -> Self {
        Self {
            status: IndexRunStatus::NotStarted,
            progress: IndexProgress {
                total_file_count: 0,
                total_symbol_count: 0,
                handled_file_count: 0,
                handled_symbol_count: 0,
            },
            last_error: None,
            cancel_token: Arc::new(SimpleCancelToken::new()),
        }
    }
}

struct SessionProgressReporter {
    session: Arc<Mutex<IndexSession>>,
}

impl ProgressReporter for SessionProgressReporter {
    fn on_progress(&self, progress: IndexProgress) {
        let mut s = self.session.lock();
        s.progress = progress;
    }

    fn on_completed(&self) {
        let mut s = self.session.lock();
        s.status = IndexRunStatus::Completed;
    }

    fn on_error(&self, error: String) {
        let mut s = self.session.lock();
        s.status = IndexRunStatus::Error;
        s.last_error = Some(error);
    }
}

impl ManagerBackend {
    /// 基于给定工程根与全局配置创建后端。
    pub async fn new(
        project_root: &Path,
        storage_options: StorageOptions,
        config: &ResolvedConfig,
    ) -> anyhow::Result<Self> {
        // This backend is used from binaries that already run inside a Tokio runtime.
        // Creating and dropping a nested runtime can panic during shutdown.
        let handle = tokio::runtime::Handle::current();
        let manager = SemanticSearchManager::new(
            storage_options,
            OnnxRuntimeConfig {
                runtime_path: config.onnx_runtime_config.runtime_path.clone(),
                intra_threads: config.onnx_runtime_config.intra_threads,
                optimization_level: GraphOptimizationLevel::Level1,
            },
            config.embedding_options.clone(),
            project_root,
            handle.clone(),
        )
        .await?;
        Ok(Self {
            manager: Arc::new(manager),
            session: Arc::new(Mutex::new(IndexSession::default())),
            project: project_root.to_string_lossy().to_string(),
        })
    }
}

#[async_trait]
impl SemanticSearchBackend for ManagerBackend {
    async fn start_indexing(&self, layers: &[IndexType]) -> anyhow::Result<IndexRunStatus> {
        let cancel_token: Arc<dyn CancelToken> = {
            let mut s = self.session.lock();
            if s.status == IndexRunStatus::Running {
                anyhow::bail!(
                    "indexing already in progress for this project; poll index_progress or use stop_index first"
                );
            }
            *s = IndexSession {
                status: IndexRunStatus::Running,
                progress: IndexProgress {
                    total_file_count: 0,
                    total_symbol_count: 0,
                    handled_file_count: 0,
                    handled_symbol_count: 0,
                },
                last_error: None,
                cancel_token: Arc::new(SimpleCancelToken::new()),
            };
            s.cancel_token.clone()
        };

        let progress_reporter: Arc<dyn ProgressReporter> = Arc::new(SessionProgressReporter {
            session: self.session.clone(),
        });

        let profiler = Arc::new(IndexProfiler::new(self.manager.storage_options()));
        prepare_profiler_for_selected_layers(&profiler, layers);

        for layer in layers.iter().copied() {
            let manager = self.manager.clone();
            let profiler_clone = profiler.clone();
            let reporter_clone = progress_reporter.clone();
            let cancel_clone = cancel_token.clone();
            tokio::spawn(async move {
                let _result = manager
                    .index_layer(layer, profiler_clone, reporter_clone.clone(), cancel_clone)
                    .await;
            });
        }

        Ok(IndexRunStatus::Running)
    }

    async fn index_progress(&self) -> anyhow::Result<IndexProgressResponse> {
        let guard = self.session.lock();
        Ok(IndexProgressResponse {
            project: self.project.clone(),
            status: guard.status,
            progress: IndexProgress {
                total_file_count: guard.progress.total_file_count,
                total_symbol_count: guard.progress.total_symbol_count,
                handled_file_count: guard.progress.handled_file_count,
                handled_symbol_count: guard.progress.handled_symbol_count,
            },
            last_error: guard.last_error.clone(),
            usage_hint: "Use /index_progress to poll; use /stop_index to cancel; use /search anytime (may be partial while running).".to_string(),
        })
    }

    async fn stop_indexing(&self) -> anyhow::Result<IndexRunStatus> {
        let mut guard = self.session.lock();
        guard.cancel_token.cancel();
        if guard.status == IndexRunStatus::Running {
            guard.status = IndexRunStatus::Cancelled;
        }
        Ok(guard.status)
    }

    async fn search_layers(
        &self,
        query: &str,
        limit: usize,
        threshold: f32,
        layers: &[IndexType],
        paths: Vec<String>,
    ) -> anyhow::Result<Vec<QueryResult>> {
        let mut merged_results: Vec<QueryResult> = Vec::new();
        for layer in layers {
            let mut results = self
                .manager
                .search(query, limit, threshold, *layer, paths.clone())
                .await?;
            merged_results.append(&mut results);
        }
        Ok(merged_results)
    }
}

fn prepare_profiler_for_selected_layers(profiler: &Arc<IndexProfiler>, layers: &[IndexType]) {
    if !layers.contains(&IndexType::File) {
        profiler.finish_layer(IndexType::File);
    }
    if !layers.contains(&IndexType::Symbol) {
        profiler.finish_layer(IndexType::Symbol);
    }
}

pub async fn dispatch_execute_start_index<B: SemanticSearchBackend + ?Sized>(
    backend: &B,
    request: StartIndexRequest,
) -> anyhow::Result<CommandResponse> {
    let layers = request.layer.to_layers();
    let status = backend.start_indexing(&layers).await?;
    Ok(CommandResponse::StartIndex(StartIndexResponse {
        project: request.project,
        layers: layers.iter().map(ToString::to_string).collect(),
        status,
        usage_hint: "Use /index_progress to check progress; /stop_index to cancel; /search is available anytime.".to_string(),
    }))
}

pub async fn dispatch_execute_index_progress<B: SemanticSearchBackend + ?Sized>(
    backend: &B,
    request: IndexProgressRequest,
) -> anyhow::Result<CommandResponse> {
    let mut response = backend.index_progress().await?;
    // 与请求中的工程键对齐（便于多工程 MCP 返回一致路径）
    response.project = request.project;
    Ok(CommandResponse::IndexProgress(response))
}

pub async fn dispatch_execute_stop_index<B: SemanticSearchBackend + ?Sized>(
    backend: &B,
    request: StopIndexRequest,
) -> anyhow::Result<CommandResponse> {
    let status = backend.stop_indexing().await?;
    Ok(CommandResponse::StopIndex(StopIndexResponse {
        project: request.project,
        status,
        usage_hint:
            "You can restart with /start_index. Searching uses whatever has been indexed so far."
                .to_string(),
    }))
}

pub async fn dispatch_execute_search<B: SemanticSearchBackend + ?Sized>(
    backend: &B,
    request: SearchRequest,
) -> anyhow::Result<CommandResponse> {
    let layers = request.layer.to_layers();

    let mut results = backend
        .search_layers(
            &request.query,
            request.limit,
            request.threshold,
            &layers,
            request.paths.clone(),
        )
        .await?;

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(request.limit);

    let layer_label = match request.layer {
        LayerSelector::File => "file",
        LayerSelector::Symbol => "symbol",
        LayerSelector::Content => "content",
        LayerSelector::All => "all",
    };

    Ok(CommandResponse::Search(SearchResponse {
        project: request.project,
        query: request.query,
        layer: layer_label.to_string(),
        limit: request.limit,
        threshold: request.threshold,
        usage_hint: "If results are missing, run /index first or continue searching while indexing is still in progress.".to_string(),
        results: results
            .into_iter()
            .map(|result| SearchResultView {
                score: result.score,
                layer: result.info.layer.to_string(),
                file_path: result.info.file_path,
                lang: result.info.lang,
                range: result.info.range.map(RangeView::from),
                content: result.info.content,
            })
            .collect(),
    }))
}

pub enum CommandResponse {
    StartIndex(StartIndexResponse),
    IndexProgress(IndexProgressResponse),
    StopIndex(StopIndexResponse),
    Search(SearchResponse),
}

#[derive(Debug, Serialize)]
pub struct StartIndexResponse {
    pub project: String,
    pub layers: Vec<String>,
    pub status: IndexRunStatus,
    pub usage_hint: String,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub project: String,
    pub query: String,
    pub layer: String,
    pub limit: usize,
    pub threshold: f32,
    pub results: Vec<SearchResultView>,
    pub usage_hint: String,
}

impl SearchResponse {
    fn render_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(&mut out, "Search results");
        let _ = writeln!(&mut out, "project: {}", self.project);
        let _ = writeln!(&mut out, "query: {}", self.query);
        let _ = writeln!(&mut out, "layer: {}", self.layer);
        if self.results.is_empty() {
            let _ = writeln!(&mut out, "results: none");
        } else {
            let _ = writeln!(&mut out, "results:");
            for result in &self.results {
                let _ = writeln!(
                    &mut out,
                    "- {:.3} {} {}",
                    result.score, result.file_path, result.layer
                );
                if let Some(range) = &result.range {
                    let _ = writeln!(
                        &mut out,
                        "  range: {}:{}-{}:{}",
                        range.start_line,
                        range.start_character,
                        range.end_line,
                        range.end_character
                    );
                }
                if let Some(content) = &result.content {
                    let snippet = content.lines().next().unwrap_or("");
                    let _ = writeln!(&mut out, "  content: {}", snippet);
                }
            }
        }
        let _ = writeln!(&mut out, "next: {}", self.usage_hint);
        out.trim_end().to_string()
    }
}

#[derive(Debug, Serialize)]
pub struct StopIndexResponse {
    pub project: String,
    pub status: IndexRunStatus,
    pub usage_hint: String,
}

#[derive(Debug, Serialize)]
pub struct IndexProgressResponse {
    pub project: String,
    pub status: IndexRunStatus,
    pub progress: IndexProgress,
    pub last_error: Option<String>,
    pub usage_hint: String,
}

impl CommandResponse {
    pub fn render(&self, format: OutputFormat) -> anyhow::Result<String> {
        match format {
            OutputFormat::Json => match self {
                Self::StartIndex(response) => {
                    serde_json::to_string_pretty(response).map_err(Into::into)
                }
                Self::IndexProgress(response) => {
                    serde_json::to_string_pretty(response).map_err(Into::into)
                }
                Self::StopIndex(response) => {
                    serde_json::to_string_pretty(response).map_err(Into::into)
                }
                Self::Search(response) => {
                    serde_json::to_string_pretty(response).map_err(Into::into)
                }
            },
            OutputFormat::Text => Ok(match self {
                Self::StartIndex(response) => format!(
                    "Index started\nproject: {}\nlayers: {}\nstatus: {:?}\nnext: {}",
                    response.project,
                    response.layers.join(", "),
                    response.status,
                    response.usage_hint
                ),
                Self::IndexProgress(response) => format!(
                    "Index progress\nproject: {}\nstatus: {:?}\nhandled: {} files, {} symbols\ntotal: {} files, {} symbols\nnext: {}",
                    response.project,
                    response.status,
                    response.progress.handled_file_count,
                    response.progress.handled_symbol_count,
                    response.progress.total_file_count,
                    response.progress.total_symbol_count,
                    response.usage_hint
                ),
                Self::StopIndex(response) => format!(
                    "Index stopped\nproject: {}\nstatus: {:?}\nnext: {}",
                    response.project, response.status, response.usage_hint
                ),
                Self::Search(response) => response.render_text(),
            }),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SearchResultView {
    pub score: f32,
    pub layer: String,
    pub file_path: String,
    pub lang: String,
    pub range: Option<RangeView>,
    pub content: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RangeView {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

impl From<lsp_types::Range> for RangeView {
    fn from(value: lsp_types::Range) -> Self {
        Self {
            start_line: value.start.line,
            start_character: value.start.character,
            end_line: value.end.line,
            end_character: value.end.character,
        }
    }
}

pub fn validate_project_exists(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "project path does not exist: {}",
            path.display()
        ))
    }
}
