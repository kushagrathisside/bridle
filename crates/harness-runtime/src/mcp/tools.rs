//! The harness's MCP tools. M1 ships `git_status`, `git_diff`, `run_verification` and
//! `harness_info`; M2 adds the deterministic repository tools.
//!
//! IMPLEMENTATION SPEC (delegated). Each tool is a struct implementing [`McpTool`]; all run git or
//! commands with `std::process::Command` in `ctx.workspace`, never through a shell, except
//! verification steps (see below). Outputs are redacted with `ctx.redactor` before returning.
//!
//! - `git_status` (no args): runs `git status --porcelain=v2 --branch -z`; parse with the pure
//!   `parse_porcelain_v2(&[u8]) -> GitStatus { branch, upstream, ahead, behind, staged: Vec<String>,
//!   unstaged: Vec<String>, untracked: Vec<String>, conflicted: Vec<String> }` (handle `1`, `2`
//!   (rename: path + orig path), `u`, `?` records and `# branch.*` headers). `text` = a compact
//!   human summary; `structured` = the JSON of `GitStatus`. Not a git repo → `Err`.
//! - `git_diff` args `{ "staged": bool = false, "paths": [string] = [], "stat_only": bool = false,
//!   "max_bytes": int = 60000 }`: every path must pass `ctx.policy` path checks (use
//!   `ToolPolicy::evaluate` with `ToolKind::Read` and the path; Deny → `Err`). Runs
//!   `git diff [--cached] --stat` and (unless stat_only) `git diff [--cached] --no-color -- <paths>`,
//!   truncating the patch to `max_bytes` with a marker. `structured` = `{ files_changed,
//!   insertions, deletions, truncated }` parsed from the shortstat line (pure helper
//!   `parse_shortstat`).
//! - `run_verification` args `{ "steps": [string] (optional names to run; default all) }`: runs
//!   `ctx.verification.steps` sequentially. BEFORE running each step, classify its command with
//!   `ctx.policy.assess(cmd)` → if the verdict (`ctx.policy.class_verdict`) is not Allow, skip
//!   it with status `blocked` and the reasons (verification must never smuggle risky commands).
//!   Run allowed steps through the platform shell (`cmd /C` on Windows, `sh -c` elsewhere) with
//!   `cwd = workspace/step.cwd`, a timeout (step.timeout_ms or `default_timeout_ms`) enforced by
//!   polling `try_wait` and killing on expiry, and capture stdout+stderr. Result per step: `{ name,
//!   command, status: passed|failed|timeout|blocked, exit_code, duration_ms, output_tail }` where
//!   `output_tail` is the last 4000 bytes, redacted. `structured` = `{ passed: bool, steps: [...] }`;
//!   `is_error` = false even when steps fail (failure is information, not a tool error). No steps
//!   configured → text explains how to add `[[verification.steps]]`.
//! - `harness_info` (no args): `structured` = `{ product, version, workspace, profile, models: [{slot,
//!   id, role, endpoint, context}], verification_steps: [names] }`, never including secrets.
//! - `default_tools()` returns all four in that order.
//! - Tests: `parse_porcelain_v2` and `parse_shortstat` fixtures; `git_status`/`git_diff` against a
//!   temp repo (`git init`, commit, modify) — skip gracefully if `git` is missing;
//!   `run_verification` with passing, failing, blocked (`rm -rf x`) and timeout (`sleep 5` with a
//!   100 ms timeout; on Windows use `ping -n 6 127.0.0.1 > NUL`) steps.

use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use harness_core::config::{Config, ModelRole, Verdict, VerificationSection, VerificationStep};
use harness_core::security::redact::Redactor;
use harness_core::security::tool_policy::{ToolKind, ToolPolicy, ToolRequest};
use serde::Serialize;
use serde_json::{Value, json};

const PRODUCT_NAME: &str = "harness";
const DEFAULT_DIFF_MAX_BYTES: u64 = 60_000;
const OUTPUT_TAIL_BYTES: usize = 4_000;
/// Bytes kept per output stream while a step runs; only the tail is ever reported.
const CAPTURE_LIMIT_BYTES: usize = 64 * 1024;
/// Entries listed per category in the `git_status` summary.
const LIST_LIMIT: usize = 50;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long to wait for output readers after a step exits, so a grandchild that inherited the
/// pipes cannot hold the tool call open.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

const PASSED: &str = "passed";
const FAILED: &str = "failed";
const TIMEOUT: &str = "timeout";
const BLOCKED: &str = "blocked";

const NO_STEPS_HELP: &str = "No verification steps are configured. Add one or more \
[[verification.steps]] tables to the harness config file, for example:\n\n\
[[verification.steps]]\n\
name = \"tests\"\n\
command = \"cargo test\"\n\n\
Each step may also set timeout_ms and cwd (relative to the workspace).";

/// Shared state every tool call receives.
#[derive(Debug)]
pub struct ToolContext {
    /// Directory every tool runs in.
    pub workspace: PathBuf,
    /// Loaded harness configuration.
    pub config: Config,
    /// Verification steps and their defaults.
    pub verification: VerificationSection,
    /// Policy used for path checks and command classification.
    pub policy: ToolPolicy,
    /// Secret redaction applied to every output.
    pub redactor: Redactor,
}

