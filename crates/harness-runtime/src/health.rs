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

use std::time::Instant;

use harness_core::config::{ModelConfig, MtpMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::providers::llama_cpp::{props_build, props_context};
use crate::providers::{
    ChatMessage, ChatRequest, HealthStatus, ModelProvider, ProviderError, ToolCall, ToolSpec,
};

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

/// Checks that quick mode runs, in order.
const QUICK_CHECKS: [CheckKind; 3] = [CheckKind::Health, CheckKind::Models, CheckKind::Completion];

/// Every check, in the order `run_checks` runs them.
const ALL_CHECKS: [CheckKind; 8] = [
    CheckKind::Health,
    CheckKind::Models,
    CheckKind::Completion,
    CheckKind::Streaming,
    CheckKind::ToolCall,
    CheckKind::StructuredOutput,
    CheckKind::Context,
    CheckKind::Mtp,
];

/// Runs the functional checks against `provider` for the slot described by `model`.
///
/// Returns one result per check, in §44 order. A failed health check skips every later check.
/// Quick mode returns only the first three results.
pub fn run_checks(
    provider: &dyn ModelProvider,
    model: &ModelConfig,
    opts: CheckOptions,
) -> Vec<CheckResult> {
    let health = check_health(provider);
    let healthy = health.status == Status::Pass;
    let mut results = vec![health];

    if !healthy {
        let later = if opts.quick {
            &QUICK_CHECKS[1..]
        } else {
            &ALL_CHECKS[1..]
        };
        results.extend(
            later
                .iter()
                .map(|kind| skipped(*kind, "endpoint not healthy")),
        );
        return results;
    }

    results.push(check_models(provider, model));
    results.push(check_completion(provider));
    if opts.quick {
        return results;
    }

    results.push(check_streaming(provider, model));
    results.push(check_tool_call(provider, model));
    results.push(check_structured_output(provider, model));

    // Fetched once: the context and MTP checks both read the same props document.
    let (props, props_ms) = timed(|| provider.server_props());
    results.push(check_context(&props, props_ms, model));
    results.push(check_mtp(&props, props_ms, model));
    results
}

/// Folds check results into one verdict: any Fail wins, then any Warn; Skip never counts.
pub fn summarize(results: &[CheckResult]) -> Status {
    if results.iter().any(|r| r.status == Status::Fail) {
        Status::Fail
    } else if results.iter().any(|r| r.status == Status::Warn) {
        Status::Warn
    } else {
        Status::Pass
    }
}

fn check_health(provider: &dyn ModelProvider) -> CheckResult {
    let (status, ms) = timed(|| provider.health());
    match status {
        Ok(HealthStatus { ok: true, detail }) => {
            check(CheckKind::Health, Status::Pass, detail, Some(ms))
        }
        Ok(HealthStatus { detail, .. }) => check(
            CheckKind::Health,
            Status::Fail,
            format!("endpoint reports not ok: {detail}"),
            Some(ms),
        ),
        Err(e) => check(CheckKind::Health, Status::Fail, e.to_string(), Some(ms)),
    }
}

fn check_models(provider: &dyn ModelProvider, model: &ModelConfig) -> CheckResult {
    let (listed, ms) = timed(|| provider.list_models());
    match listed {
        Err(e) => check(CheckKind::Models, Status::Fail, e.to_string(), Some(ms)),
        Ok(ids) if ids.contains(&model.id) => check(
            CheckKind::Models,
            Status::Pass,
            format!("endpoint serves '{}'", model.id),
            Some(ms),
        ),
        Ok(ids) => check(
            CheckKind::Models,
            Status::Warn,
            format!(
                "server serves [{}], config expects {}",
                ids.join(", "),
                model.id
            ),
            Some(ms),
        ),
    }
}

fn check_completion(provider: &dyn ModelProvider) -> CheckResult {
    let req = ChatRequest {
        messages: vec![ChatMessage::user("Reply with the single word READY.")],
        enable_thinking: Some(false),
        max_tokens: Some(32),
        temperature: Some(0.0),
        ..ChatRequest::default()
    };
    let (outcome, ms) = timed(|| provider.chat(&req));
    let resp = match outcome {
        Ok(resp) => resp,
        Err(e) => return check(CheckKind::Completion, Status::Fail, e.to_string(), Some(ms)),
    };
    let usage = format!(
        "tokens prompt {} / completion {}",
        fmt_opt(resp.usage.prompt_tokens),
        fmt_opt(resp.usage.completion_tokens)
    );
    let text = resp.content.unwrap_or_default();
    let reply = text.trim();
    if reply.is_empty() {
        check(
            CheckKind::Completion,
            Status::Fail,
            format!("empty completion ({usage})"),
            Some(ms),
        )
    } else if reply.to_ascii_lowercase().contains("ready") {
        check(
            CheckKind::Completion,
            Status::Pass,
            format!("replied READY ({usage})"),
            Some(ms),
        )
    } else {
        check(
            CheckKind::Completion,
            Status::Warn,
            format!("expected READY, got {:?} ({usage})", preview(reply, 80)),
            Some(ms),
        )
    }
}

fn check_streaming(provider: &dyn ModelProvider, model: &ModelConfig) -> CheckResult {
    if !model.capabilities.streaming {
        return skipped(CheckKind::Streaming, "capabilities.streaming = false");
    }
    let req = ChatRequest {
        messages: vec![ChatMessage::user("Count from 1 to 5.")],
        enable_thinking: Some(false),
        max_tokens: Some(32),
        temperature: Some(0.0),
        ..ChatRequest::default()
    };
    let start = Instant::now();
    let mut deltas = 0usize;
    let mut first_token_ms: Option<u64> = None;
    let outcome = {
        let mut on_delta = |_: &str| {
            deltas += 1;
            if first_token_ms.is_none() {
                first_token_ms = Some(elapsed_ms(start));
            }
        };
        provider.chat_stream(&req, &mut on_delta)
    };
    let ms = elapsed_ms(start);
    let resp = match outcome {
        Ok(resp) => resp,
        Err(e) => return check(CheckKind::Streaming, Status::Fail, e.to_string(), Some(ms)),
    };
    let first = resp.first_token_ms.or(first_token_ms);
    let timing = match first {
        Some(t) => format!("first token {t} ms"),
        None => "no first-token timing".to_owned(),
    };
    match deltas {
        0 => check(
            CheckKind::Streaming,
            Status::Fail,
            "stream produced no deltas",
            Some(ms),
        ),
        1 => check(
            CheckKind::Streaming,
            Status::Warn,
            format!("only 1 delta; server may not stream incrementally ({timing})"),
            Some(ms),
        ),
        n => check(
            CheckKind::Streaming,
            Status::Pass,
            format!("{n} deltas, {timing}"),
            Some(ms),
        ),
    }
}

fn check_tool_call(provider: &dyn ModelProvider, model: &ModelConfig) -> CheckResult {
    if !model.capabilities.tool_calling {
        return skipped(CheckKind::ToolCall, "capabilities.tool_calling = false");
    }
    let req = ChatRequest {
        messages: vec![ChatMessage::user(
            "What is the weather in Paris? Use the get_weather tool.",
        )],
        tools: vec![weather_tool()],
        tool_choice: Some(json!("auto")),
        enable_thinking: Some(false),
        max_tokens: Some(256),
        temperature: Some(0.0),
        ..ChatRequest::default()
    };
    let (outcome, ms) = timed(|| provider.chat(&req));
    let resp = match outcome {
        Ok(resp) => resp,
        Err(e) => return check(CheckKind::ToolCall, Status::Fail, e.to_string(), Some(ms)),
    };
    match resp.tool_calls.first() {
        None => {
            let content = resp.content.unwrap_or_default();
            check(
                CheckKind::ToolCall,
                Status::Fail,
                format!("no tool call; content {:?}", preview(&content, 100)),
                Some(ms),
            )
        }
        Some(call) => match validate_weather_call(call) {
            Ok(()) => check(
                CheckKind::ToolCall,
                Status::Pass,
                format!("get_weather called with {}", preview(&call.arguments, 80)),
                Some(ms),
            ),
            Err(reason) => check(
                CheckKind::ToolCall,
                Status::Warn,
                format!(
                    "tool call malformed: {reason}; arguments {:?}",
                    preview(&call.arguments, 120)
                ),
                Some(ms),
            ),
        },
    }
}

fn check_structured_output(provider: &dyn ModelProvider, model: &ModelConfig) -> CheckResult {
    if !model.capabilities.structured_output {
        return skipped(
            CheckKind::StructuredOutput,
            "capabilities.structured_output = false",
        );
    }
    let req = ChatRequest {
        messages: vec![ChatMessage::user(
            "Two attempts failed with the same error. Decide.",
        )],
        response_format: Some(decision_response_format()),
        enable_thinking: Some(false),
        max_tokens: Some(128),
        temperature: Some(0.0),
        ..ChatRequest::default()
    };
    let (outcome, ms) = timed(|| provider.chat(&req));
    let resp = match outcome {
        Ok(resp) => resp,
        Err(e) => {
            return check(
                CheckKind::StructuredOutput,
                Status::Fail,
                e.to_string(),
                Some(ms),
            );
        }
    };
    let content = resp.content.unwrap_or_default();
    match validate_decision(content.trim()) {
        Ok(()) => check(
            CheckKind::StructuredOutput,
            Status::Pass,
            "content matches the decision schema",
            Some(ms),
        ),
        Err(reason) => check(
            CheckKind::StructuredOutput,
            Status::Fail,
            format!("{reason}; content {:?}", preview(&content, 200)),
            Some(ms),
        ),
    }
}

fn check_context(
    props: &Result<Option<Value>, ProviderError>,
    props_ms: u64,
    model: &ModelConfig,
) -> CheckResult {
    let props = match props {
        Ok(Some(value)) => value,
        Ok(None) => return skipped(CheckKind::Context, "server exposes no props"),
        Err(e) => return skipped(CheckKind::Context, format!("props unavailable: {e}")),
    };
    let Some(n_ctx) = props_context(props) else {
        return skipped(CheckKind::Context, "props do not report n_ctx");
    };
    let configured = u64::from(model.context);
    let build = props_build(props)
        .map(|build| format!(", build {build}"))
        .unwrap_or_default();
    if n_ctx >= configured {
        check(
            CheckKind::Context,
            Status::Pass,
            format!("server n_ctx {n_ctx} >= configured {configured}{build}"),
            Some(props_ms),
        )
    } else {
        check(
            CheckKind::Context,
            Status::Warn,
            format!("server n_ctx {n_ctx} < configured {configured}{build}"),
            Some(props_ms),
        )
    }
}

fn check_mtp(
    props: &Result<Option<Value>, ProviderError>,
    props_ms: u64,
    model: &ModelConfig,
) -> CheckResult {
    if model.mtp == MtpMode::Off {
        return skipped(CheckKind::Mtp, "disabled (mtp = off)");
    }
    let detected = props_value(props).and_then(speculative_flag);
    match (detected, model.mtp) {
        (Some(true), _) => check(
            CheckKind::Mtp,
            Status::Pass,
            "speculative decoding active",
            Some(props_ms),
        ),
        (_, MtpMode::On) => check(
            CheckKind::Mtp,
            Status::Warn,
            "mtp = on but speculative decoding not detected",
            Some(props_ms),
        ),
        _ => skipped(CheckKind::Mtp, "not detected; optional"),
    }
}

/// The `get_weather` tool offered by the tool-call check.
fn weather_tool() -> ToolSpec {
    ToolSpec {
        name: "get_weather".to_owned(),
        description: "Get the current weather for a city.".to_owned(),
        parameters: json!({
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"]
        }),
    }
}

/// Checks a tool call against the `get_weather(city)` contract. The error names the defect.
fn validate_weather_call(call: &ToolCall) -> Result<(), String> {
    if call.name != "get_weather" {
        return Err(format!("called '{}' instead of get_weather", call.name));
    }
    let args: Value = serde_json::from_str(&call.arguments)
        .map_err(|e| format!("arguments are not valid JSON ({e})"))?;
    let object = args.as_object().ok_or("arguments are not a JSON object")?;
    let city = object
        .get("city")
        .and_then(Value::as_str)
        .ok_or("missing string field 'city'")?;
    if city.to_ascii_lowercase().contains("paris") {
        Ok(())
    } else {
        Err(format!("city is {city:?}, expected Paris"))
    }
}

/// `response_format` for the structured-output check.
fn decision_response_format() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "decision",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "decision": { "type": "string", "enum": ["CONTINUE", "ESCALATE"] },
                    "confidence": { "type": "number" }
                },
                "required": ["decision", "confidence"],
                "additionalProperties": false
            }
        }
    })
}

