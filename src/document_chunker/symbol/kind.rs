//! Cross-language symbol categories for symbol-layer chunking.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Class,
    Interface,
    Function,
    Method,
    Enum,
    Struct,
}

impl SymbolKind {
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Function => "function",
            Self::Method => "method",
            Self::Enum => "enum",
            Self::Struct => "struct",
        }
    }

    /// Generic node-kind mapping shared by all tree-sitter grammars in this project.
    /// Languages only need to initialize the right parser; symbol extraction stays uniform.
    pub fn from_node_kind(kind: &str) -> Option<Self> {
        match kind {
            "class_declaration" | "abstract_class_declaration" => Some(Self::Class),
            // C++ tree-sitter grammar
            "class_specifier" => Some(Self::Class),
            "interface_declaration" => Some(Self::Interface),
            "function_declaration"
            | "generator_function_declaration"
            | "decorated_function_declaration" => Some(Self::Function),
            // C/C++ tree-sitter grammar: top-level + class-scope function bodies.
            "function_definition" => Some(Self::Function),
            "method_definition" | "method_declaration" => Some(Self::Method),
            "enum_declaration" => Some(Self::Enum),
            // C/C++ tree-sitter grammar
            "enum_specifier" => Some(Self::Enum),
            // TS uses type_alias_declaration; ArkTS uses type_declaration.
            // ArkTS component_declaration is still a `struct`-style symbol we want to keep.
            "type_alias_declaration" | "type_declaration" | "component_declaration" => {
                Some(Self::Struct)
            }
            // C/C++ tree-sitter grammar
            "struct_specifier" => Some(Self::Struct),
            _ => None,
        }
    }
}