/// A tool's result. `text` is what the model reads; `structured` is machine-readable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolOutput {
    /// Human-readable result.
    pub text: String,
    /// Machine-readable result, if the tool has one.
    pub structured: Option<serde_json::Value>,
    /// Whether this is a tool-level error. Failed verification steps do not set this.
    pub is_error: bool,
}

/// An MCP tool: a name, a description, an argument schema and a call.
pub trait McpTool: Send {
    /// The name clients use in `tools/call`.
    fn name(&self) -> &'static str;
    /// One-line description shown to the model.
    fn description(&self) -> &'static str;
    /// JSON Schema for the arguments object.
    fn input_schema(&self) -> serde_json::Value;
    /// Runs the tool. `Err(message)` becomes an `isError` result for the client.
    fn call(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput, String>;
}

/// Every tool this server offers, in the order `tools/list` reports them.
pub fn default_tools() -> Vec<Box<dyn McpTool>> {
    vec![
        Box::new(GitStatusTool),
        Box::new(GitDiffTool),
        Box::new(RunVerificationTool),
        Box::new(HarnessInfoTool),
    ]
}

/// Parsed `git status --porcelain=v2 --branch` output.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GitStatus {
    /// Current branch name, or `(detached)`; `None` if git did not report one.
    pub branch: Option<String>,
    /// Upstream branch, if one is configured.
    pub upstream: Option<String>,
    /// Commits the branch is ahead of its upstream.
    pub ahead: u32,
    /// Commits the branch is behind its upstream.
    pub behind: u32,
    /// Paths with changes staged in the index.
    pub staged: Vec<String>,
    /// Paths with changes in the working tree that are not staged.
    pub unstaged: Vec<String>,
    /// Paths git does not track.
    pub untracked: Vec<String>,
    /// Paths with unresolved merge conflicts.
    pub conflicted: Vec<String>,
}

/// Totals from the `git diff --stat` summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DiffStat {
    /// Number of files changed.
    pub files_changed: u64,
    /// Lines inserted.
    pub insertions: u64,
    /// Lines deleted.
    pub deletions: u64,
}

/// Parses `git status --porcelain=v2 --branch -z` output into a [`GitStatus`].
///
/// Records are NUL-terminated. A rename (`2`) record is followed by its original path as a
/// separate record, which is consumed and not reported. Unknown record kinds are ignored.
pub fn parse_porcelain_v2(output: &[u8]) -> GitStatus {
    let mut status = GitStatus::default();
    let mut records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(lossy);

    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            apply_header(&mut status, header);
            continue;
        }
        match record.chars().next() {
            Some('1') => {
                let fields: Vec<&str> = record.splitn(9, ' ').collect();
                if let (Some(xy), Some(path)) = (fields.get(1), fields.get(8)) {
                    record_change(&mut status, xy, path);
                }
            }
            Some('2') => {
                // The original path is the following NUL-separated record; it is not reported.
                let _orig_path = records.next();
                let fields: Vec<&str> = record.splitn(10, ' ').collect();
                if let (Some(xy), Some(path)) = (fields.get(1), fields.get(9)) {
                    record_change(&mut status, xy, path);
                }
            }
            Some('u') => {
                let fields: Vec<&str> = record.splitn(11, ' ').collect();
                if let Some(path) = fields.get(10) {
                    status.conflicted.push((*path).to_owned());
                }
            }
            Some('?') => {
                if let Some(path) = record.strip_prefix("? ") {
                    status.untracked.push(path.to_owned());
                }
            }
            _ => {}
        }
    }
    status
}

fn apply_header(status: &mut GitStatus, header: &str) {
    if let Some(head) = header.strip_prefix("branch.head ") {
        status.branch = Some(head.to_owned());
    } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
        status.upstream = Some(upstream.to_owned());
    } else if let Some(ab) = header.strip_prefix("branch.ab ") {
        for part in ab.split_whitespace() {
            if let Some(count) = part.strip_prefix('+') {
                status.ahead = count.parse().unwrap_or(0);
            } else if let Some(count) = part.strip_prefix('-') {
                status.behind = count.parse().unwrap_or(0);
            }
        }
    }
}

/// Files a changed entry under `staged` (index column) and/or `unstaged` (worktree column).
fn record_change(status: &mut GitStatus, xy: &str, path: &str) {
    let mut columns = xy.chars();
    let index = columns.next().unwrap_or('.');
    let worktree = columns.next().unwrap_or('.');
    if index != '.' {
        status.staged.push(path.to_owned());
    }
    if worktree != '.' {
        status.unstaged.push(path.to_owned());
    }
}

/// Parses the summary line of `git diff --stat` (for example ` 2 files changed, 3 insertions(+),
/// 1 deletion(-)`). Returns zeros when there is no summary line, as for an empty diff.
pub fn parse_shortstat(text: &str) -> DiffStat {
    let mut stat = DiffStat::default();
    let summary = text.lines().rev().find(|line| {
        line.contains("changed") && line.trim_start().starts_with(|c: char| c.is_ascii_digit())
    });
    let Some(summary) = summary else {
        return stat;
    };
    for part in summary.split(',') {
        let part = part.trim();
        let Some(count) = part
            .split_whitespace()
            .next()
            .and_then(|token| token.parse::<u64>().ok())
        else {
            continue;
        };
        if part.contains("insertion") {
            stat.insertions = count;
        } else if part.contains("deletion") {
            stat.deletions = count;
        } else if part.contains("changed") {
            stat.files_changed = count;
        }
    }
    stat
}

