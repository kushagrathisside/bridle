//! llama.cpp `llama-server` provider: OpenAI-compatible chat plus server introspection.
//! Also used for the PrismML fork (Bonsai), which speaks the same protocol.
//!
//! IMPLEMENTATION SPEC (delegated):
//! - Wraps an [`OpenAiCompatProvider`] for chat/list_models/chat_stream (delegate).
//! - `root_url`: the endpoint with a trailing `/v1` (and trailing `/`) removed,
//!   e.g. `http://127.0.0.1:8001/v1` → `http://127.0.0.1:8001`.
//! - `health`: GET `<root>/health`. 200 with `{"status":"ok"}` → ok. 503 (model loading) →
//!   `HealthStatus { ok: false, detail: <status or body> }` (not an error). Other non-2xx → `Http`.
//! - `server_props`: GET `<root>/props` → `Some(json)`; 404 → `Ok(None)`.
//! - Helpers (pure, `pub`, unit-tested): `props_context(&Value) -> Option<u64>` reads
//!   `default_generation_settings.n_ctx` (fallback: top-level `n_ctx`);
//!   `props_build(&Value) -> Option<String>` reads `build_info` (string) when present;
//!   `props_model_path(&Value) -> Option<String>` reads `model_path`;
//!   `props_speculative(&Value) -> Option<bool>`: true if any key path contains `speculative` or
//!   `draft` with a truthy/non-empty value, `None` if no such key exists.

use harness_core::config::ModelConfig;

use super::{
    ChatRequest, ChatResponse, HealthStatus, ModelProvider, OpenAiCompatProvider, ProviderError,
};

#[derive(Debug)]
pub struct LlamaCppProvider {
    inner: OpenAiCompatProvider,
}

impl LlamaCppProvider {
    pub fn new(model: &ModelConfig, api_key: Option<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(model, api_key),
        }
    }

    pub fn root_url(&self) -> String {
        todo!()
    }
}

impl ModelProvider for LlamaCppProvider {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn health(&self) -> Result<HealthStatus, ProviderError> {
        todo!()
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
    fn server_props(&self) -> Result<Option<serde_json::Value>, ProviderError> {
        todo!()
    }
}
