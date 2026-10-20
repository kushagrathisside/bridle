//! Scripted provider for tests (R-7.2 `MockProvider`). Never touches the network.
//!
//! IMPLEMENTATION SPEC (delegated):
//! - `MockProvider::echo(model_id)`: every `chat` returns content `"OK"`... more precisely: if the
//!   request offers tools, returns a tool call to the first tool with arguments `{}`; otherwise
//!   returns the last user message content prefixed with `"echo: "`. Streaming splits the content
//!   into whitespace-separated pieces and calls `on_delta` per piece.
//! - `MockProvider::scripted(model_id, responses)`: returns the queued `Result`s in order (a
//!   `Mutex<VecDeque<...>>`); when the queue is empty, returns `ProviderError::Other("script
//!   exhausted")`. Records every request it receives (`requests()` returns clones) so tests can
//!   assert on what the harness sent.
//! - `health` → ok unless constructed with `.unhealthy()`.
//! - Unit tests for each behaviour.

use super::{ChatRequest, ChatResponse, HealthStatus, ModelProvider, ProviderError};

#[derive(Debug)]
pub struct MockProvider {
    model_id: String,
}

impl MockProvider {
    pub fn echo(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_owned(),
        }
    }
}

impl ModelProvider for MockProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn health(&self) -> Result<HealthStatus, ProviderError> {
        todo!()
    }
    fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec![self.model_id.clone()])
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
