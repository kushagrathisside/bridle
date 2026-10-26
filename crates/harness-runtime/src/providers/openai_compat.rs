//! OpenAI-compatible `/v1/chat/completions` client over `ureq` (plain HTTP in M1).
//!
//! Covers non-streaming and SSE streaming chat, model listing and a liveness probe. Connection
//! failures and HTTP 503 are retried with a short backoff; every other failure is returned at
//! once. The API key goes only into the `Authorization` header and is never printed.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::thread;
use std::time::{Duration, Instant};

use harness_core::config::{ModelConfig, Sampling};
use serde_json::{Map, Value, json};

use super::{
    ChatMessage, ChatRequest, ChatResponse, HealthStatus, ModelProvider, ProviderError, Role,
    Timings, ToolCall, Usage,
};

/// Sleep before retry `n` (1-based) is `RETRY_BACKOFF * n`.
const RETRY_BACKOFF: Duration = Duration::from_millis(500);
/// Bytes of an error body kept in [`ProviderError::Http`].
const ERROR_BODY_LIMIT: usize = 2048;

type HttpResponse = ureq::http::Response<ureq::Body>;

/// Secret holder whose `Debug` output never reveals the value (R-35.2).
struct ApiKey(String);

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Client for any OpenAI-compatible server. Holds one pooled `ureq::Agent` per model slot.
#[derive(Debug)]
pub struct OpenAiCompatProvider {
    model_id: String,
    /// `model.endpoint` without a trailing `/`, e.g. `http://127.0.0.1:8001/v1`.
    base_url: String,
    api_key: Option<ApiKey>,
    timeout: Duration,
    retries: u32,
    sampling: Sampling,
    max_output_tokens: u32,
    reasoning_enabled: bool,
    agent: ureq::Agent,
}

impl OpenAiCompatProvider {
    /// Builds a provider for `model`. `api_key` is the already-resolved secret; an empty string
    /// counts as no key.
    pub fn new(model: &ModelConfig, api_key: Option<String>) -> Self {
        let timeout = Duration::from_millis(model.timeout_ms);
        // A loopback endpoint never goes through a proxy; any other host uses the environment's proxy and TLS (M7, M8).
        let agent = if harness_core::config::endpoint_is_loopback(&model.endpoint) {
            crate::http::local_agent(timeout)
        } else {
            crate::http::remote_agent(timeout)
        };
        Self {
            model_id: model.id.clone(),
            base_url: model.endpoint.trim_end_matches('/').to_owned(),
            api_key: api_key.filter(|key| !key.is_empty()).map(ApiKey),
            timeout,
            retries: model.retries,
            sampling: model.sampling,
            max_output_tokens: model.max_output_tokens,
            reasoning_enabled: model.reasoning.enabled,
            agent,
        }
    }