/// Runs `git` with explicit arguments in the workspace. Errors are redacted.
fn run_git(ctx: &ToolContext, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(&ctx.workspace)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            ctx.redactor
                .redact(&format!("failed to run git: {error}"))
                .into_owned()
        })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let command = args.first().copied().unwrap_or_default();
        Err(ctx
            .redactor
            .redact(&format!("git {command} failed: {}", stderr.trim()))
            .into_owned())
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Builds an output with redacted text and redacted structured content.
fn redacted_output(ctx: &ToolContext, text: &str, structured: Option<Value>) -> ToolOutput {
    let text = ctx.redactor.redact(text).into_owned();
    let structured = structured.map(|mut value| {
        ctx.redactor.redact_json(&mut value);
        value
    });
    ToolOutput {
        text,
        structured,
        is_error: false,
    }
}

fn render_status(status: &GitStatus) -> String {
    let branch = status.branch.as_deref().unwrap_or("(unknown)");
    let mut out = match &status.upstream {
        Some(upstream) => format!(
            "On branch {branch}, tracking {upstream}: ahead {}, behind {}",
            status.ahead, status.behind
        ),
        None => format!("On branch {branch}"),
    };
    let groups = [
        ("Conflicted", &status.conflicted),
        ("Staged", &status.staged),
        ("Unstaged", &status.unstaged),
        ("Untracked", &status.untracked),
    ];
    if groups.iter().all(|(_, paths)| paths.is_empty()) {
        out.push_str("\nWorking tree clean.");
    }
    for (label, paths) in groups {
        if paths.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{label} ({}):", paths.len()));
        for path in paths.iter().take(LIST_LIMIT) {
            out.push_str(&format!("\n  {path}"));
        }
        if paths.len() > LIST_LIMIT {
            out.push_str(&format!("\n  ... and {} more", paths.len() - LIST_LIMIT));
        }
    }
    out
}

/// `git_status`: the branch and changed paths of the workspace.
#[derive(Debug)]
pub struct GitStatusTool;

impl McpTool for GitStatusTool {
    fn name(&self) -> &'static str {
        "git_status"
    }
    fn description(&self) -> &'static str {
        "Show the branch and the staged, unstaged, untracked and conflicted paths of the workspace."
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }
    fn call(&self, _args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let stdout = run_git(ctx, &["status", "--porcelain=v2", "--branch", "-z"])?;
        let status = parse_porcelain_v2(&stdout);
        let structured = serde_json::to_value(&status).map_err(|error| error.to_string())?;
        Ok(redacted_output(
            ctx,
            &render_status(&status),
            Some(structured),
        ))
    }
}

/// `git_diff`: the diff of the workspace, optionally staged and limited to paths.
#[derive(Debug)]
pub struct GitDiffTool;

impl McpTool for GitDiffTool {
    fn name(&self) -> &'static str {
        "git_diff"
    }
    fn description(&self) -> &'static str {
        "Show a summary and the patch of unstaged (or staged) changes, optionally limited to paths."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "staged": {"type": "boolean", "description": "Diff the index against HEAD.", "default": false},
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Limit the diff to these paths."},
                "stat_only": {"type": "boolean", "description": "Return only the file summary.", "default": false},
                "max_bytes": {"type": "integer", "minimum": 0, "description": "Truncate the patch to this many bytes.", "default": DEFAULT_DIFF_MAX_BYTES},
            },
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let staged = bool_arg(args, "staged", false)?;
        let stat_only = bool_arg(args, "stat_only", false)?;
        let max_bytes = usize::try_from(u64_arg(args, "max_bytes", DEFAULT_DIFF_MAX_BYTES)?)
            .unwrap_or(usize::MAX);
        let paths = optional_string_list(args, "paths")?.unwrap_or_default();
        for path in &paths {
            check_read_path(ctx, path)?;
        }

        // Shared tail of both invocations: `--cached` first, then `--` and the paths.
        let mut scope: Vec<&str> = Vec::new();
        if staged {
            scope.push("--cached");
        }
        if !paths.is_empty() {
            scope.push("--");
            scope.extend(paths.iter().map(String::as_str));
        }
        let stat_args: Vec<&str> = ["diff", "--stat", "--no-color", "--no-ext-diff"]
            .into_iter()
            .chain(scope.iter().copied())
            .collect();
        let patch_args: Vec<&str> = ["diff", "--no-color", "--no-ext-diff"]
            .into_iter()
            .chain(scope.iter().copied())
            .collect();

        let stat_raw = lossy(&run_git(ctx, &stat_args)?);
        let stat = parse_shortstat(&stat_raw);
        let (patch, truncated) = if stat_only {
            (String::new(), false)
        } else {
            // Redact before truncating so a secret cannot survive by being cut in half.
            let patch_raw = lossy(&run_git(ctx, &patch_args)?);
            let redacted = ctx.redactor.redact(&patch_raw);
            truncate_text(&redacted, max_bytes)
        };

        let mut text = ctx.redactor.redact(&stat_raw).trim_end().to_owned();
        if !patch.is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&patch);
        }
        if text.is_empty() {
            text.push_str("No changes.");
        }

        Ok(ToolOutput {
            text,
            structured: Some(json!({
                "files_changed": stat.files_changed,
                "insertions": stat.insertions,
                "deletions": stat.deletions,
                "truncated": truncated,
            })),
            is_error: false,
        })
    }
}

