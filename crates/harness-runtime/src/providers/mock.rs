//! Scripted provider for tests (`MockProvider`). Never touches the network.
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

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::{
    ChatRequest, ChatResponse, HealthStatus, ModelProvider, ProviderError, Role, ToolCall,
};

/// Test double for [`ModelProvider`]. In echo mode it answers deterministically; in scripted mode
/// it replays queued results and records every request it receives.
#[derive(Debug)]
pub struct MockProvider {
    model_id: String,
    /// Queued results consumed by `chat` and `chat_stream`. `None` selects echo mode.
    script: Option<Mutex<VecDeque<Result<ChatResponse, ProviderError>>>>,
    requests: Mutex<Vec<ChatRequest>>,
    healthy: bool,
}

impl MockProvider {
    /// Echo mode: a request that offers tools gets a call to its first tool with `{}` arguments;
    /// any other request gets its last user message back, prefixed with `echo: `.
    pub fn echo(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_owned(),
            script: None,
            requests: Mutex::new(Vec::new()),
            healthy: true,
        }
    }

    /// Scripted mode: each `chat` or `chat_stream` call returns the next queued result. Once the
    /// queue is empty, calls fail with `ProviderError::Other("script exhausted")`.
    pub fn scripted(model_id: &str, responses: Vec<Result<ChatResponse, ProviderError>>) -> Self {
        Self {
            model_id: model_id.to_owned(),
            script: Some(Mutex::new(responses.into())),
            requests: Mutex::new(Vec::new()),
            healthy: true,
        }
    }

    /// Makes `health` report `ok: false`.
    #[must_use]
    pub fn unhealthy(mut self) -> Self {
        self.healthy = false;
        self
    }

    /// Every request this provider has received, in arrival order.
    pub fn requests(&self) -> Vec<ChatRequest> {
        lock(&self.requests).clone()
    }

    fn record(&self, req: &ChatRequest) {
        lock(&self.requests).push(req.clone());
    }

    fn next_response(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        match &self.script {
            Some(queue) => lock(queue)
                .pop_front()
                .unwrap_or_else(|| Err(ProviderError::Other("script exhausted".to_owned()))),
            None => Ok(self.echo_response(req)),
        }
    }

    fn echo_response(&self, req: &ChatRequest) -> ChatResponse {
        if let Some(tool) = req.tools.first() {
            return ChatResponse {
                model: self.model_id.clone(),
                tool_calls: vec![ToolCall {
                    id: "mock-call-0".to_owned(),
                    name: tool.name.clone(),
                    arguments: "{}".to_owned(),
                }],
                finish_reason: Some("tool_calls".to_owned()),
                ..ChatResponse::default()
            };
        }
        let last_user = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.content.as_deref())
            .unwrap_or("");
        ChatResponse {
            model: self.model_id.clone(),
            content: Some(format!("echo: {last_user}")),
            finish_reason: Some("stop".to_owned()),
            ..ChatResponse::default()
        }
    }
}

impl ModelProvider for MockProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn health(&self) -> Result<HealthStatus, ProviderError> {
        let detail = if self.healthy {
            "ok"
        } else {
            "mock: unhealthy"
        };
        Ok(HealthStatus {
            ok: self.healthy,
            detail: detail.to_owned(),
        })
    }

    fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec![self.model_id.clone()])
    }

    fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.record(req);
        self.next_response(req)
    }

    fn chat_stream(
        &self,
        req: &ChatRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatResponse, ProviderError> {
        self.record(req);
        let response = self.next_response(req)?;
        if let Some(text) = response.content.as_deref() {
            // Each piece keeps its trailing whitespace, so concatenating the deltas rebuilds `text`.
            for piece in text.split_inclusive(char::is_whitespace) {
                on_delta(piece);
            }
        }
        Ok(response)
    }
}

