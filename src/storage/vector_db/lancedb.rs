use std::{path::Path, sync::Arc};

use arrow_array::{
    ArrayRef, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
    builder::{FixedSizeListBuilder, Float32Builder},
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures::TryStreamExt;
use lancedb::{
    Connection, DistanceType, Result, Table, connect,
    query::{ExecutableQuery, QueryBase},
};
use uuid::Uuid;

use crate::common::data::{Chunk, ChunkInfo, IndexType, QueryResult};

/// LanceDB vector store for a **single** project (one DB directory per codebase).
pub struct LancedbChunkStore {
    conn: Connection,
    schema: SchemaRef,
    dim: i32,
}

impl LancedbChunkStore {
    pub async fn open(db_path: &Path, dim: i32) -> Result<Self> {
        let uri = db_path.to_string_lossy();
        log::info!("Connecting to LanceDB at {}", uri);
        let conn = connect(&uri).execute().await?;

        let schema = Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("layer", DataType::Utf8, false),
            Field::new("file_path", DataType::Utf8, false),
            Field::new("lang", DataType::Utf8, false),
            Field::new("chunk_info", DataType::Utf8, false),
            Field::new(
                "embedding",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dim),
                false,
            ),
        ]);
        Ok(Self {
            conn,
            schema: SchemaRef::new(schema),
            dim,
        })
    }

    fn table_name(layer: IndexType) -> String {
        layer.to_string()
    }

    pub async fn get_table(&self, layer: IndexType) -> Result<Table> {
        let table_name = Self::table_name(layer);
        let table = self.conn.open_table(&table_name).execute().await?;
        Ok(table)
    }

    pub async fn get_or_create_table(&self, layer: IndexType) -> Result<Table> {
        let table_name = Self::table_name(layer);
        let table = self.conn.table_names().execute().await?;

        if table.contains(&table_name) {
            log::info!("Table {} already exists", table_name);
            let table = self.conn.open_table(&table_name).execute().await?;
            Ok(table)
        } else {
            log::info!("Creating table {}", table_name);
            let table = self
                .conn
                .create_empty_table(&table_name, self.schema.clone())
                .execute()
                .await?;
            Ok(table)
        }
    }

    pub async fn append_chunks(&self, layer: IndexType, chunks: Vec<Chunk>) -> Result<()> {
        let table = self.get_or_create_table(layer).await?;

        let mut ids = Vec::new();
        let mut layers = Vec::new();
        let mut file_paths = Vec::new();
        let mut langs = Vec::new();
        let mut chunk_infos = Vec::new();

        let vector_item_builder = Float32Builder::new();
        let mut vector_builder = FixedSizeListBuilder::new(vector_item_builder, self.dim);

        for chunk in chunks {
            let id = Uuid::new_v4().to_string();

            ids.push(id);
            layers.push(chunk.info.layer.to_string());
            file_paths.push(chunk.info.file_path.clone());
            langs.push(chunk.info.lang.clone());
            chunk_infos.push(serde_json::to_string(&chunk.info).unwrap_or_default());

            vector_builder.values().append_slice(&chunk.embedding);
            vector_builder.append(true);
        }

        let id_array: ArrayRef = Arc::new(StringArray::from(ids));
        let layer_array: ArrayRef = Arc::new(StringArray::from(layers));
        let file_array: ArrayRef = Arc::new(StringArray::from(file_paths));
        let lang_array: ArrayRef = Arc::new(StringArray::from(langs));
        let chunk_info_array: ArrayRef = Arc::new(StringArray::from(chunk_infos));
        let embedding_array: ArrayRef = Arc::new(vector_builder.finish());

        let batch = RecordBatch::try_new(
            self.schema.clone(),
            vec![
                id_array,
                layer_array,
                file_array,
                lang_array,
                chunk_info_array,
                embedding_array,
            ],
        )?;

        let batch_iter = RecordBatchIterator::new(vec![Ok(batch)], self.schema.clone());

        table.add(Box::new(batch_iter)).execute().await?;

        Ok(())
    }

    pub async fn search(
        &self,
        query_vector: Vec<f32>,
        limit: usize,
        threshold: f32,
        layer: IndexType,
        paths: Vec<String>,
    ) -> Result<Vec<QueryResult>> {
        log::info!("Searching for {} chunks in {}", limit, layer);

        let table = match self.get_table(layer).await {
            Ok(table) => table,
            Err(err) => {
                log::error!("Failed to open table: {}", err);
                return Ok(Vec::new());
            }
        };

        let mut search = table
            .query()
            .nearest_to(query_vector)?
            .limit(limit)
            .distance_type(DistanceType::Cosine);

        if let Some(filter) = construct_filter(&paths) {
            search = search.only_if(filter);
        }

        let results = search.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut output = Vec::new();
        for batch in batches {
            let info_array = batch
                .column_by_name("chunk_info")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();

            let distance_array = batch
                .column_by_name("_distance")
                .unwrap()
                .as_any()
                .downcast_ref::<Float32Array>()
                .unwrap();

            for i in 0..batch.num_rows() {
                let distance = distance_array.value(i);
                let score = 1.0 - distance;

                if score < threshold {
                    continue;
                }

                let info = info_array.value(i);
                let info = serde_json::from_str::<ChunkInfo>(&info).unwrap_or_default();
                output.push(QueryResult { score, info });
            }
        }

        Ok(output)
    }

    pub async fn delete_chunks_by_path(&self, file_path: &str, layer: IndexType) -> Result<()> {
        self.delete_chunks_by_paths(vec![file_path.to_string()], layer)
            .await
    }

    pub async fn delete_chunks_by_paths(&self, paths: Vec<String>, layer: IndexType) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }

        let table = self.get_or_create_table(layer).await?;
        if let Some(filter) = construct_filter(&paths) {
            table.delete(&filter).await?;
        }

        Ok(())
    }

    pub async fn delete_table(&self, layer: IndexType) -> Result<()> {
        let table_name = Self::table_name(layer);
        let tables = self.conn.table_names().execute().await?;
        if tables.contains(&table_name) {
            self.conn.drop_table(&table_name).await?;
        }
        Ok(())
    }
}

