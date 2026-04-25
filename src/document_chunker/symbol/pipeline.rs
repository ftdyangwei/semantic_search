//! Shared tree-sitter walk → symbol list → [`Chunk`] pipeline.

use lsp_types::{Position, Range as LspRange};
use tree_sitter::{Node, Tree};

use crate::common::data::{Chunk, ChunkInfo, IndexType};

use super::kind::SymbolKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEntry {
    pub range: LspRange,
    pub kind: SymbolKind,
    pub name: String,
}

pub struct SymbolPipeline {
    lang_id: String,
}

impl SymbolPipeline {
    pub fn new(lang_id: impl Into<String>) -> Self {
        Self {
            lang_id: lang_id.into(),
        }
    }

    pub fn split_file_to_chunks(
        &self,
        tree: &Tree,
        source: &str,
        relative_path: &str,
        map_kind: impl Fn(&str) -> Option<SymbolKind> + Copy,
    ) -> Vec<Chunk> {
        let mut symbols = Vec::new();
        self.walk_collect_symbols(tree.root_node(), source, &mut symbols, map_kind);
        let symbols = self.finalize_symbols(symbols);
        self.symbols_to_chunks(symbols, source, relative_path)
    }

    fn symbols_to_chunks(
        &self,
        symbols: Vec<SymbolEntry>,
        source: &str,
        relative_path: &str,
    ) -> Vec<Chunk> {
        let chunk_count = symbols.len();
        let mut chunks = Vec::with_capacity(chunk_count);
        for (idx, symbol) in symbols.into_iter().enumerate() {
            let embedding_content = symbol.name.clone();
            chunks.push(Chunk {
                embedding_content,
                info: ChunkInfo {
                    layer: IndexType::Symbol,
                    lang: self.lang_id.clone(),
                    file_path: relative_path.to_string(),
                    content: None,
                    range: Some(symbol.range),
                },
                is_last: idx + 1 == chunk_count,
                ..Default::default()
            });
        }
        chunks
    }

    fn finalize_symbols(&self, mut symbols: Vec<SymbolEntry>) -> Vec<SymbolEntry> {
        symbols.retain(|symbol| !is_empty_range(&symbol.range));
        symbols.sort_by_key(|symbol| {
            (
                symbol.range.start.line,
                symbol.range.start.character,
                symbol.range.end.line,
                symbol.range.end.character,
            )
        });
        symbols
    }

    fn walk_collect_symbols(
        &self,
        node: Node<'_>,
        source: &str,
        out: &mut Vec<SymbolEntry>,
        map_kind: impl Fn(&str) -> Option<SymbolKind> + Copy,
    ) {
        let _ = &self.lang_id;
        if let Some(kind) = map_kind(node.kind()) {
            out.push(SymbolEntry {
                range: node_to_lsp_range(node),
                kind,
                name: node_display_name(node, source),
            });
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk_collect_symbols(child, source, out, map_kind);
        }
    }
}

/// Maps a [`tree_sitter::Point`] to LSP [`Position`].
///
/// `Point::row` is 0-based (same as LSP `line`).  
/// `Point::column` in tree-sitter is a **byte offset on that line**, while LSP `character` is
/// UTF-16 code units — for strict LSP compliance you may still need to convert the line slice;
/// here we store tree-sitter’s column as `character` to avoid a full-buffer scan at chunk time.
#[inline]
pub fn point_to_position(p: tree_sitter::Point) -> Position {
    Position {
        line: p.row as u32,
        character: p.column as u32,
    }
}

#[inline]
pub fn node_to_lsp_range(node: Node<'_>) -> LspRange {
    LspRange {
        start: point_to_position(node.start_position()),
        end: point_to_position(node.end_position()),
    }
}

/// Try `name` field, then the first `identifier` child (ArkTS / some grammars).
pub fn node_display_name(node: Node<'_>, source: &str) -> String {
    if let Some(n) = node.child_by_field_name("name") {
        let s = source[n.byte_range()].trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    // Many grammars (notably C/C++) nest the symbol name under `declarator`.
    if let Some(d) = node.child_by_field_name("declarator") {
        if let Some(s) = first_identifier_in_subtree(d, source) {
            return s;
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let s = source[child.byte_range()].trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    if let Some(s) = first_identifier_in_subtree(node, source) {
        return s;
    }
    "<anonymous>".to_string()
}

fn first_identifier_in_subtree(node: Node<'_>, source: &str) -> Option<String> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier" {
            let s = source[n.byte_range()].trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
        let mut cursor = n.walk();
        // DFS: push children in reverse so earlier children are visited first.
        let children: Vec<_> = n.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    None
}

fn is_empty_range(range: &LspRange) -> bool {
    range.start.line == range.end.line && range.start.character == range.end.character
}

fn slice_by_lsp_range<'a>(source: &'a str, range: &LspRange) -> &'a str {
    let start = position_to_byte_offset(source, range.start);
    let end = position_to_byte_offset(source, range.end);
    source.get(start..end).unwrap_or("")
}

fn position_to_byte_offset(text: &str, pos: Position) -> usize {
    let line = pos.line as usize;
    let col = pos.character as usize;
    let mut line_no = 0usize;
    let mut i = 0usize;
    while line_no < line {
        let Some(rel) = text[i..].find('\n') else {
            return text.len();
        };
        i += rel + 1;
        line_no += 1;
    }
    let rest = text.get(i..).unwrap_or("");
    let line_len = rest.find('\n').unwrap_or(rest.len());
    let line_slice = rest.get(..line_len).unwrap_or("");
    i + col.min(line_slice.len())
}
