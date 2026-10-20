//! OpenAI-compatible `/v1/chat/completions` client (ureq 3, plain HTTP in M1).
//!
//! IMPLEMENTATION SPEC (delegated):
//! - `new(model, api_key)`: keep `base_url = model.endpoint` with any trailing `/` trimmed, model
//!   id, timeout (`model.timeout_ms`), retries (`model.retries`), sampling defaults
//!   (`model.sampling`), `model.max_output_tokens`, `model.reasoning.enabled`, and the API key.
//!   Build one `ureq::Agent` with `timeout_global = Some(timeout)` and
//!   `http_status_as_error(false)` so non-2xx bodies can be read and mapped to
//!   `ProviderError::Http { status, body (truncated to 2048 bytes) }`.
//! - Request body (`build_body(req, stream) -> serde_json::Value`, make it `pub(crate)` and unit
//!   test it): `model`, `messages` (role/content/tool_calls/tool_call_id in OpenAI shape: assistant
//!   tool calls as `{"id","type":"function","function":{"name","arguments"}}`), `tools` as
//!   `[{"type":"function","function":{"name","description","parameters"}}]` when non-empty,
//!   `tool_choice`, `response_format`, `max_tokens` (request or config), `temperature` (request
//!   or config), `top_p`, `top_k`, `min_p`, `repeat_penalty` (config, only when set),
//!   `stream`, `stream_options: {"include_usage": true}` when streaming, and
//!   `chat_template_kwargs: {"enable_thinking": <req.enable_thinking or config>}`.
//! - Header `Authorization: Bearer <key>` only when a key is set. Never log the key.
//! - `chat`: POST `<base>/chat/completions`; parse `choices[0].message.{content,
//!   reasoning_content, tool_calls}`, `choices[0].finish_reason`, `model`, `usage.{prompt_tokens,
//!   completion_tokens}`, and llama.cpp `timings.{prompt_ms, prompt_per_second, predicted_ms,
//!   predicted_per_second}` when present. Missing `choices` → `InvalidResponse`. Measure
//!   `latency_ms` with `Instant`.
//! - `chat_stream`: same POST with `stream: true`; read the body line by line
//!   (`BufRead` over `body_mut().as_reader()`); handle `data: {...}` lines, ignore blank lines and
//!   `:` comments, stop at `data: [DONE]`. Accumulate `delta.content` (call `on_delta` for each
//!   non-empty piece and record `first_token_ms` at the first one), `delta.reasoning_content`,
//!   and tool-call deltas merged by `index` (concatenate `function.arguments` fragments; take
//!   `id`/`name` from the first fragment that has them). Usage/timings from the final chunk if sent.
//! - `list_models`: GET `<base>/models` → `data[].id`.
//! - `health`: `list_models` succeeds → `HealthStatus { ok: true, detail: "models: <ids>" }`.
//! - Error mapping: connection refused / DNS / io → `Unreachable`; ureq timeout → `Timeout`.
//! - Retries: retry `Unreachable` and HTTP 502/503 (llama.cpp returns 503 while loading) up to
//!   `retries` extra attempts with 500 ms × attempt backoff. Never retry 4xx.
//! - Unit tests here cover `build_body` and SSE line parsing (factor the SSE accumulator into a
//!   small struct you can feed lines to without a network). End-to-end HTTP tests live in
//!   `crates/harness-runtime/tests/provider_http.rs` against a `std::net::TcpListener` fake server
//!   (no extra dependencies): JSON completion, tool call, SSE stream with split tool-call
//!   arguments, HTTP 500 → `Http`, connection refused → `Unreachable`, 503-then-200 retry.

use harness_core::config::ModelConfig;

use super::{ChatRequest, ChatResponse, HealthStatus, ModelProvider, ProviderError};

#[derive(Debug)]
pub struct OpenAiCompatProvider {
    model_id: String,
}

impl OpenAiCompatProvider {
    pub fn new(model: &ModelConfig, api_key: Option<String>) -> Self {
        let _ = api_key;
        Self {
            model_id: model.id.clone(),
        }
    }

    /// Base URL ending in `/v1` (no trailing slash).
    pub fn base_url(&self) -> &str {
        todo!()
    }
}

impl ModelProvider for OpenAiCompatProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn health(&self) -> Result<HealthStatus, ProviderError> {
        todo!()
    }
    fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        todo!()
    }
    fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let _ = req;
        todo!()
    }
    fn chat_stream(
        &self,
        req: &ChatRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatResponse, ProviderError> {
        let _ = (req, on_delta);
        todo!()
    }
}
