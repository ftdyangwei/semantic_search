use std::path::Path;

type ByteRange = std::ops::Range<usize>;

use async_trait::async_trait;
use lsp_types::{Position, Range as LspRange};
use parking_lot::Mutex;
use tree_sitter::{Node, Parser};

use crate::common::FileService;
use crate::common::data::{Chunk, ChunkInfo, IndexType};
use crate::document_chunker::chunker::Chunker;
use crate::language::language::Language;

pub struct CangjieChunker {
    parser: Mutex<Parser>,
    file_service: FileService,
}

impl CangjieChunker {
    pub fn new() -> anyhow::Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cangjie::LANGUAGE.into())
            .map_err(|e| anyhow::anyhow!("tree-sitter Cangjie grammar: {e}"))?;
        Ok(Self {
            parser: Mutex::new(parser),
            file_service: FileService::new(),
        })
    }
}

#[async_trait]
impl Chunker for CangjieChunker {
    async fn split(&self, path: &Path, relative_path: &str) -> anyhow::Result<Vec<Chunk>> {
        let source = self.file_service.read_file_to_string(path).await?;

        let tree = {
            let mut parser = self.parser.lock();
            parser
                .parse(&source, None)
                .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned None"))?
        };

        let mut symbols: Vec<(ByteRange, CangjieSymbolKind, String)> = Vec::new();
        collect_symbols(tree.root_node(), &source, &mut symbols);

        symbols.retain(|(r, _, _)| !r.is_empty());
        symbols.sort_by_key(|(r, _, _)| r.end - r.start);
        let symbols = pick_non_overlapping(symbols);

        let chunk_count = symbols.len();
        let mut chunks = Vec::with_capacity(chunk_count);
        let lang_id = Language::Cangjie.id();

        for (idx, (range, kind, name)) in symbols.into_iter().enumerate() {
            let content = source.get(range.clone()).unwrap_or("").to_string();
            let embedding_content = format!("[{}] {}\n{}", kind.as_label(), name, content);
            chunks.push(Chunk {
                embedding_content,
                info: ChunkInfo {
                    layer: IndexType::Symbol,
                    lang: lang_id.clone(),
                    file_path: relative_path.to_string(),
                    content: Some(content),
                    range: Some(range_to_lsp(&source, &range)),
                },
                is_last: idx + 1 == chunk_count,
                ..Default::default()
            });
        }

        Ok(chunks)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CangjieSymbolKind {
    Class,
    Interface,
    Function,
    Enum,
    Struct,
    Extend,
    TypeAlias,
}

impl CangjieSymbolKind {
    fn as_label(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Function => "function",
            Self::Enum => "enum",
            Self::Struct => "struct",
            Self::Extend => "extend",
            Self::TypeAlias => "type",
        }
    }

    fn from_node_kind(kind: &str) -> Option<Self> {
        match kind {
            "classDefinition" => Some(Self::Class),
            "interfaceDefinition" => Some(Self::Interface),
            "functionDefinition" => Some(Self::Function),
            "enumDefinition" => Some(Self::Enum),
            "structDefinition" => Some(Self::Struct),
            "extendDefinition" => Some(Self::Extend),
            "typeAlias" => Some(Self::TypeAlias),
            _ => None,
        }
    }

    /// Map each declaration kind to its name child node kind in the AST.
    ///
    /// The Cangjie grammar uses `alias($.identifier, $.xxxName)` patterns
    /// rather than `field("name", ...)`, so we find the name by matching
    /// child node kinds instead of using `child_by_field_name`.
    fn name_child_kind(self) -> Option<&'static str> {
        match self {
            Self::Class => Some("className"),
            Self::Interface => Some("interfaceName"),
            Self::Function => Some("funcName"),
            Self::Enum => Some("enumName"),
            Self::Struct => Some("structName"),
            Self::Extend => Some("extendType"),
            Self::TypeAlias => Some("typeAliasName"),
        }
    }
}

/// Extract the name text from a declaration node by finding its name child.
fn extract_name(node: Node<'_>, source: &str, kind: CangjieSymbolKind) -> String {
    let name_kind = match kind.name_child_kind() {
        Some(k) => k,
        None => return "<anonymous>".to_string(),
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == name_kind {
            let range = child.byte_range();
            return source
                .get(range)
                .unwrap_or("<anonymous>")
                .to_string();
        }
    }
    "<anonymous>".to_string()
}

fn collect_symbols(
    node: Node<'_>,
    source: &str,
    out: &mut Vec<(ByteRange, CangjieSymbolKind, String)>,
) {
    if let Some(kind) = CangjieSymbolKind::from_node_kind(node.kind()) {
        let name = extract_name(node, source, kind);
        out.push((node.start_byte()..node.end_byte(), kind, name));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbols(child, source, out);
    }
}