/// Cuts `text` to at most `max_bytes` (on a character boundary) and appends a marker when it cut.
fn truncate_text(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let omitted = text.len() - end;
    let mut out = text.get(..end).unwrap_or_default().to_owned();
    out.push_str(&format!(
        "\n[... diff truncated: {omitted} bytes omitted; raise max_bytes or narrow paths ...]\n"
    ));
    (out, true)
}

/// Rejects a path the policy would not let a read tool touch (outside the workspace, secrets).
fn check_read_path(ctx: &ToolContext, path: &str) -> Result<(), String> {
    let request = ToolRequest {
        paths: vec![path.to_owned()],
        ..ToolRequest::new(ToolKind::Read)
    };
    let decision = ctx.policy.evaluate(&request);
    if matches!(decision.verdict, Verdict::Allow) {
        Ok(())
    } else {
        Err(format!(
            "path not allowed: {path} ({})",
            decision.reasons.join("; ")
        ))
    }
}

fn bool_arg(args: &Value, key: &str, default: bool) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("{key} must be a boolean")),
    }
}

fn u64_arg(args: &Value, key: &str, default: u64) -> Result<u64, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

fn optional_string_list(args: &Value, key: &str) -> Result<Option<Vec<String>>, String> {
    let invalid = || format!("{key} must be an array of strings");
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(invalid()),
    }
}

/// `run_verification`: runs the configured verification steps.
#[derive(Debug)]
pub struct RunVerificationTool;

/// Outcome of one verification step, as reported to the client.
#[derive(Debug, Clone, Serialize)]
struct StepResult {
    name: String,
    command: String,
    status: &'static str,
    exit_code: Option<i32>,
    duration_ms: u64,
    output_tail: String,
}

impl McpTool for RunVerificationTool {
    fn name(&self) -> &'static str {
        "run_verification"
    }
    fn description(&self) -> &'static str {
        "Run the configured verification steps in order and report which passed."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "steps": {"type": "array", "items": {"type": "string"}, "description": "Names of the steps to run; default all."},
            },
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let requested = optional_string_list(args, "steps")?;
        if !ctx.verification.enabled {
            return Err("verification is disabled ([verification] enabled = false)".to_owned());
        }
        if ctx.verification.steps.is_empty() {
            return Ok(ToolOutput {
                text: NO_STEPS_HELP.to_owned(),
                structured: Some(json!({"passed": false, "steps": []})),
                is_error: false,
            });
        }

        let selected = select_steps(&ctx.verification.steps, requested.as_deref())?;
        let results: Vec<StepResult> = selected
            .into_iter()
            .map(|(name, step)| run_step(ctx, name, step))
            .collect();
        let passing = results.iter().filter(|step| step.status == PASSED).count();
        let passed = passing == results.len();

        let mut text = format!(
            "Verification {}: {passing} of {} steps passed.",
            if passed { "passed" } else { "failed" },
            results.len()
        );
        for step in &results {
            let exit = step
                .exit_code
                .map(|code| format!(", exit {code}"))
                .unwrap_or_default();
            text.push_str(&format!(
                "\n- {}: {}{exit} ({} ms)",
                step.name, step.status, step.duration_ms
            ));
            if step.status != PASSED {
                for line in step.output_tail.trim_end().lines() {
                    text.push_str(&format!("\n    {line}"));
                }
            }
        }

        let structured = json!({
            "passed": passed,
            "steps": serde_json::to_value(&results).map_err(|error| error.to_string())?,
        });
        Ok(ToolOutput {
            text,
            structured: Some(structured),
            is_error: false,
        })
    }
}

/// The name a step is reported under: its configured name, or `step-N` (1-based).
fn step_name(index: usize, step: &VerificationStep) -> String {
    step.name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("step-{}", index + 1))
}

/// Picks the steps to run, in configuration order. An absent or empty request selects all steps.
fn select_steps<'a>(
    steps: &'a [VerificationStep],
    requested: Option<&[String]>,
) -> Result<Vec<(String, &'a VerificationStep)>, String> {
    let named: Vec<(String, &VerificationStep)> = steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step_name(index, step), step))
        .collect();
    let Some(requested) = requested.filter(|names| !names.is_empty()) else {
        return Ok(named);
    };
    for wanted in requested {
        if !named.iter().any(|(name, _)| name == wanted) {
            let configured: Vec<&str> = named.iter().map(|(name, _)| name.as_str()).collect();
            return Err(format!(
                "unknown verification step: {wanted}; configured steps: {}",
                configured.join(", ")
            ));
        }
    }
    Ok(named
        .into_iter()
        .filter(|(name, _)| requested.contains(name))
        .collect())
}

