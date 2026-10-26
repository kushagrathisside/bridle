//! The harness's MCP tools. M1 ships `git_status`, `git_diff`, `run_verification` and
//! `harness_info`; M2 adds the deterministic repository tools.
//!
//! IMPLEMENTATION SPEC (delegated). Each tool is a struct implementing [`McpTool`]; all run git or
//! commands with `std::process::Command` in `ctx.workspace`, never through a shell, except
//! verification steps (see below). Outputs are redacted with `ctx.redactor` before returning.
//!
//! Bounds: every git call is read-only (`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`) and is
//! killed after [`GIT_TIMEOUT`]. Output is captured through bounded buffers and never held whole.
//! Structured path lists are capped at [`LIST_LIMIT`] entries, with full counts beside them.
//!
//! - `git_status` (no args): runs `git status --porcelain=v2 --branch -z`; parse with the pure
//!   `parse_porcelain_v2(&[u8]) -> GitStatus { branch, upstream, ahead, behind, staged: Vec<String>,
//!   unstaged: Vec<String>, untracked: Vec<String>, conflicted: Vec<String> }` (handle `1`, `2`
//!   (rename: path + orig path), `u`, `?` records and `# branch.*` headers). `text` = a compact
//!   human summary; `structured` = the JSON of `GitStatus`. Not a git repo → `Err`.
//! - `git_diff` args `{ "staged": bool = false, "paths": [string] = [], "stat_only": bool = false,
//!   "max_bytes": int = 60000 }` (clamped to `MAX_DIFF_BYTES`): every path must pass the read
//!   policy (`check_read_path`; Deny → `Err`). The scope is `paths`, or `.` when none are given.
//!   The changed names in that scope are listed first (`git diff [--cached] --name-only -z
//!   --relative -- <scope>`, at most `NAME_LIST_LIMIT_BYTES`, else `Err`) and each is checked with
//!   the same read policy. The shortstat, stat and patch then run on the scope plus one
//!   `:(exclude,literal)` pathspec per excluded name. Every diff is `--relative`, so paths are
//!   limited to and printed relative to the workspace, which may be a subdirectory of the
//!   repository. Excluded names are reported in the text (`N file(s) excluded by policy`) and as `excluded`.
//!   The stat lists at most `LIST_LIMIT` files. The patch is redacted, then cut to `max_bytes` on
//!   a character boundary with a marker. `structured` = `{ files_changed, insertions, deletions,
//!   truncated, excluded }`; the totals come from the shortstat line (pure helper
//!   `parse_shortstat`).
//! - `run_verification` args `{ "steps": [string] (optional names to run; default all) }`: runs
//!   `ctx.verification.steps` sequentially. BEFORE running each step, classify its command with
//!   `ctx.policy.assess(cmd)` → if the verdict (`ctx.policy.class_verdict`) is not Allow, skip
//!   it with status `blocked` and the reasons (verification must never smuggle risky commands).
//!   Run allowed steps through the platform shell (`cmd /C` on Windows, `sh -c` elsewhere) with
//!   `cwd = workspace/step.cwd`, a timeout (step.timeout_ms or `default_timeout_ms`) enforced by
//!   polling `try_wait` and killing on expiry, and capture stdout+stderr. Result per step: `{ name,
//!   command, status: passed|failed|timeout|blocked, exit_code, duration_ms, output_tail }` where
//!   `output_tail` is the last 2000 bytes of stdout and `stderr_tail` the last 2000 bytes of
//!   stderr, both redacted. `structured` = `{ passed: bool, steps: [...] }`;
//!   `is_error` = false even when steps fail (failure is information, not a tool error). No steps
//!   configured → text explains how to add `[[verification.steps]]`.
//! - `harness_info` (no args): `structured` = `{ product, version, workspace, profile, models: [{slot,
//!   id, role, endpoint, context}], verification_steps: [names] }`, never including secrets.
//! - `default_tools()` returns all four in that order.
//! - Tests: `parse_porcelain_v2` and `parse_shortstat` fixtures; `git_status`/`git_diff` against a
//!   temp repo (`git init`, commit, modify) — skip gracefully if `git` is missing;
//!   `run_verification` with passing, failing, blocked (`rm -rf x`) and timeout (`sleep 5` with a
//!   100 ms timeout; on Windows use `ping -n 6 127.0.0.1 > NUL`) steps.

use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use harness_core::config::{Config, ModelRole, Verdict, VerificationSection, VerificationStep};
use harness_core::failures::{Failure, parse_failures};
use harness_core::security::command_risk::ShellKind;
use harness_core::security::redact::Redactor;
use harness_core::security::tool_policy::{ToolKind, ToolPolicy, ToolRequest};
use serde::Serialize;
use serde_json::{Value, json};

