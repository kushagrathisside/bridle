//! Syntax-tree code intelligence over tree-sitter (ADR-0003). Pure functions over text: no I/O,
//! no process, no policy. The MCP tools in `mcp::tools::code` add the file access and the checks.
//!
//! Results are syntactic. `identifier_lines` finds identifiers with a given spelling outside
//! comments and strings; it does not resolve scopes, overloads or imports.

pub mod deps;

use serde::Serialize;
use tree_sitter::{Node, Parser, Tree};

/// A language the code tools can parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
    /// C++, also used for `.c` and `.h` files (its grammar parses plain C for symbol lookup).
    Cpp,
}

impl Language {
    /// The language for a file name, by extension. `None` for anything the tools do not parse.
    pub fn from_path(path: &str) -> Option<Self> {
        let name = path.rsplit(['/', '\\']).next()?;
        let (_, ext) = name.rsplit_once('.')?;
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "py" | "pyi" => Some(Self::Python),
            "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "c" | "h" => Some(Self::Cpp),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Cpp => "cpp",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        }
    }
}

/// A named definition found in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Symbol {
    pub name: String,
    /// `function`, `struct`, `class`, `enum`, `trait`, `module`, `namespace`, `type`, `const`,
    /// `static` or `union`.
    pub kind: &'static str,
    /// 1-based line of the definition.
    pub line: u32,
}

/// An import or include directive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Import {
    pub line: u32,
    pub text: String,
}

/// Parses `source`. `None` when the grammar cannot be loaded or parsing fails.
pub fn parse(language: Language, source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&language.grammar()).ok()?;
    parser.parse(source, None)
}

/// Every node of the tree, depth-first. Explicit stack, so deep nesting cannot overflow.
fn all_nodes(tree: &Tree) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        out.push(node);
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    out
}

