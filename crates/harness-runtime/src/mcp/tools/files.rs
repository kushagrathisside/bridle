//! File tools for the chat agent: `read_file`, `list_files`, `search_text`, `write_file` and
//! `edit_file`.
//!
//! These are not part of [`super::default_tools`]. The coding host (OpenCode) brings its own file
//! tools and the hook polices them; serving these over MCP as well would only duplicate them. They
//! exist for the standalone chat agent, which has no host to supply file access.
//!
//! Every call is checked by the same [`ToolPolicy`](harness_core::security::tool_policy::ToolPolicy)
//! the hook uses (workspace containment, symlink escape, secret files, protected control files), and
//! the tool then acts on the path the policy *resolved*, never on the string the caller gave, so
//! what was checked is what is opened. Walks never follow symlinks and never list or read secret
//! files.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use harness_core::config::Verdict;
use harness_core::security::redact::is_secret_file;
use harness_core::security::tool_policy::{ToolKind, ToolRequest};
use serde_json::{Value, json};

use super::{McpTool, ToolContext, ToolOutput, bool_arg, redacted_output, u64_arg};
use crate::state::atomic_write;

/// Largest file `read_file` and `edit_file` will load.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Lines `read_file` returns by default and at most.
const DEFAULT_LINES: u64 = 400;
const MAX_LINES: u64 = 2000;
/// Longer lines are cut in `read_file` output.
const MAX_LINE_CHARS: usize = 2000;
/// Largest content `write_file` accepts.
const MAX_WRITE_BYTES: usize = 1024 * 1024;
/// Files `search_text` looks into at most, and the size above which a file is skipped.
const MAX_SEARCH_FILES: usize = 3000;
const MAX_SEARCH_FILE_BYTES: u64 = 512 * 1024;
/// Characters of a matching line shown by `search_text`.
const MAX_MATCH_CHARS: usize = 200;
/// Directories no walk enters: version control, harness state and build or dependency output.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".local-agent",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
];

/// The five file tools, in the order they are offered.
pub fn file_tools() -> Vec<Box<dyn McpTool>> {
    vec![
        Box::new(ReadFileTool),
        Box::new(ListFilesTool),
        Box::new(SearchTextTool),
        Box::new(WriteFileTool),
        Box::new(EditFileTool),
    ]
}

