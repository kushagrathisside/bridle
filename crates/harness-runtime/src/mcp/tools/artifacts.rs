//! `inspect_artifact`, `summarize_experiment` and `compare_experiments`.
//!
//! Result files are often untracked (training outputs, benchmark runs), so these tools read by
//! path rather than through `git ls-files`. Every file they open passes the read policy, and
//! config values are redacted before they leave. Files over [`MAX_ARTIFACT_BYTES`] are refused.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::artifacts::{csv_summary, final_metrics, json_shape, jsonl_summary, kind_of};

use super::{McpTool, ToolContext, ToolOutput, check_read_path, redacted_output};

/// Largest artifact or metrics file read.
const MAX_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;
/// Entries listed per experiment directory.
const MAX_ENTRIES: usize = 200;
/// Config files probed, in order.
const CONFIG_FILES: &[&str] = &["config.json", "params.json", "hparams.json", "args.json"];
/// Metrics files probed, in order.
const METRICS_FILES: &[&str] = &["metrics.json", "metrics.csv", "metrics.jsonl"];
/// Lines of a text artifact quoted from its end.
const TAIL_LINES: usize = 20;

/// Reads a text artifact the policy allows, within the size limit.
fn read_artifact(ctx: &ToolContext, path: &str) -> Result<String, String> {
    check_read_path(ctx, path)?;
    let full = ctx.workspace.join(path);
    let size = std::fs::metadata(&full)
        .map_err(|error| error.to_string())?
        .len();
    if size > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "`{path}` is larger than {MAX_ARTIFACT_BYTES} bytes"
        ));
    }
    std::fs::read_to_string(&full)
        .map_err(|error| format!("`{path}` is not readable text: {error}"))
}

fn required_path(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("`{key}` must be a non-empty string"))
}

/// `inspect_artifact`: the shape and statistics of one result file.
#[derive(Debug)]
pub struct InspectArtifactTool;

impl McpTool for InspectArtifactTool {
    fn name(&self) -> &'static str {
        "inspect_artifact"
    }
    fn description(&self) -> &'static str {
        "Summarise a result file: JSON shape, JSON Lines or CSV field statistics, or the tail of a text log."
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
        let path = required_path(args, "path")?;
        let text = read_artifact(ctx, &path)?;
        let kind = kind_of(&path);
        let summary = match kind {
            "json" => json!({"shape": json_shape(&text).ok_or("not valid JSON")?}),
            "jsonl" => jsonl_summary(&text),
            "csv" => csv_summary(&text),
            _ => json!({}),
        };
        let lines: Vec<&str> = text.lines().collect();
        let tail: Vec<&str> = lines.iter().rev().take(TAIL_LINES).rev().copied().collect();
        let mut out = format!("{path}: {kind}, {} line(s).", lines.len());
        match kind {
            "jsonl" => out.push_str(&format!(
                " {} record(s), {} malformed.",
                summary["records"], summary["malformed"]
            )),
            "csv" => out.push_str(&format!(
                " {} row(s), {} malformed; columns: {}.",
                summary["rows"],
                summary["malformed"],
                summary["header"]
                    .as_array()
                    .map(|h| h
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            )),
            _ => {}
        }
        if let Some(numeric) = summary.get("numeric").and_then(Value::as_object) {
            for (field, stats) in numeric {
                out.push_str(&format!(
                    "\n  {field}: min {} max {} last {} mean {:.4}",
                    stats["min"],
                    stats["max"],
                    stats["last"],
                    stats["mean"].as_f64().unwrap_or_default()
                ));
            }
        }
        if kind == "text" || kind == "json" {
            out.push_str("\nTail:");
            for line in &tail {
                out.push_str(&format!("\n  {line}"));
            }
        }
        let structured = json!({
            "path": path,
            "kind": kind,
            "lines": lines.len(),
            "summary": summary,
            "tail": if kind == "text" || kind == "json" { json!(tail) } else { json!(null) },
        });
        Ok(redacted_output(ctx, &out, Some(structured)))
    }
}