const PRODUCT_NAME: &str = "harness";
const DEFAULT_DIFF_MAX_BYTES: u64 = 60_000;
/// The largest patch `git_diff` returns; a larger `max_bytes` is clamped to this.
const MAX_DIFF_BYTES: u64 = 1_000_000;
/// Bytes read past the patch limit, so a secret that straddles the cut is redacted whole.
const DIFF_READ_MARGIN: usize = 4_096;
/// The most bytes of changed-file names `git_diff` will read; a longer listing is refused.
const NAME_LIST_LIMIT_BYTES: usize = 4 * 1024 * 1024;
/// Bytes of each output stream reported for a verification step (the tail).
const OUTPUT_TAIL_BYTES: usize = 2_000;
/// Parsed failures listed per step in the text result; all of them are in the structured result.
const FAILURES_IN_TEXT: usize = 10;
/// Bytes kept per output stream while a step runs; only the tail is ever reported.
const CAPTURE_LIMIT_BYTES: usize = 64 * 1024;
/// Entries listed per category in the `git_status` summary and structured lists, and in the
/// `git_diff` stat listing and exclusion note.
const LIST_LIMIT: usize = 50;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long to wait for output readers after a process exits, so a grandchild that inherited the
/// pipes cannot hold the tool call open.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
/// How long a read-only git call may run before it is killed.
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

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
        Box::new(history::GitHistoryTool),
        Box::new(history::GitLogRangeTool),
        Box::new(RunVerificationTool),
        Box::new(verify::RUN_TESTS),
        Box::new(verify::RUN_LINTER),
        Box::new(verify::RUN_TYPECHECKER),
        Box::new(failures::ParseTestFailureTool),
        Box::new(code::RepoMapTool),
        Box::new(code::FileSummaryTool),
        Box::new(code::SymbolSearchTool),
        Box::new(code::FindReferencesTool),
        Box::new(deps::DependencySearchTool),
        Box::new(artifacts::InspectArtifactTool),
        Box::new(artifacts::SummarizeExperimentTool),
        Box::new(artifacts::CompareExperimentsTool),
        Box::new(run::RunCommandTool),
        Box::new(run::InspectConfigTool),
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

/// Runs a read-only `git` command in the workspace and returns its stdout, at most `stdout_limit`
/// bytes. Git never prompts or takes optional locks. `usize::MAX` reads everything.
fn run_git(ctx: &ToolContext, args: &[&str], stdout_limit: usize) -> Result<Vec<u8>, String> {
    let mut git = Command::new("git");
    git.args(args)
        .current_dir(&ctx.workspace)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    let subcommand = args.first().copied().unwrap_or_default();
    run_git_command(ctx, git, subcommand, stdout_limit, GIT_TIMEOUT)
}

/// Runs a prepared git `command` under `timeout`. Errors and timeouts are redacted. Reaching
/// `stdout_limit` closes the pipe, so git stops writing, and the output so far is returned.
fn run_git_command(
    ctx: &ToolContext,
    command: Command,
    subcommand: &str,
    stdout_limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let run = run_child(command, timeout, Keep::Head(stdout_limit)).map_err(|error| {
        ctx.redactor
            .redact(&format!("failed to run git: {error}"))
            .into_owned()
    })?;
    // A full buffer ends the read on purpose, so the exit status says nothing about success.
    let full = run.stdout.len() >= stdout_limit;
    if run.timed_out && !full {
        return Err(ctx
            .redactor
            .redact(&format!(
                "git {subcommand} timed out after {} s",
                timeout.as_secs()
            ))
            .into_owned());
    }
    if full || run.exit.is_some_and(|exit| exit.success()) {
        return Ok(run.stdout);
    }
    let stderr = tail_text(&lossy(&run.stderr), OUTPUT_TAIL_BYTES);
    Err(ctx
        .redactor
        .redact(&format!("git {subcommand} failed: {}", stderr.trim()))
        .into_owned())
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
        let stdout = run_git(
            ctx,
            &["status", "--porcelain=v2", "--branch", "-z"],
            usize::MAX,
        )?;
        let status = parse_porcelain_v2(&stdout);
        Ok(redacted_output(
            ctx,
            &render_status(&status),
            Some(status_structured(&status)),
        ))
    }
}