fn escape(value: &str) -> String {
    value.replace('\'', "''")
}

fn construct_filter(paths: &[String]) -> Option<String> {
    if !paths.is_empty() {
        let path_list = paths
            .iter()
            .map(|path| format!("'{}'", escape(path)))
            .collect::<Vec<String>>()
            .join(", ");
        Some(format!("file_path IN ({})", path_list))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::test::utils::temp_dir;

    use super::*;
    use crate::common::data::{Chunk, ChunkInfo, IndexType};

    async fn setup_table() -> Result<LancedbChunkStore> {
        let dir = temp_dir().join(format!("db/vectordb_{}", uuid::Uuid::new_v4()));
        let db = LancedbChunkStore::open(&dir, 2).await?;

        db.delete_table(IndexType::File).await?;
        db.delete_table(IndexType::Symbol).await?;

        db.get_or_create_table(IndexType::File).await?;
        db.get_or_create_table(IndexType::Symbol).await?;
        Ok(db)
    }

    #[tokio::test]
    async fn test_delete_chunks() -> anyhow::Result<()> {
        let db = setup_table().await?;

        let file_chunks = vec![
            Chunk {
                embedding_content: String::new(),
                embedding: vec![1.0, 0.0],
                info: ChunkInfo {
                    layer: IndexType::File,
                    file_path: "src/lib.rs".to_string(),
                    lang: "rust".to_string(),
                    range: None,
                    content: None,
                },
                is_last: false,
            },
            Chunk {
                embedding_content: String::new(),
                embedding: vec![0.0, 1.0],
                info: ChunkInfo {
                    layer: IndexType::File,
                    file_path: "src/main.rs".to_string(),
                    lang: "rust".to_string(),
                    range: None,
                    content: None,
                },
                is_last: false,
            },
        ];

        db.append_chunks(IndexType::File, file_chunks).await?;

        db.delete_chunks_by_path("src/lib.rs", IndexType::File)
            .await?;

        let results = db
            .search(vec![1.0, 0.0], 10, 0.0, IndexType::File, vec![])
            .await?;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].info.file_path, "src/main.rs");

        Ok(())
    }

    #[tokio::test]
    async fn test_search_layer_filer() -> anyhow::Result<()> {
        let db = setup_table().await?;

        let symbol_chunks = vec![Chunk {
            embedding_content: String::new(),
            embedding: vec![1.0, 0.0],
            info: ChunkInfo {
                layer: IndexType::Symbol,
                file_path: "src/lib.rs".to_string(),
                lang: "rust".to_string(),
                range: None,
                content: None,
            },
            is_last: false,
        }];

        db.append_chunks(IndexType::Symbol, symbol_chunks).await?;

        let results = db
            .search(
                vec![1.0, 0.0],
                10,
                0.0,
                IndexType::Symbol,
                vec!["src/lib.rs".to_string()],
            )
            .await?;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].info.file_path, "src/lib.rs");

        Ok(())
    }
}
