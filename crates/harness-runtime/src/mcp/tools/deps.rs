//! `dependency_search` (M2 step 6): the dependencies the tracked manifests declare, and the
//! source files that import a given dependency. Manifests are read only when tracked and only
//! when the read policy allows them; source files are scanned with the same limits as the code
//! tools.

use serde_json::{Value, json};

use crate::code::deps::{Dependency, is_manifest, normalise, parse_manifest};
use crate::code::{Language, imports, parse};

use super::{
    McpTool, ToolContext, ToolOutput, check_read_path, lossy, redacted_output, run_git, u64_arg,
};

/// Most source files scanned for imports in one call.
const MAX_SCANNED_FILES: usize = 2_000;
/// Largest manifest or source file read.
const MAX_FILE_BYTES: u64 = 512 * 1024;

fn tracked(ctx: &ToolContext) -> Result<(Vec<String>, usize), String> {
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

fn read(ctx: &ToolContext, relative: &str) -> Option<String> {
    let path = ctx.workspace.join(relative);
    if std::fs::metadata(&path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    (!bytes.contains(&0)).then(|| lossy(&bytes))
}

/// The top-level module or crate a source import names, normalised. `None` for includes and
/// anything that does not name a package.
fn imported_package(language: Language, import_text: &str) -> Option<String> {
    let rest = match language {
        Language::Rust => import_text
            .strip_prefix("pub ")
            .unwrap_or(import_text)
            .strip_prefix("use ")?
            .trim_start_matches("::"),
        Language::Python => {
            if let Some(module) = import_text.strip_prefix("from ") {
                module
            } else {
                import_text.strip_prefix("import ")?
            }
        }
        Language::Cpp => return None,
    };
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .unwrap_or(rest.len());
    let head = &rest[..end];
    let package = match language {
        Language::Python => head.split('.').next().unwrap_or_default(),
        _ => head.split("::").next().unwrap_or_default(),
    };
    (!package.is_empty()).then(|| normalise(package))
}

/// `dependency_search`: declared dependencies, and the source files that import one.
#[derive(Debug)]
pub struct DependencySearchTool;

impl McpTool for DependencySearchTool {
    fn name(&self) -> &'static str {
        "dependency_search"
    }
    fn description(&self) -> &'static str {
        "List the dependencies declared by Cargo.toml, pyproject.toml, requirements*.txt and package.json, and the source files that import a given one."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Dependency name (case and -/_ insensitive). Omit to list all."},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 500, "default": 100},
            },
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let query = match args.get("name") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) if !name.trim().is_empty() => Some(name.trim().to_owned()),
            Some(_) => return Err("`name` must be a non-empty string".to_owned()),
        };
        let max = u64_arg(args, "max_results", 100)?.clamp(1, 500) as usize;
        let wanted = query.as_deref().map(normalise);
        let (files, _excluded) = tracked(ctx)?;

        // Declared dependencies from every manifest.
        let mut declared: Vec<(String, Dependency)> = Vec::new();
        for file in files.iter().filter(|f| {
            let name = f.rsplit(['/', '\\']).next().unwrap_or(f);
            is_manifest(name)
        }) {
            let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
            let Some(text) = read(ctx, file) else {
                continue;
            };
            for dep in parse_manifest(name, &text) {
                if wanted.as_ref().is_none_or(|w| normalise(&dep.name) == *w) {
                    declared.push((file.clone(), dep));
                }
            }
        }

        // Source files importing the named package (only when a name was given).
        let mut importers = Vec::new();
        let mut scanned = 0usize;
        let mut truncated = false;
        if let Some(wanted) = &wanted {
            for file in files.iter().filter(|f| Language::from_path(f).is_some()) {
                if scanned >= MAX_SCANNED_FILES {
                    truncated = true;
                    break;
                }
                let Some(language) = Language::from_path(file) else {
                    continue;
                };
                let Some(source) = read(ctx, file) else {
                    continue;
                };
                let Some(tree) = parse(language, &source) else {
                    continue;
                };
                scanned += 1;
                for import in imports(language, &tree, &source) {
                    if imported_package(language, &import.text).as_deref() == Some(wanted.as_str())
                    {
                        importers
                            .push(json!({"file": file, "line": import.line, "text": import.text}));
                    }
                }
            }
        }

        let declared_count = declared.len();
        let importers_count = importers.len();
        importers.truncate(max);
        declared.truncate(max);
        let declared_json: Vec<Value> = declared
            .iter()
            .map(|(file, dep)| {
                json!({
                    "file": file,
                    "name": dep.name,
                    "version": dep.version,
                    "section": dep.section,
                })
            })
            .collect();

        let mut text = format!("{declared_count} declared dependency(ies)");
        if let Some(name) = &query {
            text.push_str(&format!(" matching `{name}`"));
        }
        text.push('.');
        for item in &declared_json {
            text.push_str(&format!(
                "\n  {} {} [{}] in {}",
                item["name"].as_str().unwrap_or_default(),
                item["version"].as_str().unwrap_or("(no version)"),
                item["section"].as_str().unwrap_or_default(),
                item["file"].as_str().unwrap_or_default(),
            ));
        }
        if query.is_some() {
            text.push_str(&format!(
                "\n{importers_count} import(s) found in {scanned} source file(s)."
            ));
            for item in &importers {
                text.push_str(&format!(
                    "\n  {}:{}  {}",
                    item["file"].as_str().unwrap_or_default(),
                    item["line"],
                    item["text"].as_str().unwrap_or_default(),
                ));
            }
        }
        if truncated {
            text.push_str(&format!(
                "\n[scanned the first {MAX_SCANNED_FILES} source files]"
            ));
        }
        let structured = json!({
            "name": query,
            "declared": declared_json,
            "declared_count": declared_count,
            "imports": importers,
            "imports_count": importers_count,
            "scanned_files": scanned,
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

    #[test]
    fn import_heads_name_the_package() {
        assert_eq!(
            imported_package(Language::Rust, "use serde_json::Value;").as_deref(),
            Some("serde_json")
        );
        assert_eq!(
            imported_package(Language::Rust, "use std::fs;").as_deref(),
            Some("std")
        );
        assert_eq!(
            imported_package(Language::Python, "import numpy.linalg").as_deref(),
            Some("numpy")
        );
        assert_eq!(
            imported_package(Language::Python, "from flask_cors import CORS").as_deref(),
            Some("flask_cors")
        );
        assert_eq!(imported_package(Language::Cpp, "#include <vector>"), None);
    }

    #[test]
    fn declared_and_imported_dependencies_are_found() {
        let Some(dir) = init_repo() else { return };
        commit_files(
            dir.path(),
            &[
                (
                    "Cargo.toml",
                    "[package]\nname = \"x\"\n[dependencies]\nserde_json = \"1\"\n",
                ),
                (
                    "src/lib.rs",
                    "use serde_json::Value;\n\npub fn f(v: Value) -> Value { v }\n",
                ),
                ("calc.py", "import numpy\nfrom flask_cors import CORS\n"),
                ("requirements.txt", "numpy==1.26.0\n"),
            ],
        );
        let ctx = test_context(dir.path());

        let all = DependencySearchTool
            .call(&json!({}), &ctx)
            .unwrap()
            .structured
            .unwrap();
        assert_eq!(all["declared_count"], json!(2), "{all}");
        assert_eq!(all["imports_count"], json!(0));

        let numpy = DependencySearchTool
            .call(&json!({"name": "NumPy"}), &ctx)
            .unwrap()
            .structured
            .unwrap();
        // The requirement is reported as written, operator included.
        assert_eq!(
            numpy["declared"][0]["version"],
            json!("==1.26.0"),
            "{numpy}"
        );
        assert_eq!(numpy["imports"][0]["file"], json!("calc.py"));

        let serde = DependencySearchTool
            .call(&json!({"name": "serde_json"}), &ctx)
            .unwrap()
            .structured
            .unwrap();
        assert_eq!(serde["declared"][0]["section"], json!("dependencies"));
        assert_eq!(serde["imports"][0]["file"], json!("src/lib.rs"));
        assert!(
            DependencySearchTool
                .call(&json!({"name": ""}), &ctx)
                .is_err()
        );
    }
}
