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

use std::path::PathBuf;

use harness_core::config::{Config, VerificationSection};
use harness_core::security::redact::Redactor;
use harness_core::security::tool_policy::ToolPolicy;

/// Shared state every tool call receives.
#[derive(Debug)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub config: Config,
    pub verification: VerificationSection,
    pub policy: ToolPolicy,
    pub redactor: Redactor,
}

/// A tool's result. `text` is what the model reads; `structured` is machine-readable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub structured: Option<serde_json::Value>,
    pub is_error: bool,
}

pub trait McpTool: Send {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// JSON Schema for the arguments object.
    fn input_schema(&self) -> serde_json::Value;
    fn call(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput, String>;
}

pub fn default_tools() -> Vec<Box<dyn McpTool>> {
    todo!()
}
