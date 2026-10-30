//! Model providers: every LLM call goes through the [`ModelProvider`] trait, so the harness is
//! never hard-coded against one server.
//!
//! The API is synchronous on purpose: the CLI and hook processes are short-lived and make one call
//! at a time, so an async runtime would add weight without benefit.

pub(crate) mod llama_cpp;
mod mock;
pub(crate) mod openai_compat;

pub use llama_cpp::LlamaCppProvider;
pub use mock::MockProvider;
pub use openai_compat::OpenAiCompatProvider;

use std::time::Duration;

use harness_core::config::{ModelConfig, ProviderKind};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    /// Could not connect / DNS / connection reset.
    #[error("endpoint unreachable: {0}")]
    Unreachable(String),
    #[error("request timed out after {0:?}")]
    Timeout(Duration),
    /// Non-2xx HTTP status. `body` is truncated to 2 KiB.
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    /// The server answered but the payload did not match the OpenAI schema.
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("missing API key: environment variable {0} is not set")]
    MissingApiKey(String),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool call emitted by the model (OpenAI `tool_calls[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    /// Function name.
    pub name: String,
    /// Raw JSON-encoded arguments string, exactly as returned (models can emit invalid JSON).
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(text.into()),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
}

/// A function tool offered to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments object.
    pub parameters: serde_json::Value,
}

/// A chat completion request. Generation parameters left `None` fall back to the model config.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// OpenAI `tool_choice` (`"auto"`, `"required"`, or an object).
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
    /// OpenAI `response_format`, e.g. `{"type":"json_schema","json_schema":{...}}`.
    #[serde(default)]
    pub response_format: Option<serde_json::Value>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Override the model config's `reasoning.enabled` for this request.
    #[serde(default)]
    pub enable_thinking: Option<bool>,
}

/// Token usage reported by the server, when available.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

/// Server-side timings (llama.cpp `timings` object), used to separate prefill from generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    pub prompt_ms: Option<f64>,
    pub prompt_per_second: Option<f64>,
    pub predicted_ms: Option<f64>,
    pub predicted_per_second: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    /// Model id reported by the server.
    pub model: String,
    pub content: Option<String>,
    /// Thinking text, when the server separates it (`reasoning_content`).
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: Usage,
    pub timings: Option<Timings>,
    /// Wall-clock latency measured by the client.
    pub latency_ms: u64,
    /// Time to first streamed token; only set by `chat_stream`.
    pub first_token_ms: Option<u64>,
}

/// Liveness as reported by the endpoint itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthStatus {
    pub ok: bool,
    /// Raw status text, e.g. llama.cpp `{"status":"ok"}` / `loading model`.
    pub detail: String,
}

/// What a stream reports while it is read (see [`ModelProvider::chat_stream_with`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEvent<'a> {
    /// A piece of the answer text.
    Text(&'a str),
    /// The model is writing a tool call. `name` is empty until the server has sent it; `chars` is the
    /// length of the argument text so far.
    ToolCall { name: &'a str, chars: usize },
    /// The model is producing reasoning that is not part of the answer; `chars` is its length so far.
    Reasoning { chars: usize },
}

pub trait ModelProvider: Send + Sync {
    /// Model id this provider sends in requests.
    fn model_id(&self) -> &str;

    /// Liveness probe. llama.cpp: `GET <root>/health`; generic: `GET <base>/models`.
    fn health(&self) -> Result<HealthStatus, ProviderError>;

    /// `GET <base>/models` → model ids.
    fn list_models(&self) -> Result<Vec<String>, ProviderError>;

    /// Non-streaming chat completion.
    fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError>;

    /// Streaming chat completion (SSE). Calls `on_delta` with each content delta and returns the
    /// assembled response (content, tool calls, usage if sent, `first_token_ms`).
    fn chat_stream(
        &self,
        req: &ChatRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatResponse, ProviderError>;

    /// Like [`chat_stream`](Self::chat_stream), and reports what arrives, including the parts that are not
    /// text. When `on_event` returns `false` the stream is abandoned: the connection is dropped, so the
    /// server stops generating, and the response holds what had arrived, with `finish_reason` set to
    /// `stopped` and no tool calls (one that was still being written is incomplete and must not be run).
    ///
    /// The default implementation reports text only and cannot stop early.
    fn chat_stream_with(
        &self,
        req: &ChatRequest,
        on_event: &mut dyn FnMut(StreamEvent<'_>) -> bool,
    ) -> Result<ChatResponse, ProviderError> {
        self.chat_stream(req, &mut |piece| {
            let _ = on_event(StreamEvent::Text(piece));
        })
    }

    /// Server introspection (llama.cpp `GET <root>/props`). `None` for servers without it.
    fn server_props(&self) -> Result<Option<serde_json::Value>, ProviderError> {
        Ok(None)
    }
}

/// Builds the provider for a model slot. `env` resolves `api_key_env` (injected for tests).
pub fn provider_for(
    model: &ModelConfig,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Box<dyn ModelProvider>, ProviderError> {
    let api_key = match &model.api_key_env {
        Some(var) => Some(env(var).ok_or_else(|| ProviderError::MissingApiKey(var.clone()))?),
        None => None,
    };
    Ok(match model.provider {
        ProviderKind::OpenaiCompatible | ProviderKind::Litellm => {
            Box::new(OpenAiCompatProvider::new(model, api_key))
        }
        ProviderKind::LlamaCpp | ProviderKind::PrismBonsai => {
            Box::new(LlamaCppProvider::new(model, api_key))
        }
        ProviderKind::Mock => Box::new(MockProvider::echo(&model.id)),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod gateway_tests {
    use super::*;
    use harness_core::config::Config;

    const HYBRID: &str = include_str!("../../../../configs/hybrid.toml");

    /// A live check of the gateway path: a LiteLLM gateway in front of a running local model. The gateway's address
    /// comes from `BRIDLE_GATEWAY_URL` (default: the hybrid profile's endpoint), and its key from `BRIDLE_GATEWAY_KEY`.
    /// Run with `--ignored` while the gateway and the model are up.
    #[test]
    #[ignore = "needs a running LiteLLM gateway"]
    fn a_gateway_slot_completes_a_request() {
        let mut config: Config = toml::from_str(HYBRID).unwrap();
        let slot = config
            .models
            .get_mut("gateway")
            .expect("the hybrid profile has a gateway slot");
        if let Ok(url) = std::env::var("BRIDLE_GATEWAY_URL") {
            slot.endpoint = url;
        }
        let model = slot.clone();
        let provider = provider_for(&model, &|name| std::env::var(name).ok())
            .expect("the gateway slot builds a provider");
        let response = provider
            .chat(&ChatRequest {
                messages: vec![ChatMessage::user("Reply with the single word: ok")],
                max_tokens: Some(16),
                ..ChatRequest::default()
            })
            .expect("the gateway answers");
        assert!(
            response
                .content
                .as_deref()
                .is_some_and(|c| !c.trim().is_empty()),
            "the gateway gave no text"
        );
    }
}