/// Locks ignoring poisoning: a panic elsewhere must not cascade into every later mock call.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ChatMessage, ToolSpec};

    fn request(text: &str) -> ChatRequest {
        ChatRequest {
            messages: vec![ChatMessage::user(text)],
            ..ChatRequest::default()
        }
    }

    fn reply(text: &str) -> ChatResponse {
        ChatResponse {
            model: "m".to_owned(),
            content: Some(text.to_owned()),
            ..ChatResponse::default()
        }
    }

    #[test]
    fn echo_repeats_the_last_user_message() {
        let provider = MockProvider::echo("m");
        let response = provider.chat(&request("hello there")).expect("echo");
        assert_eq!(response.content.as_deref(), Some("echo: hello there"));
        assert_eq!(response.model, "m");
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn echo_calls_the_first_offered_tool_with_empty_arguments() {
        let provider = MockProvider::echo("m");
        let mut req = request("use a tool");
        req.tools = vec![ToolSpec {
            name: "get_weather".to_owned(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }];
        let response = provider.chat(&req).expect("echo");
        assert_eq!(response.content, None);
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "get_weather");
        assert_eq!(response.tool_calls[0].arguments, "{}");
    }

    #[test]
    fn echo_stream_deltas_rebuild_the_content() {
        let provider = MockProvider::echo("m");
        let mut deltas = Vec::new();
        let response = provider
            .chat_stream(&request("one two"), &mut |d: &str| {
                deltas.push(d.to_owned())
            })
            .expect("stream");
        assert!(deltas.len() > 1);
        assert_eq!(deltas.concat(), "echo: one two");
        assert_eq!(response.content.as_deref(), Some("echo: one two"));
    }

    #[test]
    fn scripted_replays_results_in_order_then_exhausts() {
        let provider = MockProvider::scripted(
            "m",
            vec![
                Ok(reply("first")),
                Err(ProviderError::Unreachable("down".to_owned())),
                Ok(reply("third")),
            ],
        );
        let first = provider.chat(&request("a")).expect("first");
        assert_eq!(first.content.as_deref(), Some("first"));
        assert!(matches!(
            provider.chat(&request("b")),
            Err(ProviderError::Unreachable(_))
        ));
        let third = provider.chat(&request("c")).expect("third");
        assert_eq!(third.content.as_deref(), Some("third"));
        assert!(matches!(
            provider.chat(&request("d")),
            Err(ProviderError::Other(msg)) if msg == "script exhausted"
        ));
    }

    #[test]
    fn scripted_stream_consumes_the_script_and_emits_scripted_deltas() {
        let provider = MockProvider::scripted(
            "m",
            vec![
                Ok(reply("1 2 3 4 5")),
                Err(ProviderError::Other("boom".to_owned())),
            ],
        );
        let mut deltas = Vec::new();
        let response = provider
            .chat_stream(&request("count"), &mut |d: &str| deltas.push(d.to_owned()))
            .expect("stream");
        assert_eq!(deltas, ["1 ", "2 ", "3 ", "4 ", "5"]);
        assert_eq!(response.content.as_deref(), Some("1 2 3 4 5"));

        // A queued error is returned without emitting any delta.
        let mut seen = 0;
        assert!(
            provider
                .chat_stream(&request("again"), &mut |_: &str| seen += 1)
                .is_err()
        );
        assert_eq!(seen, 0);
    }

    #[test]
    fn requests_are_recorded_for_chat_and_stream() {
        let provider = MockProvider::scripted("m", vec![Ok(reply("a")), Ok(reply("b"))]);
        provider.chat(&request("one")).expect("chat");
        provider
            .chat_stream(&request("two"), &mut |_: &str| {})
            .expect("stream");
        let seen = provider.requests();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].messages[0].content.as_deref(), Some("two"));
    }

    #[test]
    fn health_is_ok_unless_marked_unhealthy() {
        assert!(MockProvider::echo("m").health().expect("health").ok);
        let down = MockProvider::echo("m")
            .unhealthy()
            .health()
            .expect("health");
        assert!(!down.ok);
    }

    #[test]
    fn list_models_reports_the_configured_id() {
        let provider = MockProvider::echo("m");
        assert_eq!(provider.list_models().expect("list"), ["m"]);
        assert_eq!(provider.model_id(), "m");
    }

    #[test]
    fn poisoned_lock_does_not_panic() {
        let provider = MockProvider::echo("m");
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = provider.requests.lock().expect("fresh lock");
            panic!("poison the requests lock");
        }));
        assert!(provider.requests().is_empty());
        assert!(provider.chat(&request("still works")).is_ok());
        assert_eq!(provider.requests().len(), 1);
    }

    #[test]
    fn mock_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MockProvider>();
    }
}
