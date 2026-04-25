use std::path::{Path, PathBuf};

use lancedb::Table;
use serde::Deserialize;

use crate::{
    common::data::{Chunk, IndexStatus, IndexType, Project, QueryResult},
    storage::{rdb::sqlite::IndexStatusStore, vector_db::lancedb::LancedbChunkStore},
};

#[derive(Deserialize, Clone)]
pub struct StorageOptions {
    pub index_db_path: PathBuf,
    pub vector_db_path: PathBuf,
}

/// Per-project storage: one SQLite file + one LanceDB directory for a single codebase.
pub struct StorageManager {
    options: StorageOptions,
    pub index_status_store: IndexStatusStore,
    pub chunk_store: LancedbChunkStore,
}

impl StorageManager {
    pub async fn new(options: StorageOptions, dim: i32) -> anyhow::Result<Self> {
        Ok(Self {
            index_status_store: IndexStatusStore::new(options.index_db_path.clone()),
            chunk_store: LancedbChunkStore::open(&options.vector_db_path, dim).await?,
            options,
        })
    }

    pub fn storage_options(&self) -> StorageOptions {
        self.options.clone()
    }

    pub fn get_or_create_project(
        &self,
        root_path: &Path,
        embedding_model: &str,
    ) -> anyhow::Result<Project> {
        let project = self
            .index_status_store
            .get_or_create_project(root_path, embedding_model)?;
        Ok(project)
    }

    /// 更新本库存储的工程元数据中的「索引完成时间」（单库至多一行）。
    pub fn update_project_index_finished_time(
        &self,
        index_finished_time: u64,
    ) -> anyhow::Result<()> {
        self.index_status_store
            .update_project_index_finished_time(index_finished_time)?;
        Ok(())
    }

    pub fn get_project_index_finished_time(&self) -> anyhow::Result<Option<u64>> {
        let project_index_finished_time =
            self.index_status_store.get_project_index_finished_time()?;
        Ok(project_index_finished_time)
    }

    pub fn get_index_status_by_layer(&self, layer: IndexType) -> anyhow::Result<Vec<IndexStatus>> {
        let index_status = self.index_status_store.get_index_status_by_layer(layer)?;
        Ok(index_status)
    }

    pub fn get_index_status(
        &self,
        file_path: &str,
        layer: IndexType,
    ) -> anyhow::Result<Option<IndexStatus>> {
        let index_status = self
            .index_status_store
            .get_index_status_by_path(file_path, layer)?;
        Ok(index_status)
    }

    pub fn upsert_index_status(&self, index_status: &IndexStatus) -> anyhow::Result<()> {
        self.index_status_store.upsert_index_status(index_status)?;
        Ok(())
    }

    pub fn delete_index_status_by_layer(&self, layer: IndexType) -> anyhow::Result<()> {
        self.index_status_store
            .delete_index_status_by_layer(layer)?;
        Ok(())
    }

    pub fn delete_index_status(&self, file_path: &str, layer: IndexType) -> anyhow::Result<()> {
        self.index_status_store
            .delete_index_status_by_path(file_path, layer)?;
        Ok(())
    }

    pub async fn get_or_create_chunk_table(&self, layer: IndexType) -> anyhow::Result<Table> {
        let table = self.chunk_store.get_or_create_table(layer).await?;
        Ok(table)
    }

    pub async fn append_chunks(&self, layer: IndexType, chunks: Vec<Chunk>) -> anyhow::Result<()> {
        self.chunk_store.append_chunks(layer, chunks).await?;
        Ok(())
    }

    /// Drop the LanceDB table for this layer (this project DB only).
    pub async fn delete_layer_table(&self, layer: IndexType) -> anyhow::Result<()> {
        self.chunk_store.delete_table(layer).await?;
        Ok(())
    }

    pub async fn delete_chunks(&self, file_path: &str, layer: IndexType) -> anyhow::Result<()> {
        self.chunk_store
            .delete_chunks_by_path(file_path, layer)
            .await?;
        Ok(())
    }

    pub async fn search(
        &self,
        query_vector: Vec<f32>,
        limit: usize,
        threshold: f32,
        layer: IndexType,
        paths: Vec<String>,
    ) -> anyhow::Result<Vec<QueryResult>> {
        let results = self
            .chunk_store
            .search(query_vector, limit, threshold, layer, paths)
            .await?;
        Ok(results)
    }
}
