//! Functional health checks (§44): an HTTP 200 is not evidence of compatibility.
//!
//! IMPLEMENTATION SPEC (delegated). `run_checks(provider, model, opts)` runs, in order, and
//! returns one [`CheckResult`] per check (never panics, never returns early except: if `Health`
//! fails, every later check is `Skip` with detail "endpoint not healthy"):
//!
//! 1. `Health` — `provider.health()`; ok → Pass (detail = status), not ok → Fail.
//! 2. `Models` — `list_models()` contains `model.id` → Pass; reachable but id missing → Warn
//!    ("server serves [..], config expects <id>"); error → Fail.
//! 3. `Completion` — user prompt `Reply with the single word READY.`; `enable_thinking:
//!    Some(false)`, `max_tokens: 32`, temperature 0. Pass if the content contains `READY`
//!    (case-insensitive); Warn if non-empty but different; Fail on error/empty. Record latency and
//!    usage in `detail`/`latency_ms`.
//! 4. `Streaming` (only if `capabilities.streaming`, else Skip) — prompt `Count from 1 to 5.`;
//!    Pass if ≥ 2 deltas were received; record `first_token_ms` in detail.
//! 5. `ToolCall` (only if `capabilities.tool_calling`, else Skip) — offer one tool
//!    `get_weather(city: string, required)` with `tool_choice: "auto"` and prompt `What is the
//!    weather in Paris? Use the get_weather tool.` Pass if the first tool call is named
//!    `get_weather` and its arguments parse as a JSON object whose `city` contains "Paris"
//!    (case-insensitive); Warn if a tool call is present but malformed (record why — K-9);
//!    Fail if no tool call.
//! 6. `StructuredOutput` (only if `capabilities.structured_output`, else Skip) — `response_format`
//!    `{"type":"json_schema","json_schema":{"name":"decision","strict":true,"schema":{"type":
//!    "object","properties":{"decision":{"type":"string","enum":["CONTINUE","ESCALATE"]},
//!    "confidence":{"type":"number"}},"required":["decision","confidence"],
//!    "additionalProperties":false}}}`, prompt `Two attempts failed with the same error. Decide.`
//!    Pass if the content parses as JSON matching the schema (enum value valid, confidence a
//!    number in [0,1]); Fail otherwise (detail includes the first 200 chars).
//! 7. `Context` — `server_props()`: if it reports `n_ctx`, Pass when `n_ctx >= model.context`,
//!    Warn when smaller ("server n_ctx X < configured Y"); Skip if props unavailable.
//! 8. `Mtp` — informational: Skip when `model.mtp == Off`; otherwise from props
//!    (`LlamaCppProvider::props_speculative`-style detection on the JSON): Some(true) → Pass
//!    "speculative decoding active", Some(false)/None → Warn when `mtp == On` ("requested but not
//!    detected"), Skip when `mtp == Auto` ("not detected; optional").
//!
//! `opts.quick == true` runs only checks 1–3. Each check's `latency_ms` is wall-clock.
//! `summarize(&[CheckResult]) -> Status`: Fail if any Fail, else Warn if any Warn, else Pass
//! (Skip ignored). Tests in `crates/harness-runtime/tests/health_mock.rs` drive it with
//! `MockProvider::scripted` for each pass/warn/fail path.

use harness_core::config::ModelConfig;
use serde::{Deserialize, Serialize};

use crate::providers::ModelProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Health,
    Models,
    Completion,
    Streaming,
    ToolCall,
    StructuredOutput,
    Context,
    Mtp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Skip,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub kind: CheckKind,
    pub status: Status,
    pub detail: String,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckOptions {
    pub quick: bool,
}

pub fn run_checks(
    provider: &dyn ModelProvider,
    model: &ModelConfig,
    opts: CheckOptions,
) -> Vec<CheckResult> {
    let _ = (provider, model, opts);
    todo!()
}

pub fn summarize(results: &[CheckResult]) -> Status {
    let _ = results;
    todo!()
}