/// Classifies one step, then runs it unless the policy refuses it.
fn run_step(ctx: &ToolContext, name: String, step: &VerificationStep) -> StepResult {
    let assessment = ctx.policy.assess(&step.command);
    let verdict = ctx.policy.class_verdict(assessment.class);
    if !matches!(verdict, Verdict::Allow) {
        let reasons = if assessment.reasons.is_empty() {
            format!("command class {}", assessment.class.as_str())
        } else {
            assessment.reasons.join("; ")
        };
        return StepResult {
            name,
            command: step.command.clone(),
            status: BLOCKED,
            exit_code: None,
            duration_ms: 0,
            output_tail: ctx
                .redactor
                .redact(&format!("not run: {reasons}"))
                .into_owned(),
        };
    }

    let cwd = match &step.cwd {
        Some(relative) => ctx.workspace.join(relative),
        None => ctx.workspace.clone(),
    };
    let timeout_ms = step
        .timeout_ms
        .unwrap_or(ctx.verification.default_timeout_ms);
    let started = Instant::now();
    let outcome = execute(&step.command, &cwd, Duration::from_millis(timeout_ms));
    let duration_ms = elapsed_ms(started);

    let (status, exit_code, output) = match outcome {
        Ok(run) if run.timed_out => (TIMEOUT, None, run.output),
        Ok(run) => {
            let success = run.exit.as_ref().is_some_and(ExitStatus::success);
            let status = if success { PASSED } else { FAILED };
            (status, run.exit.and_then(|exit| exit.code()), run.output)
        }
        Err(error) => (FAILED, None, error.into_bytes()),
    };
    StepResult {
        name,
        command: step.command.clone(),
        status,
        exit_code,
        duration_ms,
        output_tail: ctx.redactor.redact(&tail(&output)).into_owned(),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The last [`OUTPUT_TAIL_BYTES`] bytes of `output`, decoded lossily.
fn tail(output: &[u8]) -> String {
    let start = output.len().saturating_sub(OUTPUT_TAIL_BYTES);
    lossy(output.get(start..).unwrap_or_default())
}

/// The platform shell invocation for a verification command.
#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("cmd");
    shell.arg("/C").arg(command);
    shell
}

/// The platform shell invocation for a verification command.
#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

/// What happened when a command was executed.
struct Execution {
    /// The exit status, if the process was reaped.
    exit: Option<ExitStatus>,
    /// Whether the timeout expired and the process was killed.
    timed_out: bool,
    /// The tail of stdout followed by the tail of stderr.
    output: Vec<u8>,
}

/// Runs `command` through the platform shell in `cwd`, killing it once `timeout` expires.
fn execute(command: &str, cwd: &Path, timeout: Duration) -> Result<Execution, String> {
    let mut child = shell_command(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start step: {error}"))?;

    let (done_tx, done_rx) = mpsc::channel::<()>();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let mut readers = 0_usize;
    if let Some(pipe) = child.stdout.take() {
        capture(pipe, Arc::clone(&stdout), done_tx.clone());
        readers += 1;
    }
    if let Some(pipe) = child.stderr.take() {
        capture(pipe, Arc::clone(&stderr), done_tx.clone());
        readers += 1;
    }
    drop(done_tx);

    // `checked_add` keeps an absurdly large timeout from overflowing; it then never expires.
    let deadline = Instant::now().checked_add(timeout);
    let (exit, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if deadline.is_none_or(|deadline| Instant::now() < deadline) => {
                thread::sleep(POLL_INTERVAL);
            }
            Ok(None) => {
                let _ = child.kill();
                break (child.wait().ok(), true);
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed to wait for step: {error}"));
            }
        }
    };

    for _ in 0..readers {
        let _ = done_rx.recv_timeout(DRAIN_TIMEOUT);
    }
    let mut output = snapshot(&stdout);
    output.extend(snapshot(&stderr));
    Ok(Execution {
        exit,
        timed_out,
        output,
    })
}

/// Copies what a capture buffer holds so far.
fn snapshot(sink: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    sink.lock()
        .map(|buffer| buffer.to_vec())
        .unwrap_or_default()
}

/// Reads `pipe` to EOF on a background thread, keeping only the last [`CAPTURE_LIMIT_BYTES`].
/// Sends on `done` once the pipe is closed.
fn capture<R: Read + Send + 'static>(mut pipe: R, sink: Arc<Mutex<Vec<u8>>>, done: Sender<()>) {
    thread::spawn(move || {
        let mut chunk = [0_u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    if let Ok(mut buffer) = sink.lock() {
                        buffer.extend_from_slice(&chunk[..read]);
                        let excess = buffer.len().saturating_sub(CAPTURE_LIMIT_BYTES);
                        buffer.drain(..excess);
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = done.send(());
    });
}

/// `harness_info`: what this server is configured to do. Never includes secret values.
#[derive(Debug)]
pub struct HarnessInfoTool;

/// The configured role name, as spelled in the config (for example `engineer`).
fn role_name(role: ModelRole) -> String {
    serde_json::to_value(role)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{role:?}"))
}

impl McpTool for HarnessInfoTool {
    fn name(&self) -> &'static str {
        "harness_info"
    }
    fn description(&self) -> &'static str {
        "Describe the harness: version, workspace, profile, model slots and verification steps."
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }
    fn call(&self, _args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let models: Vec<Value> = ctx
            .config
            .models
            .iter()
            .map(|(slot, model)| {
                json!({
                    "slot": slot,
                    "id": model.id,
                    "role": role_name(model.role),
                    "endpoint": model.endpoint,
                    "context": model.context,
                })
            })
            .collect();
        let steps: Vec<String> = ctx
            .verification
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| step_name(index, step))
            .collect();
        let profile = ctx.config.profile.as_str();
        let workspace = ctx.workspace.display().to_string();
        let mut structured = json!({
            "product": PRODUCT_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "workspace": workspace,
            "profile": profile,
            "models": models,
            "verification_steps": steps,
        });
        // Endpoints may embed credentials in the URL; redaction removes them.
        ctx.redactor.redact_json(&mut structured);

        let mut text = format!(
            "{PRODUCT_NAME} {}\nworkspace: {workspace}\nprofile: {profile}",
            env!("CARGO_PKG_VERSION")
        );
        text.push_str("\nmodels:");
        if ctx.config.models.is_empty() {
            text.push_str(" none configured");
        }
        for (slot, model) in &ctx.config.models {
            text.push_str(&format!(
                "\n  {slot}: {} ({}) at {}, context {}",
                model.id,
                role_name(model.role),
                model.endpoint,
                model.context
            ));
        }
        if steps.is_empty() {
            text.push_str("\nverification steps: none configured");
        } else {
            text.push_str(&format!("\nverification steps: {}", steps.join(", ")));
        }
        Ok(redacted_output(ctx, &text, Some(structured)))
    }
}