/// The `git_status` structured content: branch and counts, plus each path list capped at
/// [`LIST_LIMIT`]. A list keeps its key; `<key>_count` carries the full length.
fn status_structured(status: &GitStatus) -> Value {
    let mut value = json!({
        "branch": status.branch,
        "upstream": status.upstream,
        "ahead": status.ahead,
        "behind": status.behind,
    });
    let lists = [
        ("staged", &status.staged),
        ("unstaged", &status.unstaged),
        ("untracked", &status.untracked),
        ("conflicted", &status.conflicted),
    ];
    for (key, paths) in lists {
        value[key] = json!(&paths[..paths.len().min(LIST_LIMIT)]);
        value[format!("{key}_count")] = json!(paths.len());
    }
    value
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
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Limit the diff to these paths. Files the read policy denies (secret files) are excluded and only counted."},
                "stat_only": {"type": "boolean", "description": "Return only the file summary.", "default": false},
                "max_bytes": {"type": "integer", "minimum": 0, "description": format!("Truncate the patch to this many bytes. Larger values are clamped to {MAX_DIFF_BYTES}."), "default": DEFAULT_DIFF_MAX_BYTES},
            },
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let staged = bool_arg(args, "staged", false)?;
        let stat_only = bool_arg(args, "stat_only", false)?;
        let requested = u64_arg(args, "max_bytes", DEFAULT_DIFF_MAX_BYTES)?;
        let max_bytes = usize::try_from(requested.min(MAX_DIFF_BYTES)).unwrap_or(usize::MAX);
        let paths = optional_string_list(args, "paths")?.unwrap_or_default();
        for path in &paths {
            check_read_path(ctx, path)?;
        }
        // The requested scope: the user's paths, or the whole workspace when none were given.
        let scope: Vec<&str> = if paths.is_empty() {
            vec!["."]
        } else {
            paths.iter().map(String::as_str).collect()
        };

        // The policy decides per changed file. The listing is read whole up to a fixed bound: a
        // partial listing would leave files unchecked, so an overlong one is an error instead.
        let names_raw = run_git(
            ctx,
            &diff_args(&["--name-only", "-z"], staged, &scope),
            NAME_LIST_LIMIT_BYTES,
        )?;
        if names_raw.len() >= NAME_LIST_LIMIT_BYTES {
            return Err(
                "too many changed files to check; pass paths to narrow the diff".to_owned(),
            );
        }
        let (allowed, excluded) = split_by_policy(ctx, &names_raw);

        let mut sections: Vec<String> = Vec::new();
        let mut stat = DiffStat::default();
        let mut truncated = false;
        // Nothing allowed means nothing to show, so no diff runs at all.
        if !allowed.is_empty() {
            // The scope minus each excluded file, as exclude pathspecs. Few names are excluded
            // (secret files), so the command line stays short.
            // A file that changes between the listing and this diff is not checked; that race is
            // accepted, since such a file is only shown if it is in the requested scope.
            let excludes: Vec<String> = excluded
                .iter()
                .map(|name| format!(":(exclude,literal){name}"))
                .collect();
            let mut scoped = scope.clone();
            scoped.extend(excludes.iter().map(String::as_str));

            let shortstat = lossy(&run_git(
                ctx,
                &diff_args(&["--shortstat"], staged, &scoped),
                usize::MAX,
            )?);
            stat = parse_shortstat(&shortstat);
            let stat_limit = usize::try_from(MAX_DIFF_BYTES).unwrap_or(usize::MAX);
            let stat_raw = lossy(&run_git(
                ctx,
                &diff_args(&["--stat"], staged, &scoped),
                stat_limit.saturating_add(DIFF_READ_MARGIN),
            )?);
            sections.push(render_stat(&stat_raw, &shortstat, stat.files_changed));

            if !stat_only {
                // The read stops just past the limit; `capped` marks a patch cut short by it.
                let limit = max_bytes.saturating_add(DIFF_READ_MARGIN);
                let patch_bytes = run_git(ctx, &diff_args(&[], staged, &scoped), limit)?;
                let capped = patch_bytes.len() >= limit;
                // Redact before truncating so a secret cannot survive by being cut in half.
                let patch_text = lossy(&patch_bytes);
                let redacted = ctx.redactor.redact(&patch_text);
                let (patch, cut) = truncate_text(&redacted, max_bytes, capped);
                truncated = cut;
                sections.push(patch);
            }
        }
        sections.retain(|section| !section.is_empty());
        if sections.is_empty() {
            sections.push("No changes.".to_owned());
        }
        if !excluded.is_empty() {
            sections.push(exclusion_note(&excluded));
        }

        Ok(redacted_output(
            ctx,
            &sections.join("\n\n"),
            Some(json!({
                "files_changed": stat.files_changed,
                "insertions": stat.insertions,
                "deletions": stat.deletions,
                "truncated": truncated,
                "excluded": excluded.len(),
            })),
        ))
    }
}

/// Arguments for one `git diff` call: the flags every diff of this tool shares (no external or
/// textconv drivers, no rename pairing, paths relative to and limited to the workspace), then
/// `--cached` when `staged`, then `--` and `scope`.
fn diff_args<'a>(extra: &[&'a str], staged: bool, scope: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["diff"];
    args.extend_from_slice(extra);
    args.extend([
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--relative",
    ]);
    if staged {
        args.push("--cached");
    }
    if !scope.is_empty() {
        args.push("--");
        args.extend_from_slice(scope);
    }
    args
}

/// Splits the NUL-separated names git listed into the names the read policy allows and the names
/// it excludes. Each name is checked exactly as a `paths` entry is.
fn split_by_policy(ctx: &ToolContext, names_raw: &[u8]) -> (Vec<String>, Vec<String>) {
    let mut allowed = Vec::new();
    let mut excluded = Vec::new();
    for name in names_raw
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(lossy)
    {
        if check_read_path(ctx, &name).is_ok() {
            allowed.push(name);
        } else {
            excluded.push(name);
        }
    }
    (allowed, excluded)
}

/// The stat listing: at most [`LIST_LIMIT`] file lines, a `... and N more` marker, then the
/// summary line. `files` is the full count from the shortstat line.
fn render_stat(stat_raw: &str, shortstat: &str, files: u64) -> String {
    let shown = usize::try_from(files).unwrap_or(usize::MAX).min(LIST_LIMIT);
    let mut out: Vec<String> = stat_raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(shown)
        .map(str::to_owned)
        .collect();
    if files > LIST_LIMIT as u64 {
        out.push(format!(" ... and {} more", files - LIST_LIMIT as u64));
    }
    if !shortstat.trim().is_empty() {
        out.push(shortstat.trim().to_owned());
    }
    out.join("\n")
}

