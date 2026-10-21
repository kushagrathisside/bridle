//! llama.cpp `llama-server` provider: OpenAI-compatible chat plus server introspection.
//! Also used for the PrismML fork (Bonsai), which speaks the same protocol.
//!
//! Chat traffic is delegated to [`OpenAiCompatProvider`]. `/health` and `/props` are
//! llama.cpp-specific and are handled here. `/health` answers 503 while a model loads; that is
//! reported as "not ready", not as an error.

use harness_core::config::ModelConfig;
use serde_json::Value;

use super::openai_compat::{RawReply, http_error};
use super::{
    ChatRequest, ChatResponse, HealthStatus, ModelProvider, OpenAiCompatProvider, ProviderError,
};

/// llama.cpp server. Chat traffic goes through the OpenAI-compatible client; `/health` and
/// `/props` are llama.cpp-specific and handled here.
#[derive(Debug)]
pub struct LlamaCppProvider {
    inner: OpenAiCompatProvider,
}

impl LlamaCppProvider {
    /// Builds the provider for a `llama_cpp` or `prism_bonsai` model slot.
    pub fn new(model: &ModelConfig, api_key: Option<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(model, api_key),
        }
    }

    /// Server root without the `/v1` suffix, e.g. `http://127.0.0.1:8001`.
    pub fn root_url(&self) -> String {
        root_of(self.inner.base_url())
    }
}

/// Removes trailing slashes and a final `/v1` segment.
fn root_of(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_owned()
}

/// Human-readable health text: the `status` field (or `error.message`) when the body is JSON
/// that carries one, otherwise the trimmed raw body.
fn health_detail(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("status")
                .and_then(Value::as_str)
                .or_else(|| value.pointer("/error/message").and_then(Value::as_str))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.trim().to_owned())
}

impl ModelProvider for LlamaCppProvider {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn health(&self) -> Result<HealthStatus, ProviderError> {
        // Not retried: a probe should answer at once, and 503 ("loading") is a status, not a failure.
        let reply = self.inner.get_raw(&format!("{}/health", self.root_url()))?;
        match reply.status {
            200..=299 => Ok(HealthStatus {
                ok: true,
                detail: health_detail(&reply.body),
            }),
            503 => Ok(HealthStatus {
                ok: false,
                detail: health_detail(&reply.body),
            }),
            status => Err(http_error(status, reply.body)),
        }
    }

    fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        self.inner.list_models()
    }

    fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.inner.chat(req)
    }

    fn chat_stream(
        &self,
        req: &ChatRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatResponse, ProviderError> {
        self.inner.chat_stream(req, on_delta)
    }

    fn server_props(&self) -> Result<Option<Value>, ProviderError> {
        let RawReply { status, body } =
            self.inner.get_raw(&format!("{}/props", self.root_url()))?;
        match status {
            200..=299 => serde_json::from_str::<Value>(&body)
                .map(Some)
                .map_err(|e| ProviderError::InvalidResponse(format!("/props: {e}"))),
            404 => Ok(None),
            status => Err(http_error(status, body)),
        }
    }
}

/// `default_generation_settings.n_ctx`, falling back to a top-level `n_ctx`.
#[allow(dead_code)] // Spec helper with no caller in M1; covered by this module's unit tests.
pub fn props_context(props: &Value) -> Option<u64> {
    props
        .pointer("/default_generation_settings/n_ctx")
        .and_then(Value::as_u64)
        .or_else(|| props.get("n_ctx").and_then(Value::as_u64))
}

/// The `build_info` string, when the server reports one.
#[allow(dead_code)] // Spec helper with no caller in M1; covered by this module's unit tests.
pub fn props_build(props: &Value) -> Option<String> {
    props
        .get("build_info")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `model_path` string, when the server reports one.
#[allow(dead_code)] // Spec helper with no caller in M1; covered by this module's unit tests.
pub fn props_model_path(props: &Value) -> Option<String> {
    props
        .get("model_path")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Whether speculative decoding (a draft model) is enabled.
///
/// Looks at every leaf value whose key path mentions `speculative` or `draft`. Returns
/// `Some(true)` if any such leaf is truthy, `Some(false)` if there are such leaves but all are
/// falsy, and `None` if the server reports no such key at all.
#[allow(dead_code)] // Spec helper with no caller in M1; covered by this module's unit tests.
pub fn props_speculative(props: &Value) -> Option<bool> {
    let mut found = None;
    speculative_scan(props, false, &mut found);
    found
}

fn speculative_scan(value: &Value, under_draft_key: bool, found: &mut Option<bool>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, child) in map {
                let lowered = key.to_ascii_lowercase();
                let hit =
                    under_draft_key || lowered.contains("speculative") || lowered.contains("draft");
                speculative_scan(child, hit, found);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for item in items {
                speculative_scan(item, under_draft_key, found);
            }
        }
        leaf if under_draft_key => {
            let truthy = is_truthy(leaf);
            *found = Some(found.unwrap_or(false) || truthy);
        }
        _ => {}
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|x| x != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn root_url_strips_v1_and_slashes() {
        assert_eq!(root_of("http://127.0.0.1:8001/v1"), "http://127.0.0.1:8001");
        assert_eq!(
            root_of("http://127.0.0.1:8001/v1/"),
            "http://127.0.0.1:8001"
        );
        assert_eq!(root_of("http://host:8001"), "http://host:8001");
        assert_eq!(root_of("http://host/proxy/v1"), "http://host/proxy");
    }

    #[test]
    fn health_detail_prefers_status_then_error_message() {
        assert_eq!(health_detail(r#"{"status":"ok"}"#), "ok");
        assert_eq!(
            health_detail(r#"{"status":"loading model"}"#),
            "loading model"
        );
        assert_eq!(
            health_detail(r#"{"error":{"code":503,"message":"Loading model"}}"#),
            "Loading model"
        );
        assert_eq!(health_detail("  plain text \n"), "plain text");
    }

    #[test]
    fn props_context_reads_nested_then_top_level() {
        let nested = json!({"default_generation_settings": {"n_ctx": 8192}, "n_ctx": 1});
        assert_eq!(props_context(&nested), Some(8192));
        assert_eq!(props_context(&json!({"n_ctx": 4096})), Some(4096));
        assert_eq!(props_context(&json!({})), None);
    }

    #[test]
    fn props_build_and_model_path_read_strings_only() {
        let props = json!({"build_info": "b11756-abc", "model_path": "/models/q.gguf"});
        assert_eq!(props_build(&props).as_deref(), Some("b11756-abc"));
        assert_eq!(props_model_path(&props).as_deref(), Some("/models/q.gguf"));
        assert_eq!(props_build(&json!({"build_info": 7})), None);
        assert_eq!(props_model_path(&json!({})), None);
    }

    #[test]
    fn props_speculative_is_none_without_such_keys() {
        assert_eq!(props_speculative(&json!({"n_ctx": 4096})), None);
        assert_eq!(props_speculative(&json!({})), None);
    }

    #[test]
    fn props_speculative_reports_truthy_and_falsy_settings() {
        assert_eq!(
            props_speculative(&json!({"speculative": {"n_max": 0}})),
            Some(false)
        );
        assert_eq!(
            props_speculative(&json!({"speculative": {"n_max": 16}})),
            Some(true)
        );
        assert_eq!(
            props_speculative(&json!({"draft_model": "small.gguf"})),
            Some(true)
        );
        assert_eq!(
            props_speculative(&json!({"draft_model": null})),
            Some(false)
        );
        assert_eq!(
            props_speculative(&json!({"speculative": false, "draft": {"n": 0}})),
            Some(false)
        );
    }
}
