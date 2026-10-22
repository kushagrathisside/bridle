//! Code-intelligence tools (M2 step 5, brief §14 and §15): `repo_map`, `file_summary`,
//! `symbol_search` and `find_references`. The syntax analysis is `crate::code`; this module
//! decides which files are read.
//!
//! Only tracked files (`git ls-files`) are considered, and every one passes the read policy, so a
//! secret file is never parsed, listed or quoted. Files over [`MAX_FILE_BYTES`] and binary files
//! are skipped. Parsing is bounded by [`MAX_PARSED_FILES`] per call, and the result says when the
//! bound cut the search short.

use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::code::{Language, definitions, identifier_lines, imports, parse};

use super::{
    McpTool, ToolContext, ToolOutput, bool_arg, check_read_path, lossy, redacted_output, run_git,
    u64_arg,
};

/// Largest file parsed or read by the code tools.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Most files parsed by one search call.
const MAX_PARSED_FILES: usize = 2_000;
/// Longest name accepted as a query.
const MAX_NAME_LEN: usize = 200;
/// Longest source line quoted in a reference result.
const MAX_LINE_CHARS: usize = 200;
/// Directories listed by `repo_map`.
const MAP_DIRECTORIES: usize = 40;
/// Root files that tell a reader how a project is built and run.
const KEY_FILES: &[&str] = &[
    "Cargo.toml",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "package.json",
    "CMakeLists.txt",
    "Makefile",
    "go.mod",
    "README.md",
    "build.gradle",
];

/// The tracked files of the workspace that the read policy allows, and how many it excluded.
fn tracked_files(ctx: &ToolContext) -> Result<(Vec<String>, usize), String> {
    let raw = run_git(ctx, &["ls-files", "-z"], 16 * 1024 * 1024)?;
    let mut allowed = Vec::new();
    let mut excluded = 0usize;
    for name in raw
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(lossy)
    {
        if check_read_path(ctx, &name).is_ok() {
            allowed.push(name);
        } else {
            excluded += 1;
        }
    }
    Ok((allowed, excluded))
}

/// The text of one file, or why it was skipped.
fn read_source(ctx: &ToolContext, relative: &str) -> Result<String, String> {
    let path = ctx.workspace.join(relative);
    let size = std::fs::metadata(&path)
        .map_err(|error| error.to_string())?
        .len();
    if size > MAX_FILE_BYTES {
        return Err(format!("larger than {MAX_FILE_BYTES} bytes"));
    }
    let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    if bytes.contains(&0) {
        return Err("binary".to_owned());
    }
    Ok(lossy(&bytes))
}