    /// Base URL ending in `/v1` (no trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET` without any status interpretation, for endpoints whose statuses the caller maps
    /// itself (llama.cpp `/health` answers 503 while loading, which is not an error).
    pub(crate) fn get_raw(&self, url: &str) -> Result<RawReply, ProviderError> {
        let mut request = self.agent.get(url);
        if let Some(auth) = self.auth_header() {
            request = request.header("Authorization", auth);
        }
        let mut resp = request
            .call()
            .map_err(|e| transport_error(e, self.timeout))?;
        let status = resp.status().as_u16();
        let body = read_text(resp.body_mut(), self.timeout)?;
        Ok(RawReply { status, body })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn auth_header(&self) -> Option<String> {
        self.api_key.as_ref().map(|key| format!("Bearer {}", key.0))
    }

    /// `POST <base><path>` with a JSON body. Returns the response only for 2xx statuses; the body
    /// is left unread so streaming callers can consume it.
    fn post_json(&self, path: &str, body: &Value) -> Result<HttpResponse, ProviderError> {
        let url = self.url(path);
        let mut request = self.agent.post(url.as_str());
        if let Some(auth) = self.auth_header() {
            request = request.header("Authorization", auth);
        }
        let resp = request
            .send_json(body)
            .map_err(|e| transport_error(e, self.timeout))?;
        ensure_success(resp)
    }

    /// Runs `attempt`, retrying connection failures and HTTP 503 up to `self.retries` extra times
    /// with `500 ms × attempt` backoff. Other errors (including 4xx and 5xx) are returned at once.
    fn with_retries<T>(
        &self,
        mut attempt: impl FnMut() -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let mut retried = 0;
        loop {
            match attempt() {
                Err(err) if retried < self.retries && is_transient(&err) => {
                    retried += 1;
                    thread::sleep(RETRY_BACKOFF * retried);
                }
                outcome => return outcome,
            }
        }
    }

    /// Request body in OpenAI shape with this model's defaults filled in (see the spec above).
    pub(crate) fn build_body(&self, req: &ChatRequest, stream: bool) -> Value {
        let mut body = Map::new();
        body.insert("model".to_owned(), json!(self.model_id));
        body.insert(
            "messages".to_owned(),
            Value::Array(req.messages.iter().map(message_json).collect()),
        );
        if !req.tools.is_empty() {
            let tools = req
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    })
                })
                .collect();
            body.insert("tools".to_owned(), Value::Array(tools));
        }
        if let Some(choice) = &req.tool_choice {
            body.insert("tool_choice".to_owned(), choice.clone());
        }
        if let Some(format) = &req.response_format {
            body.insert("response_format".to_owned(), format.clone());
        }
        let max_tokens = req.max_tokens.unwrap_or(self.max_output_tokens);
        body.insert("max_tokens".to_owned(), json!(max_tokens));
        if let Some(temperature) = req.temperature.or(self.sampling.temperature) {
            body.insert("temperature".to_owned(), f32_json(temperature));
        }
        if let Some(top_p) = self.sampling.top_p {
            body.insert("top_p".to_owned(), f32_json(top_p));
        }
        if let Some(top_k) = self.sampling.top_k {
            body.insert("top_k".to_owned(), json!(top_k));
        }
        if let Some(min_p) = self.sampling.min_p {
            body.insert("min_p".to_owned(), f32_json(min_p));
        }
        if let Some(penalty) = self.sampling.repeat_penalty {
            body.insert("repeat_penalty".to_owned(), f32_json(penalty));
        }
        body.insert("stream".to_owned(), json!(stream));
        if stream {
            body.insert(
                "stream_options".to_owned(),
                json!({ "include_usage": true }),
            );
        }
        let thinking = req.enable_thinking.unwrap_or(self.reasoning_enabled);
        body.insert(
            "chat_template_kwargs".to_owned(),
            json!({ "enable_thinking": thinking }),
        );
        Value::Object(body)
    }
}

impl ModelProvider for OpenAiCompatProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn health(&self) -> Result<HealthStatus, ProviderError> {
        let ids = self.list_models()?;
        Ok(HealthStatus {
            ok: true,
            detail: format!("models: {}", ids.join(", ")),
        })
    }

    fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let url = self.url("/models");
        let value = self.with_retries(|| {
            let reply = self.get_raw(&url)?;
            parse_json(&expect_success(reply)?)
        })?;
        let data = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::InvalidResponse("models response has no data".into()))?;
        Ok(data
            .iter()
            .filter_map(|model| str_field(model, "id"))
            .collect())
    }

    fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let started = Instant::now();
        let body = self.build_body(req, false);
        let value = self.with_retries(|| {
            let mut resp = self.post_json("/chat/completions", &body)?;
            parse_json(&read_text(resp.body_mut(), self.timeout)?)
        })?;
        parse_chat(&value, &self.model_id, elapsed_ms(started))
    }

    fn chat_stream(
        &self,
        req: &ChatRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatResponse, ProviderError> {
        let started = Instant::now();
        let body = self.build_body(req, true);
        // Retries cover only the connect/status phase: once a delta has reached the caller, a retry
        // would repeat output.
        let mut resp = self.with_retries(|| self.post_json("/chat/completions", &body))?;
        let mut acc = StreamAccumulator::new(started);
        let mut reader = BufReader::new(resp.body_mut().as_reader());
        let mut raw = Vec::new();
        while read_sse_line(&mut reader, &mut raw)
            .map_err(|e| transport_error(ureq::Error::from(e), self.timeout))?
        {
            let line = std::str::from_utf8(&raw)
                .map_err(|_| ProviderError::InvalidResponse("SSE line is not UTF-8".to_owned()))?;
            if acc.feed_line(line, on_delta)? {
                break;
            }
        }
        Ok(acc.finish(&self.model_id))
    }
}