/// Validates the structured-output reply by hand: an object with exactly `decision` (one of the
/// enum values) and `confidence` (a number in [0, 1]).
fn validate_decision(text: &str) -> Result<(), String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON ({e})"))?;
    let object = value.as_object().ok_or("not a JSON object")?;
    if let Some(extra) = object
        .keys()
        .find(|k| !matches!(k.as_str(), "decision" | "confidence"))
    {
        return Err(format!("unexpected property '{extra}'"));
    }
    let decision = object
        .get("decision")
        .ok_or("missing 'decision'")?
        .as_str()
        .ok_or("'decision' is not a string")?;
    if !matches!(decision, "CONTINUE" | "ESCALATE") {
        return Err(format!(
            "'decision' {decision:?} is not CONTINUE or ESCALATE"
        ));
    }
    let confidence = object
        .get("confidence")
        .ok_or("missing 'confidence'")?
        .as_f64()
        .ok_or("'confidence' is not a number")?;
    if !(0.0..=1.0).contains(&confidence) {
        return Err(format!("'confidence' {confidence} is outside [0, 1]"));
    }
    Ok(())
}

/// The props document, when the server returned one.
fn props_value(props: &Result<Option<Value>, ProviderError>) -> Option<&Value> {
    props.as_ref().ok().and_then(Option::as_ref)
}

