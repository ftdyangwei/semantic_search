use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use crate::common::data::{Chunk, IndexType};
use crate::document_chunker::file::FileChunker;
use crate::document_chunker::symbol::arkts::ArkTsChunker;
use crate::document_chunker::symbol::cangjie::CangjieChunker;
use crate::document_chunker::symbol::c::CChunker;
use crate::document_chunker::symbol::cpp::CppChunker;
use crate::document_chunker::symbol::ts::TsChunker;
use crate::language::language::Language;

#[async_trait]
pub trait Chunker: Send + Sync {
    async fn split(&self, path: &Path, relative_path: &str) -> anyhow::Result<Vec<Chunk>>;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChunkerKey {
    File,
    Symbol(Language),
    Content,
}

pub struct ChunkerRegistry {
    chunkers: HashMap<ChunkerKey, Arc<dyn Chunker>>,
}

impl ChunkerRegistry {
    pub fn new() -> Self {
        Self {
            chunkers: HashMap::new(),
        }
    }

    pub fn register(&mut self, key: ChunkerKey, chunker: Arc<dyn Chunker>) {
        self.chunkers.insert(key, chunker);
    }

    pub fn register_chunkers(&mut self) {
        self.register(ChunkerKey::File, Arc::new(FileChunker::new()));
        if let Ok(arkts_chunker) = ArkTsChunker::new() {
            self.register(ChunkerKey::Symbol(Language::Arkts), Arc::new(arkts_chunker));
        }

        if let Ok(ts_chunker) = TsChunker::new() {
            self.register(
                ChunkerKey::Symbol(Language::Typescript),
                Arc::new(ts_chunker),
            );
        }

        if let Ok(cj_chunker) = CangjieChunker::new() {
            self.register(
                ChunkerKey::Symbol(Language::Cangjie),
                Arc::new(cj_chunker),
            );

        if let Ok(c_chunker) = CChunker::new() {
            self.register(ChunkerKey::Symbol(Language::C), Arc::new(c_chunker));
        }

        if let Ok(cpp_chunker) = CppChunker::new() {
            self.register(ChunkerKey::Symbol(Language::Cpp), Arc::new(cpp_chunker));

        }
    }

    pub fn get(&self, key: &ChunkerKey) -> Option<Arc<dyn Chunker>> {
        self.chunkers.get(key).cloned()
    }

    pub fn get_by_layer(&self, layer: IndexType, lang: Language) -> Option<Arc<dyn Chunker>> {
        let key = match layer {
            IndexType::File => ChunkerKey::File,
            IndexType::Symbol => ChunkerKey::Symbol(lang),
            IndexType::Content => ChunkerKey::Content,
        };
        self.get(&key)
    }
}
