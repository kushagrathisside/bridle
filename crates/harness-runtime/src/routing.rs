//! Routing, asked: the decision model answers `MAIN` or `DEEP` for a request the rules cannot place. The pure part
//! (the rules, the answer check and the final choice) is `harness_core::routing`.
//!
//! The model sees the request text after redaction, so a secret typed into a request does not reach it. It sees no
//! repository content and no tool output. A failed call or a rejected answer is not an error for the caller: the
//! request goes to `MAIN`, and the reason says why.

use harness_core::routing::{self, Answer, Placed, RejectReason};
use harness_core::security::redact::Redactor;
use serde_json::json;

use crate::providers::{ChatMessage, ChatRequest, ModelProvider};

/// The longest request text sent to the model, in characters. A longer request is cut before it is asked about.
const MAX_REQUEST_CHARS: usize = 2_000;

/// The request for one undecided placement.
pub fn request(text: &str) -> ChatRequest {
    let redacted = Redactor::new().redact(text);
    let bounded: String = redacted.chars().take(MAX_REQUEST_CHARS).collect();
    let system = format!(
        "You are the routing step of a coding-agent harness. Decide which model answers a request. Choose exactly one: \
         MAIN for everyday work (small edits, questions about the code, running tests, documentation); DEEP for work \
         that needs a wider view (designing an architecture, finding a cause across several modules, a long review). \
         Reply with one JSON object and no other text: {{\"choice\": \"<one of {choices:?}>\", \"reason\": \"<one short sentence>\"}}.",
        choices = routing::CHOICES
    );
    ChatRequest {
        messages: vec![
            ChatMessage::system(system),
            ChatMessage::user(format!("Request: {bounded}")),
        ],
        response_format: Some(json!({"type": "json_object"})),
        max_tokens: Some(200),
        enable_thinking: Some(false),
        ..ChatRequest::default()
    }
}

/// Asks the model about a request the rules did not place, and checks its answer.
pub fn ask(provider: &dyn ModelProvider, text: &str) -> Result<Answer, String> {
    let response = provider
        .chat(&request(text))
        .map_err(|error| error.to_string())?;
    let content = response
        .content
        .ok_or_else(|| "the decision model gave no answer".to_owned())?;
    routing::parse_answer(&content).map_err(|reason: RejectReason| reason.to_string())
}

/// Places a request end to end: the rules first, the model for the rest, and the final choice. `deep_serving` says
/// whether the deep slot is running. The returned reason is shown to the person.
pub fn place_request(
    provider: Option<&dyn ModelProvider>,
    text: &str,
    deep_serving: bool,
) -> (harness_core::routing::Slot, String) {
    let placed = routing::place(text);
    let answer = match (&placed, provider) {
        (Placed::Undecided, Some(provider)) => Some(ask(provider, text)),
        _ => None,
    };
    routing::choose(placed, answer, deep_serving)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_request_carries_the_closed_choices_and_the_redacted_text() {
        let req = request("Design the cache. The key is api_key = supersecret123 in the config.");
        let system = req.messages[0].content.clone().unwrap_or_default();
        assert!(
            system.contains("MAIN") && system.contains("DEEP"),
            "{system}"
        );
        let user = req.messages[1].content.clone().unwrap_or_default();
        assert!(user.starts_with("Request: Design the cache."), "{user}");
        assert!(!user.contains("supersecret123"), "{user}");
        assert_eq!(req.enable_thinking, Some(false));
    }

    #[test]
    fn a_long_request_is_cut_before_it_is_asked_about() {
        let long = "design ".repeat(1_000);
        let user = request(&long).messages[1]
            .content
            .clone()
            .unwrap_or_default();
        assert!(user.chars().count() <= "Request: ".len() + MAX_REQUEST_CHARS);
    }

    #[test]
    fn a_placed_request_never_asks_the_model() {
        // A short request is placed by rule, so no provider is needed and none is called.
        let (slot, reason) = place_request(None, "fix the typo", true);
        assert_eq!(slot, harness_core::routing::Slot::Main);
        assert_eq!(reason, "a short request with no hard word");
    }

    #[test]
    fn an_undecided_request_without_a_provider_goes_to_main() {
        let (slot, reason) = place_request(
            None,
            "Redesign the storage architecture for the whole project",
            true,
        );
        assert_eq!(slot, harness_core::routing::Slot::Main);
        assert!(reason.contains("no decision answer"), "{reason}");
    }
}
