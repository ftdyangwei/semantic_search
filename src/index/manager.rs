use std::{sync::Arc, time::SystemTime};

use tokio::{runtime::Handle, sync::mpsc};

use crate::{
    common::{
        FileService,
        data::{ChunkMsg, IndexStatus, IndexType, Project},
        utils::{
            calculate_diff, construct_walker, get_relative_path, hash_str, system_time_to_timestamp,
        },
    },
    document_chunker::chunker::ChunkerRegistry,
    embedding::onnx::OnnxEmbedder,
    index::{
        file_checker::FileChecker,
        utils::{CancelToken, ProgressReporter},
        worker::IndexWorker,
    },
    language::language::Language,
    metrics::profiler::IndexProfiler,
    storage::manager::StorageManager,
};

pub struct IndexManager {
    storage_manager: Arc<StorageManager>,
    chunker_registry: ChunkerRegistry,
    file_checker: FileChecker,
    file_service: FileService,
    runtime_handle: Handle,
    worker: Arc<IndexWorker>,
}

impl IndexManager {
    pub fn new(
        storage_manager: Arc<StorageManager>,
        embedder: Arc<OnnxEmbedder>,
        chunker_registry: ChunkerRegistry,
        runtime_handle: Handle,
        num_threads: usize,
        embed_batch_size: usize,
    ) -> Self {
        Self {
            storage_manager: storage_manager.clone(),
            chunker_registry,
            file_checker: FileChecker::new(),
            file_service: FileService::new(),
            runtime_handle,
            worker: Arc::new(IndexWorker::new(
                embedder,
                storage_manager.clone(),
                num_threads,
                embed_batch_size,
            )),
        }
    }

    pub async fn index_project(
        &self,
        project: &Project,
        layer: IndexType,
        profiler: Arc<IndexProfiler>,
        progress_reporter: Arc<dyn ProgressReporter>,
        cancel_token: Arc<dyn CancelToken>,
    ) -> anyhow::Result<()> {
        let curr_files_status =
            scan_project(project, layer, &self.file_checker, &self.file_service).await;

        let prev_files_status = self.storage_manager.get_index_status_by_layer(layer)?;

        let diff = calculate_diff(&curr_files_status, &prev_files_status);

        log::info!(
            "Poroject {} layer {}: deleted: {}, new: {}, updated: {}",
            project.root_path.display(),
            layer,
            diff.deleted.len(),
            diff.new.len(),
            diff.updated.len(),
        );

        if !diff.deleted.is_empty() {
            self.handle_deleted(project, layer, &diff.deleted).await?;
        }

        let mut to_index: Vec<&IndexStatus> = Vec::new();
        to_index.extend(diff.new.iter());
        to_index.extend(diff.updated.iter());

        self.handle_upsert_new(project, layer, &to_index, profiler, progress_reporter)
            .await?;

        Ok(())
    }

    pub async fn delete_project(&self, project: &Project, layer: IndexType) -> anyhow::Result<()> {
        log::info!(
            "deleting project {} from layer {}",
            project.root_path.display(),
            layer
        );
        self.storage_manager.delete_index_status_by_layer(layer)?;
        self.storage_manager.delete_layer_table(layer).await?;
        log::info!(
            "deleted project {} from layer {}",
            project.root_path.display(),
            layer
        );
        Ok(())
    }

    async fn handle_deleted(
        &self,
        project: &Project,
        layer: IndexType,
        files: &[&IndexStatus],
    ) -> anyhow::Result<()> {
        log::info!(
            "handling deleted files for project {} from layer {}",
            project.root_path.display(),
            layer
        );
        for file in files {
            log::info!("deleting file {} from layer {}", file.file_path, layer);
            self.storage_manager
                .delete_index_status(&file.file_path, layer)
                .unwrap_or_else(|e| {
                    log::error!(
                        "failed to delete index status for file {}: {}",
                        file.file_path,
                        e
                    )
                });
            self.storage_manager
                .delete_chunks(&file.file_path, layer)
                .await
                .unwrap_or_else(|e| {
                    log::error!("failed to delete chunks for file {}: {}", file.file_path, e)
                });
            log::info!("deleted file {} from layer {}", file.file_path, layer);
        }
        log::info!(
            "handled deleted files for project {} from layer {}",
            project.root_path.display(),
            layer
        );
        Ok(())
    }