/// Asks the policy about `path` for `kind`; on Allow returns the resolved path to act on.
fn gate(ctx: &ToolContext, kind: ToolKind, path: &str) -> Result<PathBuf, String> {
    let request = ToolRequest {
        paths: vec![path.to_owned()],
        ..ToolRequest::new(kind)
    };
    let decision = ctx.policy.evaluate(&request);
    if decision.verdict != Verdict::Allow {
        return Err(format!(
            "blocked by policy ({}): {}",
            decision.rule,
            decision.reasons.join("; ")
        ));
    }
    ctx.policy
        .resolve(path)
        .ok_or_else(|| format!("path could not be resolved inside the workspace: {path}"))
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` must be a string"))
}

fn optional_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

/// Replaces the file with `bytes` atomically: a reader, or a quit in the middle of the call, sees the old
/// content or the new, never half of it. The permissions of the file it replaces are kept.
fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
    atomic_write(path, bytes).map_err(|error| error.to_string())?;
    if let Some(permissions) = permissions {
        let _ = fs::set_permissions(path, permissions);
    }
    Ok(())
}

fn to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Reads at most `limit` bytes of `path`. The flag says whether more was left in the file.
fn read_capped(path: &Path, limit: u64) -> Result<(Vec<u8>, bool), String> {
    let file = fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let truncated = u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit;
    if truncated {
        bytes.truncate(to_usize(limit));
    }
    Ok((bytes, truncated))
}

/// A NUL byte in the first 8 KiB is taken as a binary file.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

/// `path` relative to the workspace for display, with forward slashes.
fn display_path(ctx: &ToolContext, path: &Path) -> String {
    let relative = path
        .strip_prefix(ctx.policy.workspace())
        .or_else(|_| path.strip_prefix(&ctx.workspace))
        .unwrap_or(path);
    let text = relative.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        ".".to_owned()
    } else {
        text
    }
}

fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut cut: String = text.chars().take(max).collect();
        cut.push('…');
        cut
    }
}

// ---------------------------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct ReadFileTool;

impl McpTool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn description(&self) -> &'static str {
        "Read a text file in the workspace, with line numbers. Use offset and limit for long files."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path, relative to the workspace."},
                "offset": {"type": "integer", "minimum": 1, "description": "First line to show (1-based). Default 1."},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LINES, "description": "Lines to show. Default 400."},
            },
            "required": ["path"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let path = required_str(args, "path")?;
        let offset = to_usize(u64_arg(args, "offset", 1)?).max(1);
        let limit = to_usize(u64_arg(args, "limit", DEFAULT_LINES)?).clamp(1, to_usize(MAX_LINES));
        let resolved = gate(ctx, ToolKind::Read, path)?;
        let meta = fs::metadata(&resolved).map_err(|e| format!("cannot read {path}: {e}"))?;
        if !meta.is_file() {
            return Err(format!(
                "{path} is not a file; use list_files for a directory"
            ));
        }
        let (bytes, cut_at_limit) = read_capped(&resolved, MAX_FILE_BYTES)?;
        if looks_binary(&bytes) {
            return Err(format!("{path} looks like a binary file"));
        }
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();
        if total == 0 {
            return Ok(redacted_output(
                ctx,
                &format!("{path} is empty"),
                Some(json!({"path": path, "lines": 0})),
            ));
        }
        if offset > total {
            return Err(format!(
                "offset {offset} is past the end of {path} ({total} lines)"
            ));
        }
        let end = (offset - 1 + limit).min(total);
        let mut out = format!(
            "{path} (lines {offset}-{end} of {total}{})\n",
            if cut_at_limit {
                ", file larger than 2 MiB: only the start was read"
            } else {
                ""
            }
        );
        for (index, line) in lines[offset - 1..end].iter().enumerate() {
            out.push_str(&format!(
                "{:>5}  {}\n",
                offset + index,
                cut_chars(line, MAX_LINE_CHARS)
            ));
        }
        if end < total {
            out.push_str(&format!(
                "... {} more line(s); call again with offset={}\n",
                total - end,
                end + 1
            ));
        }
        Ok(redacted_output(
            ctx,
            &out,
            Some(json!({"path": path, "from": offset, "to": end, "total_lines": total})),
        ))
    }
}

// ---------------------------------------------------------------------------------------------
// list_files
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct ListFilesTool;

impl McpTool for ListFilesTool {
    fn name(&self) -> &'static str {
        "list_files"
    }
    fn description(&self) -> &'static str {
        "List files and folders under a directory (default: the workspace root). Skips .git, build \
         output and secret files."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory, relative to the workspace. Default: the root."},
                "depth": {"type": "integer", "minimum": 1, "maximum": 6, "description": "Levels to descend. Default 2."},
                "max_entries": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Entries to return at most. Default 200."},
            },
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let path = optional_str(args, "path")?.unwrap_or(".");
        let depth = to_usize(u64_arg(args, "depth", 2)?).clamp(1, 6);
        let max = to_usize(u64_arg(args, "max_entries", 200)?).clamp(1, 1000);
        let root = gate(ctx, ToolKind::List, path)?;
        if !root.is_dir() {
            return Err(format!(
                "{path} is not a directory; use read_file for a file"
            ));
        }
        let mut walk = ListWalk {
            lines: Vec::new(),
            max,
            max_depth: depth,
            hidden_secrets: 0,
            truncated: false,
        };
        walk.dir(&root, Path::new(""), 1);
        let mut out = format!(
            "{} ({} entr{}, depth {depth})\n",
            display_path(ctx, &root),
            walk.lines.len(),
            if walk.lines.len() == 1 { "y" } else { "ies" }
        );
        for line in &walk.lines {
            out.push_str(line);
            out.push('\n');
        }
        if walk.truncated {
            out.push_str("... more entries not shown; narrow `path` or raise `max_entries`\n");
        }
        if walk.hidden_secrets > 0 {
            out.push_str(&format!(
                "({} secret file(s) hidden)\n",
                walk.hidden_secrets
            ));
        }
        Ok(redacted_output(
            ctx,
            &out,
            Some(json!({"entries": walk.lines.len(), "truncated": walk.truncated})),
        ))
    }
}

struct ListWalk {
    lines: Vec<String>,
    max: usize,
    max_depth: usize,
    hidden_secrets: usize,
    truncated: bool,
}

impl ListWalk {
    /// Lists `dir`; `rel` is its path below the listing root (used for the secret check).
    fn dir(&mut self, dir: &Path, rel: &Path, depth: usize) {
        let Ok(read) = fs::read_dir(dir) else { return };
        let mut entries: Vec<(String, fs::FileType)> = read
            .flatten()
            .filter_map(|e| {
                Some((
                    e.file_name().to_string_lossy().into_owned(),
                    e.file_type().ok()?,
                ))
            })
            .collect();
        // Folders first, then by name.
        entries.sort_by(|a, b| (!a.1.is_dir(), &a.0).cmp(&(!b.1.is_dir(), &b.0)));
        for (name, kind) in entries {
            if self.lines.len() >= self.max {
                self.truncated = true;
                return;
            }
            let child_rel = rel.join(&name);
            if kind.is_dir() && SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            if is_secret_file(&child_rel) {
                self.hidden_secrets += 1;
                continue;
            }
            let indent = "  ".repeat(depth - 1);
            if kind.is_symlink() {
                // Never followed: it could lead out of the workspace.
                self.lines.push(format!("{indent}{name}@"));
            } else if kind.is_dir() {
                self.lines.push(format!("{indent}{name}/"));
                if depth < self.max_depth {
                    self.dir(&dir.join(&name), &child_rel, depth + 1);
                }
            } else {
                self.lines.push(format!("{indent}{name}"));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// search_text
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct SearchTextTool;

impl McpTool for SearchTextTool {
    fn name(&self) -> &'static str {
        "search_text"
    }
    fn description(&self) -> &'static str {
        "Find lines containing a piece of text in the workspace's text files (plain text, not a \
         regular expression). Skips .git, build output, binary and secret files."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Text to look for."},
                "path": {"type": "string", "description": "File or directory to search. Default: the workspace root."},
                "case_sensitive": {"type": "boolean", "description": "Default false."},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 200, "description": "Default 50."},
            },
            "required": ["pattern"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let pattern = required_str(args, "pattern")?;
        if pattern.is_empty() {
            return Err("`pattern` must not be empty".to_owned());
        }
        let case_sensitive = bool_arg(args, "case_sensitive", false)?;
        let max = to_usize(u64_arg(args, "max_results", 50)?).clamp(1, 200);
        let path = optional_str(args, "path")?.unwrap_or(".");
        let root = gate(ctx, ToolKind::Search, path)?;
        let needle = if case_sensitive {
            pattern.to_owned()
        } else {
            pattern.to_lowercase()
        };

        let mut files = Vec::new();
        if root.is_dir() {
            collect_files(&root, Path::new(""), &mut files);
        } else {
            files.push(root.clone());
        }
        let mut hits: Vec<String> = Vec::new();
        let mut searched = 0usize;
        let mut skipped = 0usize;
        let mut capped = false;
        'files: for file in files.iter().take(MAX_SEARCH_FILES) {
            let shown = display_path(ctx, file);
            if is_secret_file(Path::new(&shown)) {
                skipped += 1;
                continue;
            }
            let Ok(meta) = fs::metadata(file) else {
                continue;
            };
            if meta.len() > MAX_SEARCH_FILE_BYTES {
                skipped += 1;
                continue;
            }
            let Ok((bytes, _)) = read_capped(file, MAX_SEARCH_FILE_BYTES) else {
                continue;
            };
            if looks_binary(&bytes) {
                skipped += 1;
                continue;
            }
            searched += 1;
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                let found = if case_sensitive {
                    line.contains(&needle)
                } else {
                    line.to_lowercase().contains(&needle)
                };
                if found {
                    hits.push(format!(
                        "{shown}:{}: {}",
                        index + 1,
                        cut_chars(line.trim(), MAX_MATCH_CHARS)
                    ));
                    if hits.len() >= max {
                        capped = true;
                        break 'files;
                    }
                }
            }
        }
        if files.len() > MAX_SEARCH_FILES {
            capped = true;
        }
        let mut out = format!(
            "{} match(es) for `{pattern}` in {searched} file(s) searched{}\n",
            hits.len(),
            if skipped > 0 {
                format!(" ({skipped} skipped: binary, large or secret)")
            } else {
                String::new()
            }
        );
        for hit in &hits {
            out.push_str(hit);
            out.push('\n');
        }
        if capped {
            out.push_str("... stopped at the result or file limit; narrow `path` or `pattern`\n");
        }
        Ok(redacted_output(
            ctx,
            &out,
            Some(json!({"matches": hits.len(), "files_searched": searched, "capped": capped})),
        ))
    }
}

/// Files below `dir` in a stable order, skipping [`SKIP_DIRS`] and symlinks.
fn collect_files(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    if out.len() > MAX_SEARCH_FILES {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<(String, fs::FileType)> = read
        .flatten()
        .filter_map(|e| {
            Some((
                e.file_name().to_string_lossy().into_owned(),
                e.file_type().ok()?,
            ))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, kind) in entries {
        if kind.is_symlink() {
            continue;
        }
        let child = dir.join(&name);
        if kind.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                collect_files(&child, &rel.join(&name), out);
            }
        } else {
            out.push(child);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// write_file
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct WriteFileTool;

impl McpTool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn description(&self) -> &'static str {
        "Create a file, or replace a whole file, with the given text. Prefer edit_file for small \
         changes to an existing file."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path, relative to the workspace. Parent folders are created."},
                "content": {"type": "string", "description": "The complete new content of the file."},
            },
            "required": ["path", "content"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let path = required_str(args, "path")?;
        let content = required_str(args, "content")?;
        if content.len() > MAX_WRITE_BYTES {
            return Err(format!(
                "content is {} bytes; write_file accepts at most {MAX_WRITE_BYTES}",
                content.len()
            ));
        }
        let resolved = gate(ctx, ToolKind::Write, path)?;
        if resolved.is_dir() {
            return Err(format!("{path} is a directory"));
        }
        let existed = resolved.exists();
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create the folder for {path}: {e}"))?;
        }
        replace_file(&resolved, content.as_bytes())
            .map_err(|e| format!("cannot write {path}: {e}"))?;
        let text = format!(
            "{} {path} ({} bytes, {} line(s))",
            if existed { "replaced" } else { "created" },
            content.len(),
            content.lines().count()
        );
        Ok(redacted_output(
            ctx,
            &text,
            Some(json!({"path": path, "bytes": content.len(), "created": !existed})),
        ))
    }
}

// ---------------------------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct EditFileTool;

impl McpTool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }
    fn description(&self) -> &'static str {
        "Change a file by replacing exact text. old_string must match the file exactly (including \
         spaces and line breaks) and be unique unless replace_all is true. Read the file first."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path, relative to the workspace."},
                "old_string": {"type": "string", "description": "Exact text to replace. Include enough context to be unique."},
                "new_string": {"type": "string", "description": "Replacement text."},
                "replace_all": {"type": "boolean", "description": "Replace every occurrence. Default false."},
            },
            "required": ["path", "old_string", "new_string"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let path = required_str(args, "path")?;
        let old = required_str(args, "old_string")?;
        let new = required_str(args, "new_string")?;
        let replace_all = bool_arg(args, "replace_all", false)?;
        if old.is_empty() {
            return Err(
                "`old_string` must not be empty; use write_file to create a file".to_owned(),
            );
        }
        if old == new {
            return Err("`old_string` and `new_string` are the same; nothing to change".to_owned());
        }
        let resolved = gate(ctx, ToolKind::Edit, path)?;
        let meta = fs::metadata(&resolved).map_err(|e| format!("cannot read {path}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{path} is not a file"));
        }
        let (bytes, cut) = read_capped(&resolved, MAX_FILE_BYTES)?;
        if cut {
            return Err(format!(
                "{path} is larger than 2 MiB; edit_file will not rewrite it"
            ));
        }
        if looks_binary(&bytes) {
            return Err(format!("{path} looks like a binary file"));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{path} is not valid UTF-8; edit_file will not rewrite it"))?;
        let count = text.matches(old).count();
        if count == 0 {
            return Err(format!(
                "old_string was not found in {path}; read the file again and copy the text exactly"
            ));
        }
        if count > 1 && !replace_all {
            return Err(format!(
                "old_string appears {count} times in {path}; add surrounding lines to make it \
                 unique, or set replace_all"
            ));
        }
        let first_line = text
            .find(old)
            .map_or(1, |at| text[..at].matches('\n').count() + 1);
        let updated = if replace_all {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        };
        replace_file(&resolved, updated.as_bytes())
            .map_err(|e| format!("cannot write {path}: {e}"))?;
        let replaced = if replace_all { count } else { 1 };
        let out =
            format!("edited {path}: replaced {replaced} occurrence(s), first at line {first_line}");
        Ok(redacted_output(
            ctx,
            &out,
            Some(json!({"path": path, "replaced": replaced, "first_line": first_line})),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::test_context;
    use super::*;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(
            root.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        fs::write(root.join("src/deep/lib.rs"), "pub fn Hello() {}\n").unwrap();
        fs::write(root.join("README.md"), "# demo\nhello world\n").unwrap();
        fs::write(root.join(".env"), "API_KEY=sk-live-0123456789abcdefghij\n").unwrap();
        fs::write(root.join(".git/config"), "[core]\n").unwrap();
        fs::write(root.join("target/out.txt"), "hello from build\n").unwrap();
        fs::write(root.join("blob.bin"), [0u8, 1, 2, 3, 0, 0]).unwrap();
        fs::write(root.join("local-agent.toml"), "profile = \"local-only\"\n").unwrap();
        dir
    }

    fn run(tool: &dyn McpTool, ws: &Path, args: Value) -> Result<ToolOutput, String> {
        tool.call(&args, &test_context(ws))
    }

    #[test]
    fn read_file_numbers_lines_and_pages() {
        let ws = workspace();
        let out = run(&ReadFileTool, ws.path(), json!({"path": "src/main.rs"})).unwrap();
        assert!(out.text.contains("lines 1-3 of 3"), "{}", out.text);
        assert!(
            out.text.contains("    2      println!(\"hi\");"),
            "{}",
            out.text
        );

        let page = run(
            &ReadFileTool,
            ws.path(),
            json!({"path": "src/main.rs", "offset": 2, "limit": 1}),
        )
        .unwrap();
        assert!(page.text.contains("lines 2-2 of 3") && page.text.contains("offset=3"));
        let past = run(
            &ReadFileTool,
            ws.path(),
            json!({"path": "src/main.rs", "offset": 9}),
        );
        assert!(past.unwrap_err().contains("past the end"));
    }

    #[test]
    fn read_file_refuses_secrets_outside_paths_binaries_and_directories() {
        let ws = workspace();
        let denied = |p: &str| run(&ReadFileTool, ws.path(), json!({"path": p})).unwrap_err();
        assert!(denied(".env").contains("secret_file"), "secret file");
        assert!(
            denied("../outside.txt").contains("blocked by policy"),
            "dotdot"
        );
        assert!(
            denied("/etc/hostname").contains("blocked by policy"),
            "absolute"
        );
        assert!(denied("blob.bin").contains("binary"));
        assert!(denied("src").contains("not a file"));
        assert!(denied("missing.txt").contains("cannot read"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_workspace_is_not_followed() {
        let ws = workspace();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "top secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), ws.path().join("link")).unwrap();
        let err = run(&ReadFileTool, ws.path(), json!({"path": "link/secret.txt"})).unwrap_err();
        assert!(err.contains("blocked by policy"), "{err}");
        let listed = run(&ListFilesTool, ws.path(), json!({})).unwrap();
        assert!(listed.text.contains("link@"), "{}", listed.text);
        assert!(!listed.text.contains("secret.txt"));
    }

    #[test]
    fn list_files_hides_secrets_and_noise_and_marks_folders() {
        let ws = workspace();
        let out = run(&ListFilesTool, ws.path(), json!({})).unwrap();
        let text = out.text;
        assert!(
            text.contains("src/") && text.contains("  main.rs"),
            "{text}"
        );
        assert!(text.contains("README.md"));
        assert!(
            !text.contains(".env") && !text.contains(".git") && !text.contains("target"),
            "{text}"
        );
        assert!(text.contains("1 secret file(s) hidden"), "{text}");
        let shallow = run(&ListFilesTool, ws.path(), json!({"depth": 1}))
            .unwrap()
            .text;
        assert!(!shallow.contains("main.rs"), "{shallow}");
        let capped = run(&ListFilesTool, ws.path(), json!({"max_entries": 2})).unwrap();
        assert!(
            capped.text.contains("more entries not shown"),
            "{}",
            capped.text
        );
        assert!(run(&ListFilesTool, ws.path(), json!({"path": "README.md"})).is_err());
    }

    #[test]
    fn search_text_finds_matches_and_skips_secret_binary_and_build_files() {
        let ws = workspace();
        let out = run(&SearchTextTool, ws.path(), json!({"pattern": "HELLO"})).unwrap();
        assert!(
            out.text.contains("README.md:2: hello world"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("src/deep/lib.rs:1: pub fn Hello() {}"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("target/"), "{}", out.text);
        let exact = run(
            &SearchTextTool,
            ws.path(),
            json!({"pattern": "HELLO", "case_sensitive": true}),
        )
        .unwrap();
        assert!(exact.text.starts_with("0 match(es)"), "{}", exact.text);
        let secret = run(&SearchTextTool, ws.path(), json!({"pattern": "API_KEY"})).unwrap();
        assert!(secret.text.starts_with("0 match(es)"), "{}", secret.text);
        let one = run(
            &SearchTextTool,
            ws.path(),
            json!({"pattern": "o", "max_results": 1}),
        )
        .unwrap();
        assert!(
            one.text.contains("stopped at the result or file limit"),
            "{}",
            one.text
        );
        assert!(run(&SearchTextTool, ws.path(), json!({"pattern": ""})).is_err());
        assert!(
            run(
                &SearchTextTool,
                ws.path(),
                json!({"pattern": "x", "path": ".env"})
            )
            .is_err()
        );
    }

    #[test]
    fn write_file_creates_replaces_and_respects_the_policy() {
        let ws = workspace();
        let made = run(
            &WriteFileTool,
            ws.path(),
            json!({"path": "new/dir/a.txt", "content": "one\ntwo\n"}),
        )
        .unwrap();
        assert!(
            made.text.starts_with("created new/dir/a.txt"),
            "{}",
            made.text
        );
        assert_eq!(
            fs::read_to_string(ws.path().join("new/dir/a.txt")).unwrap(),
            "one\ntwo\n"
        );
        let again = run(
            &WriteFileTool,
            ws.path(),
            json!({"path": "new/dir/a.txt", "content": "x"}),
        )
        .unwrap();
        assert!(again.text.starts_with("replaced"), "{}", again.text);

        for (path, why) in [
            ("local-agent.toml", "control file"),
            (".git/config", "git metadata"),
            ("../escape.txt", "outside the workspace"),
            (".env", "secret file needs approval"),
        ] {
            let err = run(
                &WriteFileTool,
                ws.path(),
                json!({"path": path, "content": "x"}),
            )
            .unwrap_err();
            assert!(err.contains("blocked by policy"), "{why}: {err}");
        }
        assert_eq!(
            fs::read_to_string(ws.path().join("local-agent.toml")).unwrap(),
            "profile = \"local-only\"\n",
            "the control file is untouched"
        );
        assert!(!ws.path().parent().unwrap().join("escape.txt").exists());
        let dir_err = run(
            &WriteFileTool,
            ws.path(),
            json!({"path": "src", "content": "x"}),
        )
        .unwrap_err();
        assert!(dir_err.contains("directory"), "{dir_err}");
        let big = "x".repeat(MAX_WRITE_BYTES + 1);
        assert!(
            run(
                &WriteFileTool,
                ws.path(),
                json!({"path": "big.txt", "content": big})
            )
            .is_err()
        );
    }

    #[test]
    fn edit_file_replaces_unique_text_and_explains_failures() {
        let ws = workspace();
        let edit = |old: &str, new: &str, all: bool| {
            run(
                &EditFileTool,
                ws.path(),
                json!({"path": "src/main.rs", "old_string": old, "new_string": new, "replace_all": all}),
            )
        };
        let ok = edit("println!(\"hi\");", "println!(\"hello\");", false).unwrap();
        assert!(
            ok.text
                .contains("replaced 1 occurrence(s), first at line 2"),
            "{}",
            ok.text
        );
        assert!(
            fs::read_to_string(ws.path().join("src/main.rs"))
                .unwrap()
                .contains("hello")
        );

        assert!(
            edit("no such text", "x", false)
                .unwrap_err()
                .contains("not found")
        );
        assert!(edit("same", "same", false).unwrap_err().contains("same"));
        assert!(
            edit("", "x", false)
                .unwrap_err()
                .contains("must not be empty")
        );

        fs::write(ws.path().join("dup.txt"), "a\nb\na\n").unwrap();
        let dup = |all: bool| {
            run(
                &EditFileTool,
                ws.path(),
                json!({"path": "dup.txt", "old_string": "a", "new_string": "z", "replace_all": all}),
            )
        };
        assert!(dup(false).unwrap_err().contains("appears 2 times"));
        assert!(dup(true).unwrap().text.contains("replaced 2 occurrence(s)"));
        assert_eq!(
            fs::read_to_string(ws.path().join("dup.txt")).unwrap(),
            "z\nb\nz\n"
        );

        let protected = run(
            &EditFileTool,
            ws.path(),
            json!({"path": "local-agent.toml", "old_string": "local-only", "new_string": "x"}),
        );
        assert!(protected.unwrap_err().contains("blocked by policy"));
        let missing = run(
            &EditFileTool,
            ws.path(),
            json!({"path": "nope.txt", "old_string": "a", "new_string": "b"}),
        );
        assert!(missing.unwrap_err().contains("cannot read"));
    }

    #[test]
    fn writes_are_atomic_leave_no_temp_files_and_keep_permissions() {
        let ws = workspace();
        let dir = ws.path().join("src");
        run(
            &EditFileTool,
            ws.path(),
            json!({"path": "src/main.rs", "old_string": "hi", "new_string": "ho"}),
        )
        .unwrap();
        run(
            &WriteFileTool,
            ws.path(),
            json!({"path": "src/new.txt", "content": "x"}),
        )
        .unwrap();
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files left behind: {leftovers:?}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = ws.path().join("run.sh");
            fs::write(&script, "#!/bin/sh\necho one\n").unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            run(
                &EditFileTool,
                ws.path(),
                json!({"path": "run.sh", "old_string": "one", "new_string": "two"}),
            )
            .unwrap();
            let mode = fs::metadata(&script).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "an edited script stays executable");
            assert!(fs::read_to_string(&script).unwrap().contains("two"));
        }
    }

    #[test]
    fn outputs_are_redacted() {
        let ws = workspace();
        fs::write(
            ws.path().join("conf.txt"),
            "token = ghp_0123456789abcdefghijklmnopqrstuvwxyz\n",
        )
        .unwrap();
        let out = run(&ReadFileTool, ws.path(), json!({"path": "conf.txt"})).unwrap();
        assert!(
            !out.text
                .contains("ghp_0123456789abcdefghijklmnopqrstuvwxyz"),
            "{}",
            out.text
        );
    }

    #[test]
    fn bad_arguments_are_errors_not_panics() {
        let ws = workspace();
        assert!(run(&ReadFileTool, ws.path(), json!({})).is_err());
        assert!(run(&ReadFileTool, ws.path(), json!({"path": 5})).is_err());
        assert!(
            run(
                &ReadFileTool,
                ws.path(),
                json!({"path": "a", "offset": "x"})
            )
            .is_err()
        );
        assert!(run(&WriteFileTool, ws.path(), json!({"path": "a"})).is_err());
        assert!(
            run(
                &EditFileTool,
                ws.path(),
                json!({"path": "a", "old_string": "x"})
            )
            .is_err()
        );
        assert_eq!(file_tools().len(), 5);
    }
}