/// Context size reported by llama.cpp props: `default_generation_settings.n_ctx`, falling back
/// to a top-level `n_ctx`.
/// Looks for keys that mention speculative or draft decoding anywhere in the props JSON.
///
/// `Some(true)` when such a key holds a truthy value, `Some(false)` when such keys exist but are
/// all falsy, `None` when no such key exists. Heuristic: llama.cpp does not document a stable key.
fn speculative_flag(props: &Value) -> Option<bool> {
    let mut found = None;
    collect_speculative(props, &mut found);
    found
}

fn collect_speculative(value: &Value, found: &mut Option<bool>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let lowered = key.to_ascii_lowercase();
                if lowered.contains("speculative") || lowered.contains("draft") {
                    *found = Some(found.unwrap_or(false) || is_truthy(child));
                }
                collect_speculative(child, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_speculative(item, found);
            }
        }
        _ => {}
    }
}

/// Scalars are truthy when non-zero, non-empty or `true`. Containers are truthy when any value is.
/// Strings that name an off state are falsy: llama.cpp reports `"types": "none"` when no
/// speculative decoding is configured (checked against b11756 `/props`).
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "" | "none" | "off" | "false" | "disabled" | "0"
        ),
        Value::Array(items) => items.iter().any(is_truthy),
        Value::Object(map) => map.values().any(is_truthy),
    }
}