/// A symbol or identifier query: letters, digits, `_` and `::` (qualified names), bounded.
fn name_arg(args: &Value, key: &str) -> Result<String, String> {
    let name = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` must be a string"))?
        .trim()
        .to_owned();
    let valid = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':');
    if valid {
        Ok(name)
    } else {
        Err(format!(
            "`{key}` must be an identifier (letters, digits, _ and ::), at most {MAX_NAME_LEN} characters"
        ))
    }
}

/// The last segment of a qualified name: `Calc::add` gives `add`.
fn short_name(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

/// `repo_map`: the layout of the tracked files: directories, languages, and key build files.
#[derive(Debug)]
pub struct RepoMapTool;

impl McpTool for RepoMapTool {
    fn name(&self) -> &'static str {
        "repo_map"
    }
    fn description(&self) -> &'static str {
        "Map the tracked files: busiest directories, languages with parseable source, and the key build files. Secret files are never listed."
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }
    fn call(&self, _args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let (files, excluded) = tracked_files(ctx)?;
        let mut directories: BTreeMap<String, usize> = BTreeMap::new();
        let mut languages: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut key_files = Vec::new();
        for file in &files {
            let dir = match file.rfind(['/', '\\']) {
                Some(index) => &file[..index],
                None => ".",
            };
            *directories.entry(dir.to_owned()).or_default() += 1;
            if let Some(language) = Language::from_path(file) {
                *languages.entry(language.name()).or_default() += 1;
            }
            if KEY_FILES.contains(&file.as_str()) {
                key_files.push(file.clone());
            }
        }
        let mut busiest: Vec<(String, usize)> = directories.into_iter().collect();
        busiest.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        busiest.truncate(MAP_DIRECTORIES);

        let mut text = format!("{} tracked file(s).", files.len());
        if excluded > 0 {
            text.push_str(&format!(" {excluded} excluded by policy (not listed)."));
        }
        text.push_str("\nLanguages: ");
        text.push_str(
            &languages
                .iter()
                .map(|(name, count)| format!("{name} {count}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        if !key_files.is_empty() {
            text.push_str(&format!("\nKey files: {}", key_files.join(", ")));
        }
        text.push_str("\nDirectories:");
        for (dir, count) in &busiest {
            text.push_str(&format!("\n  {dir} ({count})"));
        }
        let structured = json!({
            "files": files.len(),
            "excluded": excluded,
            "languages": languages,
            "key_files": key_files,
            "directories": busiest
                .iter()
                .map(|(path, count)| json!({"path": path, "files": count}))
                .collect::<Vec<_>>(),
        });
        Ok(redacted_output(ctx, &text, Some(structured)))
    }
}

/// `file_summary`: language, size, the definitions and the imports of one file.
#[derive(Debug)]
pub struct FileSummaryTool;

impl McpTool for FileSummaryTool {
    fn name(&self) -> &'static str {
        "file_summary"
    }
    fn description(&self) -> &'static str {
        "Summarise one source file: language, line count, definitions with their lines, and imports."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "File path inside the workspace."}},
            "required": ["path"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .ok_or("`path` must be a non-empty string")?;
        check_read_path(ctx, path)?;
        let source = read_source(ctx, path)?;
        let language = Language::from_path(path);
        let (symbols, imports_found) =
            match language.and_then(|lang| parse(lang, &source).map(|t| (lang, t))) {
                Some((lang, tree)) => (
                    definitions(lang, &tree, &source),
                    imports(lang, &tree, &source),
                ),
                None => (Vec::new(), Vec::new()),
            };
        let lines = source.lines().count();

        let mut text = format!(
            "{path}: {} language, {lines} line(s), {} definition(s), {} import(s).",
            language.map_or("unparsed", Language::name),
            symbols.len(),
            imports_found.len()
        );
        for symbol in &symbols {
            text.push_str(&format!(
                "\n  {}:{} {}",
                symbol.line, symbol.kind, symbol.name
            ));
        }
        let structured = json!({
            "path": path,
            "language": language.map(Language::name),
            "lines": lines,
            "bytes": source.len(),
            "definitions": symbols,
            "imports": imports_found,
        });
        Ok(redacted_output(ctx, &text, Some(structured)))
    }
}

/// `symbol_search`: where a symbol is defined, across the tracked source files.
#[derive(Debug)]
pub struct SymbolSearchTool;

impl McpTool for SymbolSearchTool {
    fn name(&self) -> &'static str {
        "symbol_search"
    }
    fn description(&self) -> &'static str {
        "Find where a function, class, struct, type or module is defined. Match exact, prefix or contains."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Symbol name, e.g. add or Calc::add."},
                "match": {"type": "string", "enum": ["exact", "prefix", "contains"], "default": "prefix"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 200, "default": 50},
            },
            "required": ["name"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let name = name_arg(args, "name")?;
        let mode = args
            .get("match")
            .and_then(Value::as_str)
            .unwrap_or("prefix");
        if !matches!(mode, "exact" | "prefix" | "contains") {
            return Err("`match` must be exact, prefix or contains".to_owned());
        }
        let max = u64_arg(args, "max_results", 50)?.clamp(1, 200) as usize;
        let (files, _excluded) = tracked_files(ctx)?;
        let mut hits = Vec::new();
        let (mut parsed, mut skipped) = (0usize, 0usize);
        let mut truncated = false;

        for file in files.iter().filter(|f| Language::from_path(f).is_some()) {
            if parsed >= MAX_PARSED_FILES {
                truncated = true;
                break;
            }
            let Ok(source) = read_source(ctx, file) else {
                skipped += 1;
                continue;
            };
            // Cheap prefilter: a file without the query text cannot define the symbol.
            if !source.contains(short_name(&name)) {
                continue;
            }
            let Some(language) = Language::from_path(file) else {
                continue;
            };
            let Some(tree) = parse(language, &source) else {
                continue;
            };
            parsed += 1;
            for symbol in definitions(language, &tree, &source) {
                let short = short_name(&symbol.name);
                let matched = match mode {
                    "exact" => symbol.name == name || short == name,
                    "contains" => symbol.name.contains(&name) || short.contains(&name),
                    _ => symbol.name.starts_with(&name) || short.starts_with(&name),
                };
                if matched {
                    hits.push(json!({
                        "file": file,
                        "line": symbol.line,
                        "kind": symbol.kind,
                        "name": symbol.name,
                    }));
                }
            }
        }

        let total = hits.len();
        hits.truncate(max);
        let mut text = format!("{total} definition(s) of `{name}` ({mode}).");
        for hit in &hits {
            text.push_str(&format!(
                "\n  {}:{} {} {}",
                hit["file"].as_str().unwrap_or_default(),
                hit["line"],
                hit["kind"].as_str().unwrap_or_default(),
                hit["name"].as_str().unwrap_or_default(),
            ));
        }
        if truncated {
            text.push_str(&format!(
                "\n[searched the first {MAX_PARSED_FILES} source files; narrow the query or the paths]"
            ));
        }
        let structured = json!({
            "name": name,
            "match": mode,
            "count": total,
            "results": hits,
            "parsed_files": parsed,
            "skipped_files": skipped,
            "truncated": truncated,
        });
        Ok(redacted_output(ctx, &text, Some(structured)))
    }
}

/// `find_references`: the lines where an identifier is used, found syntactically (no comments,
/// no strings, but no scope or type resolution).
#[derive(Debug)]
pub struct FindReferencesTool;

impl McpTool for FindReferencesTool {
    fn name(&self) -> &'static str {
        "find_references"
    }
    fn description(&self) -> &'static str {
        "List the lines that use an identifier (syntactic: comments and strings excluded; no scope or type resolution)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Identifier, e.g. add. A qualified name uses its last part."},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 500, "default": 100},
                "include_definitions": {"type": "boolean", "default": true},
            },
            "required": ["name"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let name = name_arg(args, "name")?;
        let needle = short_name(&name).to_owned();
        let max = u64_arg(args, "max_results", 100)?.clamp(1, 500) as usize;
        let include_definitions = bool_arg(args, "include_definitions", true)?;
        let (files, _excluded) = tracked_files(ctx)?;
        let mut refs = Vec::new();
        let (mut parsed, mut skipped) = (0usize, 0usize);
        let mut truncated = false;

        for file in files.iter().filter(|f| Language::from_path(f).is_some()) {
            if parsed >= MAX_PARSED_FILES {
                truncated = true;
                break;
            }
            let Ok(source) = read_source(ctx, file) else {
                skipped += 1;
                continue;
            };
            if !source.contains(&needle) {
                continue;
            }
            let Some(language) = Language::from_path(file) else {
                continue;
            };
            let Some(tree) = parse(language, &source) else {
                continue;
            };
            parsed += 1;
            let defined: Vec<u32> = if include_definitions {
                Vec::new()
            } else {
                definitions(language, &tree, &source)
                    .into_iter()
                    .map(|symbol| symbol.line)
                    .collect()
            };
            for line in identifier_lines(&tree, &source, &needle) {
                if defined.contains(&line) {
                    continue;
                }
                let quoted: String = source
                    .lines()
                    .nth(line as usize - 1)
                    .unwrap_or_default()
                    .trim()
                    .chars()
                    .take(MAX_LINE_CHARS)
                    .collect();
                refs.push(json!({"file": file, "line": line, "text": quoted}));
            }
        }

        let total = refs.len();
        refs.truncate(max);
        let mut text = format!(
            "{total} reference(s) to `{needle}` (syntactic: comments and strings excluded)."
        );
        for item in &refs {
            text.push_str(&format!(
                "\n  {}:{}  {}",
                item["file"].as_str().unwrap_or_default(),
                item["line"],
                item["text"].as_str().unwrap_or_default(),
            ));
        }
        if truncated {
            text.push_str(&format!(
                "\n[searched the first {MAX_PARSED_FILES} source files; narrow the paths]"
            ));
        }
        let structured = json!({
            "name": needle,
            "count": total,
            "results": refs,
            "parsed_files": parsed,
            "skipped_files": skipped,
            "truncated": truncated,
        });
        Ok(redacted_output(ctx, &text, Some(structured)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::test_context;
    use super::super::test_support::{commit_files, init_repo};
    use super::*;

    const CALC: &str = "class Calc:\n    def add(self, a, b):\n        # add is documented here\n        return a + b\n\ndef main():\n    print(Calc().add(1, 2))\n";

    fn repo_with(files: &[(&str, &str)]) -> Option<(tempfile::TempDir, ToolContext)> {
        let dir = init_repo()?;
        commit_files(dir.path(), files);
        let ctx = test_context(dir.path());
        Some((dir, ctx))
    }

    #[test]
    fn symbol_search_finds_definitions_by_match_mode() {
        let Some((_dir, ctx)) = repo_with(&[("calc.py", CALC), ("notes.txt", "add add add")])
        else {
            return;
        };
        let exact = SymbolSearchTool
            .call(&json!({"name": "add", "match": "exact"}), &ctx)
            .unwrap();
        let s = exact.structured.unwrap();
        assert_eq!(s["count"], json!(1), "{s}");
        assert_eq!(s["results"][0]["line"], json!(2));
        assert_eq!(s["results"][0]["file"], json!("calc.py"));
        let prefix = SymbolSearchTool.call(&json!({"name": "Ca"}), &ctx).unwrap();
        assert_eq!(prefix.structured.unwrap()["count"], json!(1));
        let qualified = SymbolSearchTool
            .call(&json!({"name": "Calc::add", "match": "exact"}), &ctx)
            .unwrap();
        assert_eq!(qualified.structured.unwrap()["count"], json!(0));
    }

    #[test]
    fn find_references_skips_comments_and_strings() {
        let Some((_dir, ctx)) = repo_with(&[("calc.py", CALC)]) else {
            return;
        };
        let out = FindReferencesTool
            .call(&json!({"name": "add", "include_definitions": false}), &ctx)
            .unwrap();
        let s = out.structured.unwrap();
        let lines: Vec<u64> = s["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["line"].as_u64().unwrap())
            .collect();
        assert_eq!(lines, [7], "{s}");
        assert!(out.text.contains("syntactic"));
    }

    #[test]
    fn secret_files_are_never_parsed_listed_or_searched() {
        let secret = "def add(a, b):\n    return a + b\n";
        let Some((_dir, ctx)) = repo_with(&[(".env", secret), ("calc.py", CALC)]) else {
            return;
        };
        let map = RepoMapTool
            .call(&json!({}), &ctx)
            .unwrap()
            .structured
            .unwrap();
        assert_eq!(map["excluded"], json!(1));
        assert!(!map.to_string().contains(".env"), "{map}");
        let search = SymbolSearchTool
            .call(&json!({"name": "add", "match": "exact"}), &ctx)
            .unwrap()
            .structured
            .unwrap();
        assert!(!search.to_string().contains(".env"), "{search}");
        assert!(
            FileSummaryTool
                .call(&json!({"path": ".env"}), &ctx)
                .is_err()
        );
    }

    #[test]
    fn file_summary_lists_definitions_and_imports() {
        let src = "import os\n\ndef add(a, b):\n    return a + b\n";
        let Some((_dir, ctx)) = repo_with(&[("calc.py", src)]) else {
            return;
        };
        let s = FileSummaryTool
            .call(&json!({"path": "calc.py"}), &ctx)
            .unwrap()
            .structured
            .unwrap();
        assert_eq!(s["language"], json!("python"));
        assert_eq!(s["definitions"][0]["name"], json!("add"));
        assert_eq!(s["imports"][0]["text"], json!("import os"));
    }

    #[test]
    fn repo_map_summarises_layout_and_key_files() {
        let Some((_dir, ctx)) = repo_with(&[
            ("Cargo.toml", "[package]\n"),
            ("src/lib.rs", "pub fn x() {}\n"),
            ("src/main.rs", "fn main() {}\n"),
        ]) else {
            return;
        };
        let s = RepoMapTool
            .call(&json!({}), &ctx)
            .unwrap()
            .structured
            .unwrap();
        // init_repo commits README.md too.
        assert_eq!(s["files"], json!(4));
        assert_eq!(s["languages"]["rust"], json!(2));
        assert_eq!(s["key_files"], json!(["Cargo.toml", "README.md"]));
        // `.` and `src` tie at two files; ties sort by name.
        assert_eq!(s["directories"][0]["path"], json!("."));
        assert_eq!(s["directories"][1], json!({"path": "src", "files": 2}));
    }

    #[test]
    fn names_are_validated_before_use() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path());
        for bad in ["", "a b", "x;rm", "$HOME", "*", "a/b"] {
            assert!(
                SymbolSearchTool.call(&json!({"name": bad}), &ctx).is_err(),
                "{bad}"
            );
        }
        assert!(
            FileSummaryTool
                .call(&json!({"path": "../x.py"}), &ctx)
                .is_err()
        );
    }
}
