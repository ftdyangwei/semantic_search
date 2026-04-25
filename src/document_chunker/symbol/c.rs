//! Symbol-level chunks for C (`.c`, `.h`) using the tree-sitter-c grammar.

use std::path::Path;

use async_trait::async_trait;
use parking_lot::Mutex;
use tree_sitter::Parser;

use crate::common::FileService;
use crate::common::data::Chunk;
use crate::document_chunker::chunker::Chunker;
use crate::document_chunker::symbol::{SymbolKind, SymbolPipeline};
use crate::language::language::Language;

pub struct CChunker {
    parser: Mutex<Parser>,
    file_service: FileService,
    pipeline: SymbolPipeline,
}

impl CChunker {
    pub fn new() -> anyhow::Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .map_err(|e| anyhow::anyhow!("tree-sitter C grammar: {e}"))?;
        Ok(Self {
            parser: Mutex::new(parser),
            file_service: FileService::new(),
            pipeline: SymbolPipeline::new(Language::C.id()),
        })
    }
}

#[async_trait]
impl Chunker for CChunker {
    async fn split(&self, path: &Path, relative_path: &str) -> anyhow::Result<Vec<Chunk>> {
        let source = self.file_service.read_file_to_string(path).await?;
        let tree = {
            let mut parser = self.parser.lock();
            parser
                .parse(&source, None)
                .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned None"))?
        };
        Ok(self.pipeline.split_file_to_chunks(
            &tree,
            &source,
            relative_path,
            SymbolKind::from_node_kind,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::document_chunker::chunker::Chunker;

    fn temp_c_file(name: &str, source: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("c_chunker_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(name);
        fs::write(&path, source).expect("write");
        (path, dir)
    }

    #[tokio::test]
    async fn split_extracts_c_symbols() {
        let src = r#"
typedef struct Person {
  int age;
} Person;

enum Mode {
  MODE_A = 0,
  MODE_B = 1,
};

static int add(int a, int b) { return a + b; }

int main(void) {
  return add(1, 2);
}
"#;
        let (path, dir) = temp_c_file("sample.c", src);
        let chunker = CChunker::new().expect("CChunker::new");
        let chunks = chunker.split(&path, "sample.c").await.expect("split");
        assert!(!chunks.is_empty());

        let got = chunks
            .iter()
            .map(|c| c.embedding_content.as_str())
            .collect::<Vec<_>>();
        // C grammar should at least surface struct/enum/function_definition.
        assert!(got.contains(&"Person"));
        assert!(got.contains(&"Mode"));
        assert!(got.contains(&"add"));
        assert!(got.contains(&"main"));

        fs::remove_dir_all(&dir).ok();
    }
}