/// The note that names the files the read policy left out of the diff.
fn exclusion_note(excluded: &[String]) -> String {
    let mut note = format!("{} file(s) excluded by policy:", excluded.len());
    for name in excluded.iter().take(LIST_LIMIT) {
        note.push_str("\n  ");
        note.push_str(name);
    }
    if excluded.len() > LIST_LIMIT {
        note.push_str(&format!("\n  ... and {} more", excluded.len() - LIST_LIMIT));
    }
    note
}

/// Cuts `text` to at most `max_bytes` (on a character boundary) and appends a marker when it cut.
/// `incomplete` says `text` is only the start of a longer output, so it is marked even when short.
fn truncate_text(text: &str, max_bytes: usize, incomplete: bool) -> (String, bool) {
    if text.len() <= max_bytes && !incomplete {
        return (text.to_owned(), false);
    }
    let mut end = max_bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text.get(..end).unwrap_or_default().to_owned();
    out.push_str(&format!(
        "\n[... diff truncated after {end} bytes; raise max_bytes or narrow paths ...]\n"
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
    /// The last [`OUTPUT_TAIL_BYTES`] of stdout, redacted.
    output_tail: String,
    /// The last [`OUTPUT_TAIL_BYTES`] of stderr, redacted.
    stderr_tail: String,
    /// Failures parsed from the step's captured output when it did not pass (AC-A5).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    failures: Vec<Failure>,
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
            .map(|(name, step)| run_step(ctx, name, step, &[]))
            .collect();
        report_steps("Verification", &results)
    }
}

/// The text and structured result for a set of step results, under a label such as `Tests`.
fn report_steps(label: &str, results: &[StepResult]) -> Result<ToolOutput, String> {
    let passing = results.iter().filter(|step| step.status == PASSED).count();
    let passed = passing == results.len();

    let mut text = format!(
        "{label} {}: {passing} of {} steps passed.",
        if passed { "passed" } else { "failed" },
        results.len()
    );
    for step in results {
        let exit = step
            .exit_code
            .map(|code| format!(", exit {code}"))
            .unwrap_or_default();
        text.push_str(&format!(
            "\n- {}: {}{exit} ({} ms)",
            step.name, step.status, step.duration_ms
        ));
        // Parsed failures first: they say where to look without reading the log.
        for failure in step.failures.iter().take(FAILURES_IN_TEXT) {
            text.push_str(&format!("\n  x {}", failures::describe(failure)));
        }
        if step.failures.len() > FAILURES_IN_TEXT {
            text.push_str(&format!(
                "\n  ... and {} more failures (see the structured result)",
                step.failures.len() - FAILURES_IN_TEXT
            ));
        }
        if step.status != PASSED {
            for line in step.output_tail.trim_end().lines() {
                text.push_str(&format!("\n    {line}"));
            }
            for line in step.stderr_tail.trim_end().lines() {
                text.push_str(&format!("\n    {line}"));
            }
        }
    }

    let structured = json!({
        "passed": passed,
        "steps": serde_json::to_value(results).map_err(|error| error.to_string())?,
    });
    Ok(ToolOutput {
        text,
        structured: Some(structured),
        is_error: false,
    })
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

/// Classifies one step, with any `extra` arguments appended, then runs it unless the policy
/// refuses it. Extra arguments are already checked by the caller; they are classified with the
/// command all the same.
fn run_step(
    ctx: &ToolContext,
    name: String,
    step: &VerificationStep,
    extra: &[String],
) -> StepResult {
    let command = if extra.is_empty() {
        step.command.clone()
    } else {
        format!("{} {}", step.command, extra.join(" "))
    };
    let assessment = ctx.policy.assess_in(&command, STEP_SHELL);
    let verdict = ctx.policy.class_verdict(assessment.class);
    if !matches!(verdict, Verdict::Allow) {
        let reasons = if assessment.reasons.is_empty() {
            format!("command class {}", assessment.class.as_str())
        } else {
            assessment.reasons.join("; ")
        };
        return StepResult {
            name,
            command,
            status: BLOCKED,
            exit_code: None,
            duration_ms: 0,
            output_tail: ctx
                .redactor
                .redact(&format!("not run: {reasons}"))
                .into_owned(),
            stderr_tail: String::new(),
            failures: Vec::new(),
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
    let outcome = execute(&command, &cwd, Duration::from_millis(timeout_ms));
    let duration_ms = elapsed_ms(started);

    let (status, exit_code, stdout, stderr) = match outcome {
        Ok(run) if run.timed_out => (TIMEOUT, None, run.stdout, run.stderr),
        Ok(run) => {
            let success = run.exit.as_ref().is_some_and(ExitStatus::success);
            let status = if success { PASSED } else { FAILED };
            (
                status,
                run.exit.and_then(|exit| exit.code()),
                run.stdout,
                run.stderr,
            )
        }
        Err(error) => (FAILED, None, error.into_bytes(), Vec::new()),
    };
    // Parse the whole captured output (up to the capture limit per stream), not only the tails.
    let failures = if status == PASSED {
        Vec::new()
    } else {
        let mut both = lossy(&stdout);
        both.push('\n');
        both.push_str(&lossy(&stderr));
        parse_failures(&both).failures
    };
    // Redact each whole stream before cutting its tail, so no secret is shown in halves.
    StepResult {
        name,
        command,
        status,
        exit_code,
        duration_ms,
        output_tail: tail_text(&ctx.redactor.redact(&lossy(&stdout)), OUTPUT_TAIL_BYTES),
        stderr_tail: tail_text(&ctx.redactor.redact(&lossy(&stderr)), OUTPUT_TAIL_BYTES),
        failures,
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The last `limit` bytes of `text`, starting on a character boundary.
fn tail_text(text: &str, limit: usize) -> String {
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.get(start..).unwrap_or_default().to_owned()
}

/// The grammar of the shell that [`shell_command`] runs; steps are classified in it.
#[cfg(windows)]
const STEP_SHELL: ShellKind = ShellKind::Cmd;
/// The grammar of the shell that [`shell_command`] runs; steps are classified in it.
#[cfg(not(windows))]
const STEP_SHELL: ShellKind = ShellKind::Posix;

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

/// What happened when a command was run.
struct Execution {
    /// The exit status, if the process was reaped.
    exit: Option<ExitStatus>,
    /// Whether the timeout expired and the process was killed.
    timed_out: bool,
    /// The captured stdout, as the [`Keep`] it was run with directs.
    stdout: Vec<u8>,
    /// The last [`CAPTURE_LIMIT_BYTES`] of stderr.
    stderr: Vec<u8>,
}

/// Which part of an output stream a capture keeps.
#[derive(Debug, Clone, Copy)]
enum Keep {
    /// The last `n` bytes; older output is discarded as it arrives.
    Tail(usize),
    /// The first `n` bytes; once they are read the pipe is closed, so the process can not grow
    /// the buffer and is stopped by the closed pipe.
    Head(usize),
}

/// Runs `command` through the platform shell in `cwd`, killing it once `timeout` expires.
fn execute(command: &str, cwd: &Path, timeout: Duration) -> Result<Execution, String> {
    let mut shell = shell_command(command);
    shell.current_dir(cwd);
    run_child(shell, timeout, Keep::Tail(CAPTURE_LIMIT_BYTES))
        .map_err(|error| format!("failed to run step: {error}"))
}

/// Spawns `command` with piped output, waits for it under `timeout` and kills its whole tree on
/// expiry. stdout is kept as `stdout_keep` directs; stderr keeps its tail.
fn run_child(mut command: Command, timeout: Duration, stdout_keep: Keep) -> io::Result<Execution> {
    // Own process group, so a timeout can kill the children too (see `kill_tree`).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let (done_tx, done_rx) = mpsc::channel::<()>();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let mut readers = 0_usize;
    if let Some(pipe) = child.stdout.take() {
        capture(pipe, Arc::clone(&stdout), stdout_keep, done_tx.clone());
        readers += 1;
    }
    if let Some(pipe) = child.stderr.take() {
        capture(
            pipe,
            Arc::clone(&stderr),
            Keep::Tail(CAPTURE_LIMIT_BYTES),
            done_tx.clone(),
        );
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
                if let Err(error) = kill_tree(child.id()) {
                    eprintln!("harness mcp: {error}");
                }
                let _ = child.kill();
                break (child.wait().ok(), true);
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    };

    for _ in 0..readers {
        let _ = done_rx.recv_timeout(DRAIN_TIMEOUT);
    }
    Ok(Execution {
        exit,
        timed_out,
        stdout: snapshot(&stdout),
        stderr: snapshot(&stderr),
    })
}

/// Kills a process tree, not just its root: a shell's children (for example `npm test` under
/// `cmd /C`) would otherwise keep running after a timeout. Windows uses `taskkill /T`; Unix kills
/// the process group the child was started in. Errors only when the killer cannot be run; a
/// non-zero killer status (the tree already exited) is not an error.
fn kill_tree(pid: u32) -> Result<(), String> {
    run_killer(kill_command(pid))
}

/// The command that kills the tree rooted at `pid`.
#[cfg(windows)]
fn kill_command(pid: u32) -> Command {
    let mut killer = Command::new("taskkill");
    killer.args(["/PID", &pid.to_string(), "/T", "/F"]);
    killer
}

/// The command that kills the process group `pid`.
#[cfg(unix)]
fn kill_command(pid: u32) -> Command {
    let mut killer = Command::new("kill");
    killer.args(["-KILL", &format!("-{pid}")]);
    killer
}

/// Runs a killer quietly, reporting only a failure to start it.
fn run_killer(mut killer: Command) -> Result<(), String> {
    killer
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|_| ())
        .map_err(|error| format!("failed to run the process killer: {error}"))
}

/// Copies what a capture buffer holds so far.
fn snapshot(sink: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    sink.lock()
        .map(|buffer| buffer.to_vec())
        .unwrap_or_default()
}

/// Reads `pipe` on a background thread into `sink`, keeping what `keep` says. Sends on `done`
/// once the pipe is closed or the head limit has been read.
fn capture<R: Read + Send + 'static>(
    mut pipe: R,
    sink: Arc<Mutex<Vec<u8>>>,
    keep: Keep,
    done: Sender<()>,
) {
    thread::spawn(move || {
        let mut chunk = [0_u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let Ok(mut buffer) = sink.lock() else {
                        break;
                    };
                    match keep {
                        Keep::Tail(limit) => {
                            buffer.extend_from_slice(&chunk[..read]);
                            let excess = buffer.len().saturating_sub(limit);
                            buffer.drain(..excess);
                        }
                        Keep::Head(limit) => {
                            let room = limit.saturating_sub(buffer.len());
                            buffer.extend_from_slice(&chunk[..read.min(room)]);
                            if buffer.len() >= limit {
                                break;
                            }
                        }
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
pub mod adapters;
pub mod artifacts;
pub mod code;
pub mod deps;
pub mod failures;
pub mod history;
pub mod run;
pub mod task;
pub mod verify;

#[cfg(test)]
mod test_support;

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

    use harness_core::config::StepKind;

    use super::test_support::{commit_files, git, git_available, init_repo};
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
        let (text, truncated) = truncate_text("ééé", 3, false);
        assert!(truncated);
        assert!(text.starts_with('é'));
        assert!(text.contains("truncated"));
        assert_eq!(
            truncate_text("short", 60_000, false),
            ("short".to_owned(), false)
        );
        // A capped read is marked even when the redacted text came out short.
        let (text, truncated) = truncate_text("short", 60_000, true);
        assert!(truncated && text.contains("truncated"), "{text}");
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
            kind: StepKind::Test,
            targets: false,
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
        assert!(String::from_utf8_lossy(&run.stdout).contains("hello"));

        let run = execute("exit 3", dir.path(), Duration::from_secs(30)).unwrap();
        assert_eq!(run.exit.and_then(|exit| exit.code()), Some(3));
    }

    #[test]
    fn execute_kills_a_step_that_exceeds_its_timeout() {
        let dir = tempfile::tempdir().unwrap();
        // Long enough that a step left running would miss the bound by a wide margin, even on a busy machine.
        let command = if cfg!(windows) {
            "ping -n 30 127.0.0.1 > NUL"
        } else {
            "sleep 30"
        };
        let started = Instant::now();
        let run = execute(command, dir.path(), Duration::from_millis(100)).unwrap();
        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn run_verification_passing_and_failing_steps() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_for(dir.path());
        ctx.verification.steps = vec![
            VerificationStep {
                name: Some("pass".to_owned()),
                command: "echo hello".to_owned(),
                timeout_ms: Some(30_000),
                cwd: None,
                kind: StepKind::Test,
                targets: false,
            },
            VerificationStep {
                name: Some("fail".to_owned()),
                // `pytest` is EXECUTE_SAFE, so the step is run, and an unknown option makes it exit non-zero.
                command: "pytest --no-such-option-xyz".to_owned(),
                timeout_ms: Some(30_000),
                cwd: None,
                kind: StepKind::Test,
                targets: false,
            },
        ];
        let output = RunVerificationTool.call(&json!({}), &ctx).unwrap();
        assert!(!output.is_error, "a failing step is not a tool error");
        let structured = output.structured.unwrap();
        assert_eq!(structured["passed"], json!(false));
        assert_eq!(structured["steps"][0]["status"], json!("passed"));
        assert_eq!(structured["steps"][0]["exit_code"], json!(0));
        assert_eq!(structured["steps"][1]["status"], json!("failed"));
        // Exact code depends on whether pytest is installed (127 when missing); non-zero is the contract.
        assert_ne!(structured["steps"][1]["exit_code"], json!(0));
    }

    #[test]
    fn run_verification_blocks_risky_steps() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_for(dir.path());
        ctx.verification.steps = vec![VerificationStep {
            name: Some("wipe".to_owned()),
            command: "rm -rf x".to_owned(),
            timeout_ms: None,
            cwd: None,
            kind: StepKind::Test,
            targets: false,
        }];
        let output = RunVerificationTool.call(&json!({}), &ctx).unwrap();
        assert_eq!(
            output.structured.unwrap()["steps"][0]["status"],
            json!("blocked")
        );
    }

    #[test]
    fn a_failed_step_returns_its_parsed_failures() {
        let has_pytest = Command::new("python")
            .args(["-m", "pytest", "--version"])
            .output()
            .is_ok_and(|out| out.status.success());
        if !has_pytest {
            eprintln!("pytest not available; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("test_calc.py"),
            "def add(a, b):\n    return a - b\n\n\ndef test_add():\n    assert add(1, 2) == 3\n",
        )
        .unwrap();
        let mut ctx = ctx_for(dir.path());
        ctx.verification.steps = vec![VerificationStep {
            name: Some("pytest".to_owned()),
            command: "python -m pytest -q test_calc.py".to_owned(),
            timeout_ms: None,
            cwd: None,
            kind: StepKind::Test,
            targets: false,
        }];
        let output = RunVerificationTool.call(&json!({}), &ctx).unwrap();
        let step = output.structured.unwrap()["steps"][0].clone();
        assert_eq!(step["status"], json!("failed"), "{step}");
        assert_eq!(
            step["failures"][0]["name"],
            json!("test_calc.py::test_add"),
            "{step}"
        );
        assert_eq!(step["failures"][0]["line"], json!(6), "{step}");
        assert!(
            output
                .text
                .contains("x test_calc.py::test_add at test_calc.py:6"),
            "{}",
            output.text
        );
    }

    /// Steps run under `cmd /C` on Windows and `sh -c` elsewhere, whatever shell the host uses for
    /// the agent, so they are classified in that grammar. In cmd a single quote does not quote,
    /// so this step deletes a directory; in PowerShell it would only print text.
    #[test]
    fn verification_steps_are_classified_in_the_shell_that_runs_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_for(dir.path());
        let command = if cfg!(windows) {
            "echo ' & rd /s /q build & '"
        } else {
            "echo \"$(rm -rf build)\""
        };
        ctx.verification.steps = vec![VerificationStep {
            name: Some("sneaky".to_owned()),
            command: command.to_owned(),
            timeout_ms: None,
            cwd: None,
            kind: StepKind::Test,
            targets: false,
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
            kind: StepKind::Test,
            targets: false,
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
            [
                "git_status",
                "git_diff",
                "git_history",
                "git_log_range",
                "run_verification",
                "run_tests",
                "run_linter",
                "run_typechecker",
                "parse_test_failure",
                "repo_map",
                "file_summary",
                "symbol_search",
                "find_references",
                "dependency_search",
                "inspect_artifact",
                "summarize_experiment",
                "compare_experiments",
                "run_command",
                "inspect_config",
                "harness_info"
            ]
        );
    }

    #[test]
    fn git_diff_excludes_policy_denied_files_in_every_scope() {
        let Some(dir) = init_repo() else { return };
        commit_files(
            dir.path(),
            &[
                (".env", "TOKEN=old\n"),
                ("config/prod.pem", "old key\n"),
                ("config/calc.py", "def add(a, b):\n    return a + b\n"),
            ],
        );
        fs::write(dir.path().join(".env"), "sentinel-env-9317\n").unwrap();
        fs::write(dir.path().join("config/prod.pem"), "sentinel-pem-9317\n").unwrap();
        fs::write(
            dir.path().join("config/calc.py"),
            "def add(a, b):\n    return a + b  # changed\n",
        )
        .unwrap();
        let ctx = ctx_for(dir.path());

        let cases = [
            (json!({}), 2),
            (json!({"paths": ["."]}), 2),
            (json!({"paths": ["config"]}), 1),
        ];
        for (args, excluded) in cases {
            let output = GitDiffTool.call(&args, &ctx).unwrap();
            let structured = output.structured.clone().unwrap();
            assert!(
                !output.text.contains("sentinel-"),
                "{args}: {}",
                output.text
            );
            assert!(
                !structured.to_string().contains("sentinel-"),
                "{args}: {structured}"
            );
            assert!(output.text.contains("# changed"), "{args}: {}", output.text);
            assert_eq!(structured["excluded"], json!(excluded), "{args}");
            assert!(
                output.text.contains("excluded by policy"),
                "{args}: {}",
                output.text
            );
        }
    }

    #[test]
    fn git_diff_with_only_excluded_changes_says_no_changes() {
        let Some(dir) = init_repo() else { return };
        commit_files(dir.path(), &[(".env", "TOKEN=old\n")]);
        fs::write(dir.path().join(".env"), "sentinel-env-9317\n").unwrap();

        let output = GitDiffTool.call(&json!({}), &ctx_for(dir.path())).unwrap();
        assert!(output.text.starts_with("No changes."), "{}", output.text);
        assert!(output.text.contains("1 file(s) excluded by policy"));
        assert!(!output.text.contains("sentinel-"), "{}", output.text);
        let structured = output.structured.unwrap();
        assert_eq!(structured["files_changed"], json!(0));
        assert_eq!(structured["excluded"], json!(1));
    }

    #[test]
    fn git_diff_is_limited_to_a_workspace_inside_the_repository() {
        let Some(dir) = init_repo() else { return };
        commit_files(
            dir.path(),
            &[
                ("outside.txt", "one\n"),
                ("sub/inside.txt", "one\n"),
                ("sub/.env", "TOKEN=old\n"),
            ],
        );
        fs::write(dir.path().join("outside.txt"), "sentinel-outside\n").unwrap();
        fs::write(dir.path().join("sub/inside.txt"), "two\n").unwrap();
        fs::write(dir.path().join("sub/.env"), "sentinel-env-sub\n").unwrap();
        let workspace = dir.path().join("sub");

        let output = GitDiffTool.call(&json!({}), &ctx_for(&workspace)).unwrap();
        assert!(output.text.contains("inside.txt"), "{}", output.text);
        assert!(!output.text.contains("outside"), "{}", output.text);
        assert!(!output.text.contains("sentinel-"), "{}", output.text);
        let structured = output.structured.unwrap();
        assert_eq!(structured["files_changed"], json!(1));
        assert_eq!(structured["excluded"], json!(1));
    }

    #[test]
    fn git_diff_caps_the_stat_listing_and_keeps_the_summary() {
        let Some(dir) = init_repo() else { return };
        let files: Vec<(String, String)> = (0..60)
            .map(|index| (format!("f{index:02}.txt"), "one\n".to_owned()))
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, content)| (path.as_str(), content.as_str()))
            .collect();
        commit_files(dir.path(), &refs);
        for (path, _) in &files {
            fs::write(dir.path().join(path), "two\n").unwrap();
        }

        let output = GitDiffTool
            .call(&json!({"stat_only": true}), &ctx_for(dir.path()))
            .unwrap();
        assert_eq!(
            output.text.matches(" | ").count(),
            LIST_LIMIT,
            "{}",
            output.text
        );
        assert!(output.text.contains("... and 10 more"), "{}", output.text);
        assert!(output.text.contains("60 files changed"), "{}", output.text);
        assert_eq!(output.structured.unwrap()["files_changed"], json!(60));
    }

    #[test]
    fn git_diff_clamps_max_bytes_and_bounds_the_read() {
        let Some(dir) = init_repo() else { return };
        let big = "added line of text\n".repeat(200_000);
        fs::write(dir.path().join("README.md"), format!("hello\n{big}")).unwrap();

        // Asks for far more than the server maximum (1_000_000 bytes).
        let output = GitDiffTool
            .call(
                &json!({"max_bytes": 1_099_511_627_776_u64}),
                &ctx_for(dir.path()),
            )
            .unwrap();
        assert!(output.text.len() < 1_000_500, "{}", output.text.len());
        assert!(output.text.contains("diff truncated"));
        assert_eq!(output.structured.unwrap()["truncated"], json!(true));
    }

    #[test]
    fn git_diff_truncation_never_exposes_a_cut_secret() {
        let Some(dir) = init_repo() else { return };
        fs::write(
            dir.path().join("README.md"),
            "hello\nAPI_KEY=abcdef123456\n",
        )
        .unwrap();
        let raw = Command::new("git")
            .args(["diff", "--no-color"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout;
        let secret_at = raw
            .windows(b"abcdef123456".len())
            .position(|window| window == b"abcdef123456")
            .unwrap();

        // The cut lands inside the secret; the redacted text must not show its start.
        let output = GitDiffTool
            .call(&json!({"max_bytes": secret_at + 4}), &ctx_for(dir.path()))
            .unwrap();
        assert!(!output.text.contains("abcd"), "{}", output.text);
    }

    #[test]
    fn git_status_caps_structured_lists_and_reports_counts() {
        let Some(dir) = init_repo() else { return };
        for index in 0..120 {
            fs::write(dir.path().join(format!("u{index:03}.txt")), "x\n").unwrap();
        }

        let output = GitStatusTool
            .call(&json!({}), &ctx_for(dir.path()))
            .unwrap();
        let structured = output.structured.unwrap();
        assert_eq!(
            structured["untracked"].as_array().unwrap().len(),
            LIST_LIMIT
        );
        assert_eq!(structured["untracked_count"], json!(120));
        assert!(output.text.contains("... and 70 more"), "{}", output.text);
    }

    #[cfg(unix)]
    #[test]
    fn run_verification_reports_stdout_and_stderr_tails_separately() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_for(dir.path());
        ctx.verification.steps = vec![VerificationStep {
            name: Some("noisy".to_owned()),
            command: format!("echo important-stdout; echo {} 1>&2", "E".repeat(5_000)),
            timeout_ms: Some(30_000),
            cwd: None,
            kind: StepKind::Test,
            targets: false,
        }];
        let output = RunVerificationTool.call(&json!({}), &ctx).unwrap();
        let step = &output.structured.unwrap()["steps"][0];
        assert!(
            step["output_tail"]
                .as_str()
                .unwrap()
                .contains("important-stdout"),
            "{step}"
        );
        assert!(
            step["stderr_tail"].as_str().unwrap().contains("EEEE"),
            "{step}"
        );
    }

    #[test]
    fn git_command_past_its_timeout_is_killed_with_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(dir.path());
        let hung = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "ping -n 6 127.0.0.1 > NUL"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("5");
            command
        };
        let started = Instant::now();
        let error = run_git_command(&ctx, hung, "diff", usize::MAX, Duration::from_millis(100))
            .unwrap_err();
        assert!(error.contains("git diff timed out after"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[cfg(unix)]
    #[test]
    fn git_command_output_stops_at_the_head_limit() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(dir.path());
        let mut producer = Command::new("head");
        producer.args(["-c", "100000", "/dev/zero"]);
        let stdout =
            run_git_command(&ctx, producer, "diff", 1_000, Duration::from_secs(30)).unwrap();
        assert_eq!(stdout.len(), 1_000);
    }

    #[test]
    fn a_missing_process_killer_is_reported_not_ignored() {
        let error = run_killer(Command::new("harness-no-such-killer")).unwrap_err();
        assert!(
            error.contains("failed to run the process killer"),
            "{error}"
        );
    }
}