    async fn handle_upsert_new(
        &self,
        project: &Project,
        layer: IndexType,
        files: &[&IndexStatus],
        profiler: Arc<IndexProfiler>,
        progress_reporter: Arc<dyn ProgressReporter>,
    ) -> anyhow::Result<()> {
        if files.is_empty() {
            profiler.finish_layer(layer);
            progress_reporter.on_completed();
            return Ok(());
        }

        profiler.set_total_count(layer, files.len());
        progress_reporter.on_progress(profiler.index_progress());

        log::info!(
            "handling upserted new files for project {} from layer {}",
            project.root_path.display(),
            layer
        );

        let (chunk_tx, chunk_rx) = mpsc::channel(1024);
        let (embed_tx, embed_rx) = mpsc::channel(1024);

        let embed_worker = self.worker.clone();
        let embed_profiler = profiler.clone();
        self.runtime_handle.spawn(async move {
            embed_worker
                .embed(chunk_rx, embed_tx, layer, embed_profiler)
                .await;
        });

        let insert_worker = self.worker.clone();
        let insert_profiler = profiler.clone();
        let insert_progress_reporter = progress_reporter.clone();
        let insert_project = project.clone();
        self.runtime_handle.spawn(async move {
            insert_worker
                .insert(
                    insert_project,
                    layer,
                    embed_rx,
                    insert_profiler,
                    insert_progress_reporter,
                )
                .await;
        });

        for file in files {
            log::info!("chunking file {} into layer {}", file.file_path, layer);

            let chunks = self
                .chunk_file(project, &file.file_path, layer)
                .await
                .unwrap_or_else(|e| {
                    log::error!("failed to chunk file {}: {}", file.file_path, e);
                    vec![]
                });

            chunk_tx
                .send(ChunkMsg::FileStart {
                    file_path: file.file_path.clone(),
                })
                .await
                .unwrap_or_else(|e| log::error!("failed to send file start for embed: {}", e));

            for chunk in chunks {
                chunk_tx
                    .send(chunk)
                    .await
                    .unwrap_or_else(|e| log::error!("failed to send chunk for embed: {}", e));
            }

            chunk_tx
                .send(ChunkMsg::FileEnd {
                    file_status: (*file).clone(),
                })
                .await
                .unwrap_or_else(|e| log::error!("failed to send file end for embed: {}", e));
        }
        Ok(())
    }

    async fn chunk_file(
        &self,
        project: &Project,
        file_path: &str,
        layer: IndexType,
    ) -> anyhow::Result<Vec<ChunkMsg>> {
        let full_path = project.root_path.join(file_path);
        let language: Language = full_path.as_path().into();
        let chunker = self
            .chunker_registry
            .get_by_layer(layer, language)
            .ok_or(anyhow::anyhow!("no chunker found for {}", file_path))?;

        let mut chunks = chunker.split(&full_path, file_path).await?;
        chunks.last_mut().map(|f| f.is_last = true);

        Ok(chunks
            .into_iter()
            .map(|chunk| ChunkMsg::Chunk(chunk))
            .collect())
    }
}

async fn scan_project(
    project: &Project,
    layer: IndexType,
    file_checker: &FileChecker,
    file_service: &FileService,
) -> Vec<IndexStatus> {
    log::info!(
        "scanning project {} from layer {}",
        project.root_path.display(),
        layer
    );

    let walker = construct_walker(project.root_path.as_path(), true, &[], &[], None);
    let mut index_statuses = Vec::new();

    for entry in walker.flatten() {
        let full_path = entry.path().to_path_buf();
        if full_path.is_dir() || !file_checker.is_supported(&full_path) {
            continue;
        }

        let file_stat = file_service.file_stat(&full_path).await;

        match file_stat {
            Ok(file_stat) => {
                let relative_path = get_relative_path(&full_path, project.root_path.as_path())
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();

                let content = match file_service.read_file_to_string(&full_path).await {
                    Ok(content) => content,
                    Err(e) => {
                        log::error!(
                            "failed to read file content for {}: {}",
                            full_path.display(),
                            e
                        );
                        continue;
                    }
                };

                let file_hash = hash_str(&content);
                let mtime = file_stat.modified.unwrap_or(SystemTime::now());
                let ctime = file_stat.created.unwrap_or(SystemTime::now());
                let size = file_stat.len;
                let indexed_at = system_time_to_timestamp(SystemTime::now());

                index_statuses.push(IndexStatus {
                    file_path: relative_path,
                    layer,
                    file_hash,
                    mtime: system_time_to_timestamp(mtime),
                    ctime: system_time_to_timestamp(ctime),
                    size,
                    indexed_at,
                });
            }
            Err(e) => {
                log::error!("failed to get file stat for {}: {}", full_path.display(), e);
                continue;
            }
        }
    }

    index_statuses
}