/// The definition kind for a node, if the node defines a named symbol in `language`.
/// C++ classes, structs, enums and unions count only with a body, so forward declarations and
/// elaborated type uses are not reported as definitions.
fn definition_kind(language: Language, node: Node<'_>) -> Option<&'static str> {
    let kind = match (language, node.kind()) {
        (Language::Rust, "function_item") => "function",
        (Language::Rust, "struct_item") => "struct",
        (Language::Rust, "enum_item") => "enum",
        (Language::Rust, "union_item") => "union",
        (Language::Rust, "trait_item") => "trait",
        (Language::Rust, "mod_item") => "module",
        (Language::Rust, "type_item") => "type",
        (Language::Rust, "const_item") => "const",
        (Language::Rust, "static_item") => "static",
        (Language::Python, "function_definition") => "function",
        (Language::Python, "class_definition") => "class",
        (Language::Cpp, "function_definition") => "function",
        (Language::Cpp, "namespace_definition") => "namespace",
        (
            Language::Cpp,
            "class_specifier" | "struct_specifier" | "union_specifier" | "enum_specifier",
        ) => {
            node.child_by_field_name("body")?;
            match node.kind() {
                "class_specifier" => "class",
                "struct_specifier" => "struct",
                "union_specifier" => "union",
                _ => "enum",
            }
        }
        _ => return None,
    };
    Some(kind)
}

/// Identifier kinds that can name something, in every grammar used here.
const NAME_KINDS: &[&str] = &[
    "identifier",
    "field_identifier",
    "type_identifier",
    "property_identifier",
    "namespace_identifier",
    "qualified_identifier",
    "destructor_name",
    "operator_name",
];

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

/// The name of a C++ function: the first name inside its declarator (`Foo::bar`, `operator+`).
fn cpp_function_name<'a>(node: Node<'a>, source: &str) -> Option<String> {
    let declarator = node.child_by_field_name("declarator")?;
    let mut stack = vec![declarator];
    while let Some(current) = stack.pop() {
        if NAME_KINDS.contains(&current.kind()) {
            return Some(text(current, source).to_owned());
        }
        let mut cursor = current.walk();
        let children: Vec<Node<'a>> = current.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

/// The definitions in a parsed source file, in source order.
pub fn definitions(language: Language, tree: &Tree, source: &str) -> Vec<Symbol> {
    let mut found: Vec<Symbol> = all_nodes(tree)
        .into_iter()
        .filter_map(|node| {
            let kind = definition_kind(language, node)?;
            let name = if language == Language::Cpp && kind == "function" {
                cpp_function_name(node, source)?
            } else {
                text(node.child_by_field_name("name")?, source).to_owned()
            };
            (!name.is_empty()).then(|| Symbol {
                name,
                kind,
                line: u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX),
            })
        })
        .collect();
    found.sort_by_key(|symbol| symbol.line);
    found
}

/// The 1-based lines where an identifier spelled `name` occurs (outside comments and strings,
/// which the grammar gives different node kinds).
pub fn identifier_lines(tree: &Tree, source: &str, name: &str) -> Vec<u32> {
    let mut lines: Vec<u32> = all_nodes(tree)
        .into_iter()
        .filter(|node| {
            node.kind() == "identifier"
                || node.kind() == "field_identifier"
                || node.kind() == "type_identifier"
                || node.kind() == "property_identifier"
                || node.kind() == "namespace_identifier"
        })
        .filter(|node| text(*node, source) == name)
        .map(|node| u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX))
        .collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// The import and include directives, in source order.
pub fn imports(language: Language, tree: &Tree, source: &str) -> Vec<Import> {
    let wanted: &[&str] = match language {
        Language::Rust => &["use_declaration"],
        Language::Python => &["import_statement", "import_from_statement"],
        Language::Cpp => &["preproc_include"],
    };
    let mut found: Vec<Import> = all_nodes(tree)
        .into_iter()
        .filter(|node| wanted.contains(&node.kind()))
        .map(|node| Import {
            line: u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX),
            text: text(node, source)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        })
        .collect();
    found.sort_by_key(|import| import.line);
    found
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tree(language: Language, source: &str) -> Tree {
        parse(language, source).expect("parses")
    }

    #[test]
    fn languages_come_from_extensions() {
        assert_eq!(Language::from_path("src/main.rs"), Some(Language::Rust));
        assert_eq!(Language::from_path("pkg/mod.PYI"), Some(Language::Python));
        assert_eq!(
            Language::from_path("include\\calc.hpp"),
            Some(Language::Cpp)
        );
        assert_eq!(Language::from_path("README.md"), None);
        assert_eq!(Language::from_path("Makefile"), None);
        assert_eq!(Language::from_path("dir.d/file"), None);
    }

    #[test]
    fn rust_definitions_have_names_kinds_and_lines() {
        let src = "pub struct Calc { x: i32 }\n\nfn add(a: i32, b: i32) -> i32 { a + b }\n\nimpl Calc {\n    fn new() -> Self { Calc { x: 0 } }\n}\n";
        let symbols = definitions(Language::Rust, &tree(Language::Rust, src), src);
        let names: Vec<(&str, &str, u32)> = symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            names,
            [
                ("Calc", "struct", 1),
                ("add", "function", 3),
                ("new", "function", 6)
            ]
        );
    }

    #[test]
    fn python_classes_and_methods_are_found() {
        let src = "class Calc:\n    def add(self, a, b):\n        return a + b\n\ndef main():\n    pass\n";
        let symbols = definitions(Language::Python, &tree(Language::Python, src), src);
        let names: Vec<(&str, &str)> = symbols.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(
            names,
            [("Calc", "class"), ("add", "function"), ("main", "function")]
        );
    }

    #[test]
    fn cpp_qualified_definitions_and_forward_declarations() {
        let src = "class Calc;\nclass Calc {\n public:\n  int add(int a, int b);\n};\nint Calc::add(int a, int b) { return a + b; }\n";
        let symbols = definitions(Language::Cpp, &tree(Language::Cpp, src), src);
        let names: Vec<(&str, &str)> = symbols.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(names, [("Calc", "class"), ("Calc::add", "function")]);
    }

    #[test]
    fn references_skip_comments_and_strings() {
        let src = "def add(a, b):\n    # add is the function\n    return a + b\n\nprint(\"add\")\nadd(1, 2)\n";
        let lines = identifier_lines(&tree(Language::Python, src), src, "add");
        assert_eq!(lines, [1, 6]);
    }

    #[test]
    fn imports_are_listed_per_language() {
        let rust = "use std::fs;\nfn main() {}\n";
        assert_eq!(
            imports(Language::Rust, &tree(Language::Rust, rust), rust),
            [Import {
                line: 1,
                text: "use std::fs;".to_owned()
            }]
        );
        let py = "import os\nfrom a import b\n";
        assert_eq!(
            imports(Language::Python, &tree(Language::Python, py), py).len(),
            2
        );
        let cpp = "#include <vector>\nint x;\n";
        assert_eq!(
            imports(Language::Cpp, &tree(Language::Cpp, cpp), cpp)[0].line,
            1
        );
    }

    #[test]
    fn broken_source_still_yields_what_parses() {
        let src = "fn good() {}\nfn broken( {\n";
        let symbols = definitions(Language::Rust, &tree(Language::Rust, src), src);
        assert!(symbols.iter().any(|s| s.name == "good"), "{symbols:?}");
    }
}