/// The longest SSE line accepted. A delta line is a few hundred bytes and a usage chunk a few
/// kilobytes; a longer line means a broken or hostile server, and is not buffered.
const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;

/// Reads one line into `line` without its `\n` or `\r\n`. Returns `Ok(false)` at end of stream,
/// and an `InvalidData` error for a line longer than [`MAX_SSE_LINE_BYTES`].
fn read_sse_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<bool> {
    line.clear();
    let limit = MAX_SSE_LINE_BYTES + 1;
    let read = reader
        .by_ref()
        .take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_until(b'\n', line)?;
    if read == 0 {
        return Ok(false);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    } else if read == limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("SSE line exceeds {MAX_SSE_LINE_BYTES} bytes"),
        ));
    }
    Ok(true)
}

/// Status and body of a reply whose status the caller interprets.
#[derive(Debug)]
pub(crate) struct RawReply {
    pub(crate) status: u16,
    pub(crate) body: String,
}

/// `ProviderError::Http` with the body truncated to [`ERROR_BODY_LIMIT`] bytes.
pub(crate) fn http_error(status: u16, body: String) -> ProviderError {
    ProviderError::Http {
        status,
        body: truncate_utf8(body, ERROR_BODY_LIMIT),
    }
}

/// Maps a ureq failure onto the provider error taxonomy. `timeout` is the configured limit.
pub(crate) fn transport_error(err: ureq::Error, timeout: Duration) -> ProviderError {
    match err {
        ureq::Error::Timeout(_) => ProviderError::Timeout(timeout),
        ureq::Error::Io(e) if e.kind() == io::ErrorKind::TimedOut => {
            ProviderError::Timeout(timeout)
        }
        ureq::Error::Io(e) if e.kind() == io::ErrorKind::InvalidData => {
            ProviderError::InvalidResponse(e.to_string())
        }
        ureq::Error::Io(e) => ProviderError::Unreachable(e.to_string()),
        ureq::Error::ConnectionFailed => ProviderError::Unreachable("connection failed".to_owned()),
        ureq::Error::HostNotFound => ProviderError::Unreachable("host not found".to_owned()),
        ureq::Error::Json(e) => ProviderError::InvalidResponse(e.to_string()),
        ureq::Error::Protocol(e) => ProviderError::InvalidResponse(e.to_string()),
        other => ProviderError::Other(other.to_string()),
    }
}

/// `Unreachable` and HTTP 503 (llama.cpp answers 503 while the model loads) are worth retrying;
/// everything else is final.
fn is_transient(err: &ProviderError) -> bool {
    matches!(
        err,
        ProviderError::Unreachable(_) | ProviderError::Http { status: 503, .. }
    )
}

/// Turns a non-2xx response into `ProviderError::Http`; 2xx responses pass through unread.
fn ensure_success(mut resp: HttpResponse) -> Result<HttpResponse, ProviderError> {
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(resp);
    }
    let mut buf = Vec::new();
    // A failed read of an error body still reports the status, so the partial text is kept.
    let _ = resp
        .body_mut()
        .as_reader()
        .take(ERROR_BODY_LIMIT as u64)
        .read_to_end(&mut buf);
    Err(http_error(
        status,
        String::from_utf8_lossy(&buf).into_owned(),
    ))
}