/// The summary of one experiment directory, as JSON (shared by the two experiment tools).
fn summarize_dir(ctx: &ToolContext, dir: &str) -> Result<Value, String> {
    check_read_path(ctx, dir)?;
    let full = ctx.workspace.join(dir);
    let mut names: Vec<String> = std::fs::read_dir(&full)
        .map_err(|error| format!("`{dir}` is not a readable directory: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    let mut excluded = 0usize;
    let mut files = Vec::new();
    for name in names.iter().take(MAX_ENTRIES) {
        let child = format!("{dir}/{name}");
        if check_read_path(ctx, &child).is_err() {
            excluded += 1;
            continue;
        }
        let size = std::fs::metadata(full.join(name))
            .map(|m| m.len())
            .unwrap_or(0);
        files.push(json!({"name": name, "size": size, "kind": kind_of(name)}));
    }

    let config = CONFIG_FILES
        .iter()
        .find(|name| names.iter().any(|n| n == *name))
        .and_then(|name| read_artifact(ctx, &format!("{dir}/{name}")).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|mut value| {
            ctx.redactor.redact_json(&mut value);
            value
        });

    let metrics_name = METRICS_FILES
        .iter()
        .find(|name| names.iter().any(|n| n == *name));
    let metrics: Option<BTreeMap<String, f64>> = metrics_name.and_then(|name| {
        let text = read_artifact(ctx, &format!("{dir}/{name}")).ok()?;
        final_metrics(name, &text)
    });

    Ok(json!({
        "directory": dir,
        "files": files,
        "files_excluded": excluded,
        "config": config,
        "metrics_file": metrics_name,
        "metrics": metrics,
    }))
}

/// `summarize_experiment`: the files, config and final metrics of one run directory.
#[derive(Debug)]
pub struct SummarizeExperimentTool;

impl McpTool for SummarizeExperimentTool {
    fn name(&self) -> &'static str {
        "summarize_experiment"
    }
    fn description(&self) -> &'static str {
        "Summarise an experiment directory: its files, the config (secrets redacted), and the final value of each metric."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"directory": {"type": "string", "description": "Run directory inside the workspace."}},
            "required": ["directory"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let dir = required_path(args, "directory")?;
        let summary = summarize_dir(ctx, &dir)?;
        let mut text = format!(
            "{dir}: {} file(s).",
            summary["files"].as_array().map_or(0, Vec::len)
        );
        if let Some(metrics) = summary["metrics"].as_object() {
            text.push_str(&format!(
                " Final metrics from {}:",
                summary["metrics_file"].as_str().unwrap_or_default()
            ));
            for (name, value) in metrics {
                text.push_str(&format!("\n  {name} = {value}"));
            }
        } else {
            text.push_str(" No metrics file found.");
        }
        if summary["config"].is_object() {
            text.push_str("\nConfig keys: ");
            text.push_str(
                &summary["config"]
                    .as_object()
                    .map(|c| c.keys().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_default(),
            );
        }
        Ok(redacted_output(ctx, &text, Some(summary)))
    }
}

/// `compare_experiments`: metric differences and the config keys that differ between two runs.
#[derive(Debug)]
pub struct CompareExperimentsTool;

impl McpTool for CompareExperimentsTool {
    fn name(&self) -> &'static str {
        "compare_experiments"
    }
    fn description(&self) -> &'static str {
        "Compare two experiment directories: final metric values with their differences, and config keys that differ."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "baseline": {"type": "string", "description": "Run directory used as the reference."},
                "candidate": {"type": "string", "description": "Run directory being compared."},
            },
            "required": ["baseline", "candidate"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let baseline = required_path(args, "baseline")?;
        let candidate = required_path(args, "candidate")?;
        let a = summarize_dir(ctx, &baseline)?;
        let b = summarize_dir(ctx, &candidate)?;

        let empty = serde_json::Map::new();
        let (am, bm) = (
            a["metrics"].as_object().unwrap_or(&empty),
            b["metrics"].as_object().unwrap_or(&empty),
        );
        let mut names: Vec<&String> = am.keys().chain(bm.keys()).collect();
        names.sort();
        names.dedup();
        let metrics: Vec<Value> = names
            .iter()
            .map(|name| {
                let (x, y) = (
                    am.get(*name).and_then(Value::as_f64),
                    bm.get(*name).and_then(Value::as_f64),
                );
                json!({
                    "metric": name,
                    "baseline": x,
                    "candidate": y,
                    "delta": match (x, y) { (Some(x), Some(y)) => json!(y - x), _ => Value::Null },
                })
            })
            .collect();

        let ac = a["config"].as_object().unwrap_or(&empty);
        let bc = b["config"].as_object().unwrap_or(&empty);
        let mut keys: Vec<&String> = ac.keys().chain(bc.keys()).collect();
        keys.sort();
        keys.dedup();
        let changed_config: Vec<&String> = keys
            .into_iter()
            .filter(|k| ac.get(*k) != bc.get(*k))
            .collect();

        let mut text = format!("{baseline} vs {candidate}:");
        for row in &metrics {
            text.push_str(&format!(
                "\n  {}: {} -> {} (delta {})",
                row["metric"].as_str().unwrap_or_default(),
                row["baseline"],
                row["candidate"],
                row["delta"],
            ));
        }
        if changed_config.is_empty() {
            text.push_str("\nConfig: no differences in top-level keys.");
        } else {
            text.push_str(&format!(
                "\nConfig differs in: {}",
                changed_config
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let structured = json!({
            "baseline": baseline,
            "candidate": candidate,
            "metrics": metrics,
            "config_changed": changed_config,
        });
        Ok(redacted_output(ctx, &text, Some(structured)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::test_context;
    use super::*;
    use std::fs;

    fn run(dir: &std::path::Path, name: &str, config: &str, metrics: &str) {
        let run = dir.join(name);
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("config.json"), config).unwrap();
        fs::write(run.join("metrics.json"), metrics).unwrap();
    }

    #[test]
    fn experiments_compare_metrics_and_config_keys() {
        let dir = tempfile::tempdir().unwrap();
        run(
            dir.path(),
            "a",
            r#"{"lr":0.1,"seed":1,"api_key":"sk-live-0123456789abcdefghij"}"#,
            r#"{"acc":0.80,"loss":{"val":0.5}}"#,
        );
        run(
            dir.path(),
            "b",
            r#"{"lr":0.01,"seed":1,"api_key":"sk-live-0123456789abcdefghij"}"#,
            r#"{"acc":0.90,"loss":{"val":0.4}}"#,
        );
        let ctx = test_context(dir.path());

        let out = CompareExperimentsTool
            .call(&json!({"baseline": "a", "candidate": "b"}), &ctx)
            .unwrap();
        let s = out.structured.unwrap();
        let acc = s["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["metric"] == json!("acc"))
            .unwrap();
        assert!(
            (acc["delta"].as_f64().unwrap() - 0.10).abs() < 1e-9,
            "{acc}"
        );
        assert_eq!(s["config_changed"], json!(["lr"]), "{s}");
        assert!(!s.to_string().contains("sk-live"), "{s}");
    }

    #[test]
    fn summaries_keep_secret_files_out() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("metrics.csv"), "epoch,acc\n1,0.5\n2,0.9\n").unwrap();
        fs::write(run.join(".env"), "TOKEN=hunter2\n").unwrap();
        let ctx = test_context(dir.path());
        let s = SummarizeExperimentTool
            .call(&json!({"directory": "run"}), &ctx)
            .unwrap()
            .structured
            .unwrap();
        assert_eq!(s["metrics"]["acc"], json!(0.9), "{s}");
        assert_eq!(s["files_excluded"], json!(1));
        assert!(!s.to_string().contains("hunter2"));
    }

    #[test]
    fn inspect_artifact_summarises_jsonl_and_refuses_secrets() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("train.jsonl"),
            "{\"step\":1,\"loss\":2.0}\n{\"step\":2,\"loss\":1.0}\n",
        )
        .unwrap();
        fs::write(dir.path().join("credentials.json"), "{}").unwrap();
        let ctx = test_context(dir.path());
        let s = InspectArtifactTool
            .call(&json!({"path": "train.jsonl"}), &ctx)
            .unwrap()
            .structured
            .unwrap();
        assert_eq!(s["summary"]["records"], json!(2));
        assert_eq!(s["summary"]["numeric"]["loss"]["last"], json!(1.0));
        assert!(
            InspectArtifactTool
                .call(&json!({"path": "credentials.json"}), &ctx)
                .is_err()
        );
    }
}