fn check(
    kind: CheckKind,
    status: Status,
    detail: impl Into<String>,
    latency_ms: Option<u64>,
) -> CheckResult {
    CheckResult {
        kind,
        status,
        detail: detail.into(),
        latency_ms,
    }
}

fn skipped(kind: CheckKind, detail: impl Into<String>) -> CheckResult {
    check(kind, Status::Skip, detail, None)
}

/// Runs `f` and returns its value with the wall-clock time it took.
fn timed<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let start = Instant::now();
    let value = f();
    (value, elapsed_ms(start))
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn fmt_opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |v| v.to_string())
}

/// First `max_chars` characters of `text`, with `...` appended when cut.
fn preview(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}...", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod speculative_tests {
    use serde_json::json;

    use super::speculative_flag;

    #[test]
    fn off_state_strings_are_not_speculative_decoding() {
        // Exact shape reported by llama.cpp b11756 `/props` when MTP is not configured.
        let props =
            json!({"default_generation_settings": {"params": {"speculative": {"types": "none"}}}});
        assert_eq!(speculative_flag(&props), Some(false));
    }

    #[test]
    fn an_active_draft_type_is_speculative_decoding() {
        let props = json!({"speculative": {"types": "draft-mtp"}});
        assert_eq!(speculative_flag(&props), Some(true));
    }

    #[test]
    fn absent_key_is_unknown() {
        assert_eq!(speculative_flag(&json!({"n_ctx": 16384})), None);
    }
}