/// Body of a 2xx reply, or the `Http` error for anything else.
fn expect_success(reply: RawReply) -> Result<String, ProviderError> {
    if (200..300).contains(&reply.status) {
        Ok(reply.body)
    } else {
        Err(http_error(reply.status, reply.body))
    }
}

pub(crate) fn read_text(body: &mut ureq::Body, timeout: Duration) -> Result<String, ProviderError> {
    body.read_to_string()
        .map_err(|e| transport_error(e, timeout))
}

fn parse_json(text: &str) -> Result<Value, ProviderError> {
    serde_json::from_str(text)
        .map_err(|e| ProviderError::InvalidResponse(format!("body is not JSON: {e}")))
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Truncates `s` to at most `max` bytes, cutting on a char boundary.
fn truncate_utf8(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// JSON number for an `f32` sampling value, written in its shortest decimal form (0.7 stays 0.7).
fn f32_json(value: f32) -> Value {
    let as_f64 = value.to_string().parse::<f64>().unwrap_or(f64::from(value));
    serde_json::Number::from_f64(as_f64).map_or(Value::Null, Value::Number)
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn message_json(message: &ChatMessage) -> Value {
    let mut obj = Map::new();
    obj.insert("role".to_owned(), json!(role_str(message.role)));
    obj.insert("content".to_owned(), json!(message.content));
    if !message.tool_calls.is_empty() {
        let calls: Vec<Value> = message
            .tool_calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": { "name": call.name, "arguments": call.arguments },
                })
            })
            .collect();
        obj.insert("tool_calls".to_owned(), Value::Array(calls));
    }
    if let Some(id) = &message.tool_call_id {
        obj.insert("tool_call_id".to_owned(), json!(id));
    }
    Value::Object(obj)
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Sets `slot` from `value` only while `slot` is still empty (first fragment wins).
fn set_if_empty(slot: &mut String, value: Option<&str>) {
    if let Some(v) = value.filter(|_| slot.is_empty()) {
        *slot = v.to_owned();
    }
}

fn parse_usage(value: Option<&Value>) -> Option<Usage> {
    let obj = value?.as_object()?;
    Some(Usage {
        prompt_tokens: obj.get("prompt_tokens").and_then(Value::as_u64),
        completion_tokens: obj.get("completion_tokens").and_then(Value::as_u64),
    })
}

fn parse_timings(value: &Value) -> Option<Timings> {
    let obj = value.as_object()?;
    let number = |key: &str| obj.get(key).and_then(Value::as_f64);
    Some(Timings {
        prompt_ms: number("prompt_ms"),
        prompt_per_second: number("prompt_per_second"),
        predicted_ms: number("predicted_ms"),
        predicted_per_second: number("predicted_per_second"),
    })
}

/// `arguments` is normally a JSON string; some servers send an object, which is kept as its JSON text.
fn arguments_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn parse_tool_calls(calls: Option<&Value>) -> Vec<ToolCall> {
    calls
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |calls| {
            calls.iter().map(parse_tool_call).collect()
        })
}

fn parse_tool_call(call: &Value) -> ToolCall {
    let function = call.get("function");
    ToolCall {
        id: str_field(call, "id").unwrap_or_default(),
        name: function
            .and_then(|f| str_field(f, "name"))
            .unwrap_or_default(),
        arguments: function
            .and_then(|f| f.get("arguments"))
            .map(arguments_text)
            .unwrap_or_default(),
    }
}