/// A context for tests: the given workspace, default config and policy.
#[cfg(test)]
pub(crate) fn test_context(workspace: &Path) -> ToolContext {
    let policy = ToolPolicy::new(workspace, &harness_core::config::SecuritySection::default())
        .unwrap_or_else(|error| panic!("policy for {}: {error}", workspace.display()));
    ToolContext {
        workspace: workspace.to_path_buf(),
        config: Config::default(),
        verification: VerificationSection::default(),
        policy,
        redactor: Redactor::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;

    use super::*;

    const CLEAN_REPO: &[u8] = b"# branch.oid 0123456789abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +0 -0\0";

    const MIXED_REPO: &[u8] = b"# branch.oid 0123456789abcdef\0\
# branch.head feature/x\0\
# branch.upstream origin/feature/x\0\
# branch.ab +2 -3\0\
1 M. N... 100644 100644 100644 aaaa bbbb src/lib.rs\0\
1 .M N... 100644 100644 100644 aaaa aaaa README.md\0\
1 MM N... 100644 100644 100644 aaaa bbbb src/main.rs\0\
2 R. N... 100644 100644 100644 aaaa bbbb R100 new name.rs\0old name.rs\0\
u UU N... 100644 100644 100644 100644 aaaa bbbb cccc conflict.txt\0\
? notes/todo list.txt\0\
? scratch.txt\0";

    const UNBORN_REPO: &[u8] = b"# branch.oid (initial)\0# branch.head main\0";

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    /// A temporary repository with one committed file, or `None` when git is unavailable.
    fn init_repo() -> Option<tempfile::TempDir> {
        if !git_available() {
            eprintln!("git not found; skipping");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join("README.md"), "hello\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        );
        Some(dir)
    }

    fn ctx_for(dir: &Path) -> ToolContext {
        test_context(dir)
    }

    #[test]
    fn porcelain_clean_repository() {
        let status = parse_porcelain_v2(CLEAN_REPO);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (0, 0));
        assert_eq!(
            status,
            GitStatus {
                branch: Some("main".to_owned()),
                upstream: Some("origin/main".to_owned()),
                ..GitStatus::default()
            }
        );
    }

    #[test]
    fn porcelain_mixed_fixture() {
        let status = parse_porcelain_v2(MIXED_REPO);
        assert_eq!(status.branch.as_deref(), Some("feature/x"));
        assert_eq!(status.upstream.as_deref(), Some("origin/feature/x"));
        assert_eq!((status.ahead, status.behind), (2, 3));
        // `M.` is staged only, `.M` is unstaged only, `MM` is both.
        assert_eq!(
            status.staged,
            strings(&["src/lib.rs", "src/main.rs", "new name.rs"])
        );
        assert_eq!(status.unstaged, strings(&["README.md", "src/main.rs"]));
        assert_eq!(status.conflicted, strings(&["conflict.txt"]));
        assert_eq!(
            status.untracked,
            strings(&["notes/todo list.txt", "scratch.txt"])
        );
    }

    #[test]
    fn porcelain_rename_does_not_leak_original_path() {
        let status = parse_porcelain_v2(MIXED_REPO);
        assert!(!status.staged.iter().any(|path| path == "old name.rs"));
        assert!(!status.unstaged.iter().any(|path| path == "old name.rs"));
    }

    #[test]
    fn porcelain_unborn_branch_has_no_upstream() {
        let status = parse_porcelain_v2(UNBORN_REPO);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream, None);
        assert_eq!(
            status,
            GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }
        );
    }

    #[test]
    fn porcelain_empty_output_is_empty_status() {
        assert_eq!(parse_porcelain_v2(b""), GitStatus::default());
    }

    #[test]
    fn shortstat_fixtures() {
        assert_eq!(
            parse_shortstat(" 2 files changed, 10 insertions(+), 3 deletions(-)\n"),
            DiffStat {
                files_changed: 2,
                insertions: 10,
                deletions: 3
            }
        );
        assert_eq!(
            parse_shortstat(" 1 file changed, 1 insertion(+)"),
            DiffStat {
                files_changed: 1,
                insertions: 1,
                deletions: 0
            }
        );
        assert_eq!(
            parse_shortstat(" 1 file changed, 4 deletions(-)"),
            DiffStat {
                files_changed: 1,
                insertions: 0,
                deletions: 4
            }
        );
        assert_eq!(parse_shortstat(""), DiffStat::default());
    }

    #[test]
    fn shortstat_ignores_file_names_containing_changed() {
        let stat = " src/changed.rs | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n";
        assert_eq!(
            parse_shortstat(stat),
            DiffStat {
                files_changed: 1,
                insertions: 1,
                deletions: 1
            }
        );
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        let (text, truncated) = truncate_text("ééé", 3);
        assert!(truncated);
        assert!(text.starts_with('é'));
        assert!(text.contains("truncated"));
        assert_eq!(truncate_text("short", 60_000), ("short".to_owned(), false));
    }

    #[test]
    fn git_status_reports_each_category() {
        let Some(dir) = init_repo() else { return };
        fs::write(dir.path().join("README.md"), "hello\nworld\n").unwrap();
        fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        git(dir.path(), &["add", "new.txt"]);
        fs::write(dir.path().join("other.txt"), "other\n").unwrap();

        let output = GitStatusTool
            .call(&json!({}), &ctx_for(dir.path()))
            .unwrap();
        assert!(!output.is_error);
        let structured = output.structured.unwrap();
        assert_eq!(structured["staged"], json!(["new.txt"]));
        assert_eq!(structured["unstaged"], json!(["README.md"]));
        assert_eq!(structured["untracked"], json!(["other.txt"]));
        assert!(output.text.contains("Untracked (1):"));
    }

    #[test]
    fn git_status_outside_a_repository_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        if !git_available() {
            return;
        }
        // A tempdir is not inside a repository in any sane setup; if it were, skip rather than lie.
        let inside_repo = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(dir.path())
            .output()
            .is_ok_and(|output| output.status.success());
        if inside_repo {
            return;
        }
        assert!(
            GitStatusTool
                .call(&json!({}), &ctx_for(dir.path()))
                .is_err()
        );
    }

    #[test]
    fn git_diff_reports_unstaged_change() {
        let Some(dir) = init_repo() else { return };
        fs::write(dir.path().join("README.md"), "hello\nworld\n").unwrap();

        let output = GitDiffTool.call(&json!({}), &ctx_for(dir.path())).unwrap();
        assert!(output.text.contains("README.md"));
        assert!(output.text.contains("+world"));
        let structured = output.structured.unwrap();
        assert_eq!(structured["files_changed"], json!(1));
        assert_eq!(structured["insertions"], json!(1));
        assert_eq!(structured["deletions"], json!(0));
        assert_eq!(structured["truncated"], json!(false));
    }

    #[test]
    fn git_diff_staged_scope_ignores_unstaged_changes() {
        let Some(dir) = init_repo() else { return };
        fs::write(dir.path().join("README.md"), "hello\nworld\n").unwrap();

        let output = GitDiffTool
            .call(&json!({"staged": true}), &ctx_for(dir.path()))
            .unwrap();
        assert_eq!(output.text, "No changes.");
        assert_eq!(output.structured.unwrap()["files_changed"], json!(0));
    }

    #[test]
    fn git_diff_stat_only_omits_the_patch() {
        let Some(dir) = init_repo() else { return };
        fs::write(dir.path().join("README.md"), "hello\nworld\n").unwrap();

        let output = GitDiffTool
            .call(&json!({"stat_only": true}), &ctx_for(dir.path()))
            .unwrap();
        assert!(output.text.contains("README.md"));
        assert!(!output.text.contains("+world"));
    }

    #[test]
    fn git_diff_truncates_with_marker() {
        let Some(dir) = init_repo() else { return };
        fs::write(dir.path().join("README.md"), "hello\nworld\n").unwrap();

        let output = GitDiffTool
            .call(&json!({"max_bytes": 10}), &ctx_for(dir.path()))
            .unwrap();
        assert!(output.text.contains("diff truncated"));
        assert_eq!(output.structured.unwrap()["truncated"], json!(true));
    }

    #[test]
    fn git_diff_rejects_secret_paths() {
        let Some(dir) = init_repo() else { return };
        fs::write(dir.path().join(".env"), "TOKEN=value\n").unwrap();

        let error = GitDiffTool
            .call(&json!({"paths": [".env"]}), &ctx_for(dir.path()))
            .unwrap_err();
        assert!(error.contains("path not allowed"), "{error}");
    }

    #[test]
    fn git_diff_redacts_secrets_in_output() {
        let Some(dir) = init_repo() else { return };
        fs::write(
            dir.path().join("README.md"),
            "hello\nAPI_KEY=abcdef123456\n",
        )
        .unwrap();

        let output = GitDiffTool.call(&json!({}), &ctx_for(dir.path())).unwrap();
        assert!(output.text.contains("API_KEY"));
        assert!(!output.text.contains("abcdef123456"), "{}", output.text);
    }

    #[test]
    fn git_diff_rejects_malformed_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(dir.path());
        assert!(GitDiffTool.call(&json!({"staged": "yes"}), &ctx).is_err());
        assert!(GitDiffTool.call(&json!({"paths": [1]}), &ctx).is_err());
        assert!(GitDiffTool.call(&json!({"max_bytes": -1}), &ctx).is_err());
    }

    #[test]
    fn run_verification_without_steps_explains_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let output = RunVerificationTool
            .call(&json!({}), &ctx_for(dir.path()))
            .unwrap();
        assert!(!output.is_error);
        assert!(output.text.contains("[[verification.steps]]"));
        assert_eq!(output.structured.unwrap()["passed"], json!(false));
    }

    #[test]
    fn run_verification_unknown_step_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_for(dir.path());
        ctx.verification.steps.push(VerificationStep {
            name: Some("tests".to_owned()),
            command: "echo ok".to_owned(),
            timeout_ms: None,
            cwd: None,
        });
        let error = RunVerificationTool
            .call(&json!({"steps": ["lint"]}), &ctx)
            .unwrap_err();
        assert!(error.contains("unknown verification step: lint"), "{error}");
    }

    #[test]
    fn execute_captures_output_and_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let run = execute("echo hello", dir.path(), Duration::from_secs(30)).unwrap();
        assert!(!run.timed_out);
        assert!(run.exit.is_some_and(|exit| exit.success()));
        assert!(String::from_utf8_lossy(&run.output).contains("hello"));

        let run = execute("exit 3", dir.path(), Duration::from_secs(30)).unwrap();
        assert_eq!(run.exit.and_then(|exit| exit.code()), Some(3));
    }

    #[test]
    fn execute_kills_a_step_that_exceeds_its_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let command = if cfg!(windows) {
            "ping -n 6 127.0.0.1 > NUL"
        } else {
            "sleep 5"
        };
        let started = Instant::now();
        let run = execute(command, dir.path(), Duration::from_millis(100)).unwrap();
        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    #[ignore = "needs command_risk::assess_command"]
    fn run_verification_passing_and_failing_steps() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_for(dir.path());
        ctx.verification.steps = vec![
            VerificationStep {
                name: Some("pass".to_owned()),
                command: "echo hello".to_owned(),
                timeout_ms: Some(30_000),
                cwd: None,
            },
            VerificationStep {
                name: Some("fail".to_owned()),
                command: "exit 1".to_owned(),
                timeout_ms: Some(30_000),
                cwd: None,
            },
        ];
        let output = RunVerificationTool.call(&json!({}), &ctx).unwrap();
        assert!(!output.is_error, "a failing step is not a tool error");
        let structured = output.structured.unwrap();
        assert_eq!(structured["passed"], json!(false));
        assert_eq!(structured["steps"][0]["status"], json!("passed"));
        assert_eq!(structured["steps"][0]["exit_code"], json!(0));
        assert_eq!(structured["steps"][1]["status"], json!("failed"));
        assert_eq!(structured["steps"][1]["exit_code"], json!(1));
    }

    #[test]
    #[ignore = "needs command_risk::assess_command"]
    fn run_verification_blocks_risky_steps() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_for(dir.path());
        ctx.verification.steps = vec![VerificationStep {
            name: Some("wipe".to_owned()),
            command: "rm -rf x".to_owned(),
            timeout_ms: None,
            cwd: None,
        }];
        let output = RunVerificationTool.call(&json!({}), &ctx).unwrap();
        assert_eq!(
            output.structured.unwrap()["steps"][0]["status"],
            json!("blocked")
        );
    }

    #[test]
    fn harness_info_lists_steps_and_never_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_for(dir.path());
        ctx.verification.steps.push(VerificationStep {
            name: None,
            command: "echo ok".to_owned(),
            timeout_ms: None,
            cwd: None,
        });
        let output = HarnessInfoTool.call(&json!({}), &ctx).unwrap();
        let structured = output.structured.unwrap();
        assert_eq!(structured["product"], json!("harness"));
        assert_eq!(structured["version"], json!(env!("CARGO_PKG_VERSION")));
        assert_eq!(structured["profile"], json!("local-only"));
        assert_eq!(structured["models"], json!([]));
        assert_eq!(structured["verification_steps"], json!(["step-1"]));
        assert!(output.text.contains("step-1"));
    }

    #[test]
    fn default_tools_are_in_documented_order() {
        let names: Vec<&str> = default_tools().iter().map(|tool| tool.name()).collect();
        assert_eq!(
            names,
            ["git_status", "git_diff", "run_verification", "harness_info"]
        );
    }
}