// Prefer narrower nodes for overlap (method inside class wins over class).
fn pick_non_overlapping(
    mut sorted_by_len: Vec<(ByteRange, CangjieSymbolKind, String)>,
) -> Vec<(ByteRange, CangjieSymbolKind, String)> {
    let mut chosen: Vec<(ByteRange, CangjieSymbolKind, String)> = Vec::new();
    'next: for (range, kind, name) in sorted_by_len.drain(..) {
        for (existing, _, _) in &chosen {
            if ranges_overlap(&range, existing) {
                continue 'next;
            }
        }
        chosen.push((range, kind, name));
    }
    chosen.sort_by_key(|(range, _, _)| range.start);
    chosen
}

fn ranges_overlap(a: &ByteRange, b: &ByteRange) -> bool {
    a.start < b.end && b.start < a.end
}

fn range_to_lsp(source: &str, range: &ByteRange) -> LspRange {
    LspRange {
        start: byte_offset_to_position(source, range.start),
        end: byte_offset_to_position(source, range.end),
    }
}

fn byte_offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut col_utf16 = 0u32;
    let mut i = 0usize;
    for ch in text.chars() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col_utf16 = 0;
        } else {
            col_utf16 += ch.len_utf16() as u32;
        }
        i += ch.len_utf8();
    }
    Position {
        line,
        character: col_utf16,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::document_chunker::chunker::Chunker;

    fn temp_cj_file(name: &str, source: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("cj_chunker_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(name);
        fs::write(&path, source).expect("write");
        (path, dir)
    }

    fn first_line(s: &str) -> &str {
        s.lines().next().unwrap_or("")
    }

    fn position_to_offset(text: &str, pos: Position) -> usize {
        let mut line = 0u32;
        let mut col_utf16 = 0u32;
        let mut offset = 0usize;
        for ch in text.chars() {
            if line == pos.line && col_utf16 >= pos.character {
                break;
            }
            if ch == '\n' {
                line += 1;
                col_utf16 = 0;
            } else {
                col_utf16 += ch.len_utf16() as u32;
            }
            offset += ch.len_utf8();
        }
        offset
    }

    #[tokio::test]
    async fn split_extracts_all_symbol_types() {
        let src = r#"package example

interface Serializable {
    func serialize(): String
}

enum Color {
    | Red
    | Green
    | Blue
}

struct Point {
    var x: Float64
    var y: Float64
}

class Calculator {
    public var value: Int64

    public init() {
        this.value = 0
    }

    public func add(n: Int64): Unit {
        this.value += n
    }
}

func add(a: Int64, b: Int64): Int64 {
    a + b
}

type IntAlias = Int64

extend String {
    func double(): String {
        this + this
    }
}
"#;
        let (path, dir) = temp_cj_file("sample.cj", src);
        let chunker = CangjieChunker::new().expect("CangjieChunker::new");
        let chunks = chunker.split(&path, "sample.cj").await.expect("split");

        assert!(!chunks.is_empty());

        let headers: Vec<&str> = chunks
            .iter()
            .map(|c| first_line(&c.embedding_content))
            .collect();

        // Interface body method (narrower than interface, so it's kept)
        assert!(
            headers.iter().any(|h| *h == "[function] serialize"),
            "should have serialize function, got: {:?}",
            headers
        );

        // Enum should be present
        assert!(
            headers.iter().any(|h| *h == "[enum] Color"),
            "should have Color enum, got: {:?}",
            headers
        );

        // Struct should be present
        assert!(
            headers.iter().any(|h| *h == "[struct] Point"),
            "should have Point struct, got: {:?}",
            headers
        );

        // Class methods (narrower, so kept over class)
        assert!(
            headers.iter().any(|h| *h == "[function] add"),
            "should have add function, got: {:?}",
            headers
        );

        // Free function
        assert!(
            headers.iter().any(|h| *h == "[function] add"),
            "should have free add function, got: {:?}",
            headers
        );

        // Type alias
        assert!(
            headers.iter().any(|h| *h == "[type] IntAlias"),
            "should have IntAlias type, got: {:?}",
            headers
        );

        // Extend body function (narrower than extend, so it's kept instead of extend)
        assert!(
            headers.iter().any(|h| *h == "[function] double"),
            "should have double function from extend, got: {:?}",
            headers
        );

        // Verify content matches source by range
        for chunk in &chunks {
            let content = chunk.info.content.as_ref().expect("content should be set");
            let range = chunk.info.range.as_ref().expect("range should be set");
            assert!(!content.is_empty());

            let start = position_to_offset(src, range.start);
            let end = position_to_offset(src, range.end);
            assert!(
                start <= end && end <= src.len(),
                "invalid range: {:?}",
                range
            );

            let slice = &src[start..end];
            assert_eq!(
                slice,
                content,
                "content should match source slice by range for {:?}",
                first_line(&chunk.embedding_content)
            );
        }

        fs::remove_dir_all(&dir).ok();
    }
}