/// Parses a non-streaming chat-completion body.
fn parse_chat(
    json: &Value,
    default_model: &str,
    latency_ms: u64,
) -> Result<ChatResponse, ProviderError> {
    let choice = json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| ProviderError::InvalidResponse("response has no choices[0]".into()))?;
    let message = choice
        .get("message")
        .filter(|message| message.is_object())
        .ok_or_else(|| ProviderError::InvalidResponse("choices[0] has no message".into()))?;
    Ok(ChatResponse {
        model: str_field(json, "model").unwrap_or_else(|| default_model.to_owned()),
        content: str_field(message, "content"),
        reasoning: str_field(message, "reasoning_content"),
        tool_calls: parse_tool_calls(message.get("tool_calls")),
        finish_reason: str_field(choice, "finish_reason"),
        usage: parse_usage(json.get("usage")).unwrap_or_default(),
        timings: json.get("timings").and_then(parse_timings),
        latency_ms,
        first_token_ms: None,
    })
}

/// Incremental assembler for an OpenAI-style SSE chat stream. Pure: feed it lines, then `finish`.
#[derive(Debug)]
struct StreamAccumulator {
    started: Instant,
    model: Option<String>,
    content: Option<String>,
    reasoning: Option<String>,
    /// Tool-call fragments merged by their `index`.
    tool_calls: BTreeMap<u64, ToolCall>,
    finish_reason: Option<String>,
    usage: Usage,
    timings: Option<Timings>,
    first_token_ms: Option<u64>,
}

impl StreamAccumulator {
    fn new(started: Instant) -> Self {
        Self {
            started,
            model: None,
            content: None,
            reasoning: None,
            tool_calls: BTreeMap::new(),
            finish_reason: None,
            usage: Usage::default(),
            timings: None,
            first_token_ms: None,
        }
    }

    /// Consumes one line of the SSE body. Returns `Ok(true)` once the `[DONE]` sentinel arrives.
    /// Blank lines, `:` comments and non-`data` fields are ignored.
    fn feed_line(
        &mut self,
        line: &str,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<bool, ProviderError> {
        let Some(payload) = line.strip_prefix("data:") else {
            return Ok(false);
        };
        let payload = payload.strip_prefix(' ').unwrap_or(payload);
        if payload == "[DONE]" {
            return Ok(true);
        }
        let chunk: Value = serde_json::from_str(payload)
            .map_err(|e| ProviderError::InvalidResponse(format!("bad SSE chunk: {e}")))?;
        if let Some(error) = chunk.get("error").filter(|e| !e.is_null()) {
            let message = str_field(error, "message").unwrap_or_else(|| error.to_string());
            return Err(ProviderError::Other(format!("stream error: {message}")));
        }
        if self.model.is_none() {
            self.model = str_field(&chunk, "model");
        }
        if let Some(usage) = parse_usage(chunk.get("usage")) {
            self.usage = usage;
        }
        if let Some(timings) = chunk.get("timings").and_then(parse_timings) {
            self.timings = Some(timings);
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return Ok(false);
        };
        if let Some(reason) = str_field(choice, "finish_reason") {
            self.finish_reason = Some(reason);
        }
        let Some(delta) = choice.get("delta") else {
            return Ok(false);
        };
        if let Some(piece) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|piece| !piece.is_empty())
        {
            if self.first_token_ms.is_none() {
                self.first_token_ms = Some(elapsed_ms(self.started));
            }
            on_delta(piece);
            self.content.get_or_insert_with(String::new).push_str(piece);
        }
        if let Some(piece) = delta
            .get("reasoning_content")
            .and_then(Value::as_str)
            .filter(|piece| !piece.is_empty())
        {
            self.reasoning
                .get_or_insert_with(String::new)
                .push_str(piece);
        }
        if let Some(fragments) = delta.get("tool_calls").and_then(Value::as_array) {
            for (position, fragment) in fragments.iter().enumerate() {
                let index = fragment
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(position as u64);
                let call = self.tool_calls.entry(index).or_insert_with(|| ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
                set_if_empty(&mut call.id, str_field(fragment, "id").as_deref());
                if let Some(function) = fragment.get("function") {
                    set_if_empty(&mut call.name, str_field(function, "name").as_deref());
                    if let Some(args) = function.get("arguments").and_then(Value::as_str) {
                        call.arguments.push_str(args);
                    }
                }
            }
        }
        Ok(false)
    }

    fn finish(self, default_model: &str) -> ChatResponse {
        ChatResponse {
            model: self.model.unwrap_or_else(|| default_model.to_owned()),
            content: self.content,
            reasoning: self.reasoning,
            tool_calls: self.tool_calls.into_values().collect(),
            finish_reason: self.finish_reason,
            usage: self.usage,
            timings: self.timings,
            latency_ms: elapsed_ms(self.started),
            first_token_ms: self.first_token_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::providers::ToolSpec;

    use super::*;

    fn model(overrides: Value) -> ModelConfig {
        let mut base = json!({
            "provider": "openai_compatible",
            "id": "test-model",
            "role": "engineer",
            "endpoint": "http://127.0.0.1:9/v1/",
            "context": 8192,
            "max_output_tokens": 512,
            "sampling": { "temperature": 0.7, "top_k": 40, "min_p": 0.05 },
            "reasoning": { "enabled": true },
            "hardware": "cpu"
        });
        if let (Some(obj), Value::Object(extra)) = (base.as_object_mut(), overrides) {
            obj.extend(extra);
        }
        serde_json::from_value(base).unwrap_or_else(|e| panic!("fixture: {e}"))
    }

    fn provider() -> OpenAiCompatProvider {
        OpenAiCompatProvider::new(&model(json!({})), None)
    }

    fn request() -> ChatRequest {
        ChatRequest {
            messages: vec![ChatMessage::system("be brief"), ChatMessage::user("hi")],
            max_tokens: Some(64),
            ..ChatRequest::default()
        }
    }

    #[test]
    fn base_url_trims_trailing_slash() {
        assert_eq!(provider().base_url(), "http://127.0.0.1:9/v1");
    }

    #[test]
    fn debug_output_never_contains_the_api_key() {
        let p = OpenAiCompatProvider::new(&model(json!({})), Some("sk-very-secret".into()));
        let rendered = format!("{p:?}");
        assert!(!rendered.contains("sk-very-secret"), "leaked: {rendered}");
        assert!(rendered.contains("***"));
    }

    #[test]
    fn empty_api_key_is_treated_as_absent() {
        let p = OpenAiCompatProvider::new(&model(json!({})), Some(String::new()));
        assert!(p.auth_header().is_none());
    }

    #[test]
    fn body_uses_config_defaults_and_request_overrides() {
        let p = provider();
        let body = p.build_body(&request(), false);
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["max_tokens"], 64, "request value wins over config");
        assert_eq!(
            body["temperature"], 0.7,
            "config temperature, shortest form"
        );
        assert_eq!(body["top_k"], 40);
        assert_eq!(body["min_p"], 0.05);
        assert!(body.get("top_p").is_none(), "unset sampling is omitted");
        assert!(body.get("repeat_penalty").is_none());
        assert_eq!(body["stream"].as_bool(), Some(false));
        assert!(body.get("stream_options").is_none());
        assert_eq!(
            body["chat_template_kwargs"]["enable_thinking"].as_bool(),
            Some(true)
        );
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn request_thinking_flag_overrides_config() {
        let req = ChatRequest {
            enable_thinking: Some(false),
            ..request()
        };
        let body = provider().build_body(&req, false);
        assert_eq!(
            body["chat_template_kwargs"]["enable_thinking"].as_bool(),
            Some(false)
        );
    }

    #[test]
    fn streaming_body_asks_for_usage() {
        let body = provider().build_body(&request(), true);
        assert_eq!(body["stream"].as_bool(), Some(true));
        assert_eq!(
            body["stream_options"]["include_usage"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn max_tokens_falls_back_to_config() {
        let req = ChatRequest {
            max_tokens: None,
            ..request()
        };
        let body = provider().build_body(&req, false);
        assert_eq!(body["max_tokens"], 512);
    }

    #[test]
    fn tools_and_tool_messages_use_openai_shape() {
        let req = ChatRequest {
            messages: vec![
                ChatMessage {
                    role: Role::Assistant,
                    content: None,
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        arguments: "{\"path\":\"a.rs\"}".into(),
                    }],
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::Tool,
                    content: Some("contents".into()),
                    tool_calls: vec![],
                    tool_call_id: Some("call_1".into()),
                },
            ],
            tools: vec![ToolSpec {
                name: "read_file".into(),
                description: "Read a file".into(),
                parameters: json!({"type": "object"}),
            }],
            tool_choice: Some(json!("auto")),
            ..ChatRequest::default()
        };
        let body = provider().build_body(&req, false);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(body["tool_choice"], "auto");
        let assistant = &body["messages"][0];
        assert_eq!(assistant["role"], "assistant");
        assert!(assistant["content"].is_null());
        assert_eq!(assistant["tool_calls"][0]["id"], "call_1");
        assert_eq!(assistant["tool_calls"][0]["type"], "function");
        assert_eq!(
            assistant["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a.rs\"}"
        );
        let tool = &body["messages"][1];
        assert_eq!(tool["role"], "tool");
        assert_eq!(tool["tool_call_id"], "call_1");
    }

    #[test]
    fn chat_response_parses_content_usage_and_timings() -> Result<(), Box<dyn Error>> {
        let json = json!({
            "model": "served-name",
            "choices": [{
                "message": {"role": "assistant", "content": "hi", "reasoning_content": "hmm"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 3},
            "timings": {"prompt_ms": 1.5, "prompt_per_second": 900.0,
                        "predicted_ms": 30.0, "predicted_per_second": 100.0}
        });
        let resp = parse_chat(&json, "fallback", 7)?;
        assert_eq!(resp.model, "served-name");
        assert_eq!(resp.content.as_deref(), Some("hi"));
        assert_eq!(resp.reasoning.as_deref(), Some("hmm"));
        assert_eq!(resp.finish_reason.as_deref(), Some("stop"));
        assert_eq!(resp.usage.prompt_tokens, Some(10));
        assert_eq!(resp.usage.completion_tokens, Some(3));
        assert_eq!(resp.timings.and_then(|t| t.predicted_ms), Some(30.0));
        assert_eq!(resp.latency_ms, 7);
        Ok(())
    }

    #[test]
    fn chat_response_without_choices_is_invalid() {
        let err = parse_chat(&json!({"model": "m"}), "m", 0);
        assert!(matches!(err, Err(ProviderError::InvalidResponse(_))));
    }

    #[test]
    fn tool_call_arguments_may_be_an_object() -> Result<(), Box<dyn Error>> {
        let json = json!({"choices": [{"message": {"content": null, "tool_calls": [
            {"id": "c", "type": "function", "function": {"name": "f", "arguments": {"a": 1}}}
        ]}}]});
        let resp = parse_chat(&json, "m", 0)?;
        assert_eq!(resp.tool_calls[0].arguments, "{\"a\":1}");
        assert!(resp.content.is_none());
        Ok(())
    }

    #[test]
    fn sse_accumulates_content_and_stops_at_done() -> Result<(), Box<dyn Error>> {
        let mut acc = StreamAccumulator::new(Instant::now());
        let mut pieces = Vec::new();
        let mut sink = |piece: &str| pieces.push(piece.to_owned());
        assert!(!acc.feed_line("", &mut sink)?);
        assert!(!acc.feed_line(": keep-alive", &mut sink)?);
        assert!(!acc.feed_line(
            r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#,
            &mut sink
        )?);
        assert!(!acc.feed_line(r#"data: {"choices":[{"delta":{"content":""}}]}"#, &mut sink)?);
        assert!(!acc.feed_line(
            r#"data:{"choices":[{"delta":{"content":"lo"}}]}"#,
            &mut sink
        )?);
        assert!(acc.feed_line("data: [DONE]", &mut sink)?);
        let resp = acc.finish("m");
        assert_eq!(pieces, vec!["Hel", "lo"]);
        assert_eq!(resp.content.as_deref(), Some("Hello"));
        assert!(resp.first_token_ms.is_some());
        Ok(())
    }

    #[test]
    fn sse_merges_tool_call_fragments_by_index() -> Result<(), Box<dyn Error>> {
        let mut acc = StreamAccumulator::new(Instant::now());
        let mut sink = |_: &str| {};
        acc.feed_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_7","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
            &mut sink,
        )?;
        acc.feed_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a.rs\"}"}}]}}]}"#,
            &mut sink,
        )?;
        acc.feed_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"name":"list_dir","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut sink,
        )?;
        let resp = acc.finish("m");
        assert_eq!(resp.tool_calls.len(), 2);
        assert_eq!(resp.tool_calls[0].id, "call_7");
        assert_eq!(resp.tool_calls[0].name, "read_file");
        assert_eq!(resp.tool_calls[0].arguments, "{\"path\":\"a.rs\"}");
        assert_eq!(resp.tool_calls[1].name, "list_dir");
        assert_eq!(resp.finish_reason.as_deref(), Some("tool_calls"));
        Ok(())
    }

    #[test]
    fn sse_takes_usage_from_final_chunk_and_reasoning_separately() -> Result<(), Box<dyn Error>> {
        let mut acc = StreamAccumulator::new(Instant::now());
        let mut pieces = Vec::new();
        let mut sink = |piece: &str| pieces.push(piece.to_owned());
        acc.feed_line(
            r#"data: {"choices":[{"delta":{"reasoning_content":"think"}}]}"#,
            &mut sink,
        )?;
        acc.feed_line(
            r#"data: {"choices":[],"usage":{"prompt_tokens":4,"completion_tokens":2}}"#,
            &mut sink,
        )?;
        let resp = acc.finish("m");
        assert!(pieces.is_empty(), "reasoning is not content");
        assert_eq!(resp.reasoning.as_deref(), Some("think"));
        assert!(resp.content.is_none());
        assert!(resp.first_token_ms.is_none());
        assert_eq!(resp.usage.completion_tokens, Some(2));
        Ok(())
    }

    #[test]
    fn sse_error_chunk_is_reported() {
        let mut acc = StreamAccumulator::new(Instant::now());
        let mut sink = |_: &str| {};
        let err = acc.feed_line(
            r#"data: {"error":{"message":"context exceeded"}}"#,
            &mut sink,
        );
        assert!(matches!(err, Err(ProviderError::Other(msg)) if msg.contains("context exceeded")));
    }

    #[test]
    fn sse_garbage_payload_is_invalid_response() {
        let mut acc = StreamAccumulator::new(Instant::now());
        let mut sink = |_: &str| {};
        let err = acc.feed_line("data: {not json", &mut sink);
        assert!(matches!(err, Err(ProviderError::InvalidResponse(_))));
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let text = "é".repeat(10); // 2 bytes per char
        let cut = truncate_utf8(text, 5);
        assert_eq!(cut, "éé");
    }

    #[test]
    fn transient_errors_are_limited_to_connection_and_loading() {
        assert!(is_transient(&ProviderError::Unreachable("x".into())));
        assert!(is_transient(&ProviderError::Http {
            status: 503,
            body: String::new()
        }));
        assert!(!is_transient(&ProviderError::Http {
            status: 502,
            body: String::new()
        }));
        assert!(!is_transient(&ProviderError::Http {
            status: 400,
            body: String::new()
        }));
        assert!(!is_transient(&ProviderError::Http {
            status: 500,
            body: String::new()
        }));
        assert!(!is_transient(&ProviderError::Timeout(Duration::from_secs(
            1
        ))));
    }

    #[test]
    fn f32_values_serialise_in_short_form() {
        assert_eq!(f32_json(0.7), json!(0.7));
        assert_eq!(f32_json(1.0), json!(1.0));
    }
}
