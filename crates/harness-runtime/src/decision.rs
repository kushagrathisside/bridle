//! The LFM decision engine (M4): on a stalled loop, asks the micro slot for one of the answers in
//! `harness_core::lfm::CHOICES`. Every other case, and every error, is decided by the rules (R-30).
//!
//! The model sees a bounded summary of the task: the phase, the counters, the last action and the names
//! of the loop signals. It sees no prompt text, no command text and no tool output.

use std::time::Instant;

use harness_core::decision::{
    Decision, DecisionEngine, DecisionInput, DecisionResult, RulesDecisionEngine,
};
use harness_core::lfm::{self, CHOICES};
use harness_core::loops::LoopSignal;
use serde_json::json;

use crate::providers::{ChatMessage, ChatRequest, ModelProvider};

/// The name recorded for decisions made by the model.
pub const PROVIDER_NAME: &str = "local_lfm";
/// The label of a decision that was escalated to the deep slot, whether the call succeeded or not. Counting the
/// recorded decisions with this label counts the escalations a task has used (R-18.1).
pub const ESCALATED_PREFIX: &str = "escalated to deep";
/// The longest time a deep escalation may take. The hook that asks has a limit of its own (the shim gives up after
/// 20 s and blocks the call), so the deep slot's own timeout is not used for this call.
pub const ESCALATION_TIMEOUT_MS: u64 = 15_000;
/// The longest action text sent to the model.
const MAX_ACTION: usize = 120;

/// A decision engine that asks a model for stalled loops.
pub struct LfmEngine {
    provider: Box<dyn ModelProvider>,
}

impl std::fmt::Debug for LfmEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LfmEngine")
            .field("model", &self.provider.model_id())
            .finish()
    }
}

impl LfmEngine {
    pub fn new(provider: Box<dyn ModelProvider>) -> Self {
        Self { provider }
    }

    /// The request for a stalled loop.
    pub fn request(input: &DecisionInput<'_>) -> ChatRequest {
        let state = input.state;
        let action = state
            .last_action
            .as_deref()
            .unwrap_or("none")
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_ACTION)
            .collect::<String>();
        let signals: Vec<&str> = input.signals.iter().map(signal_name).collect();
        let system = format!(
            "You are the decision step of a coding-agent harness. A loop has stalled. Choose exactly one of: \
             ESCALATE (stop and report the blocker), ASK_USER (ask the person for a decision), REPLAN (make a \
             new plan from the current state). Reply with one JSON object and no other text: \
             {{\"decision\": \"<one of {CHOICES:?}>\", \"reason\": \"<one short sentence>\"}}."
        );
        let user = format!(
            "Phase: {}. Iteration: {}. Failures in a row: {}. Refusals in a row: {}. Last action: {}. \
             Stalled loop: {}.",
            state.phase.name(),
            state.iteration,
            state.failure_count,
            state.consecutive_refusals,
            action,
            signals.join(", "),
        );
        ChatRequest {
            messages: vec![ChatMessage::system(system), ChatMessage::user(user)],
            response_format: Some(json!({"type": "json_object"})),
            max_tokens: Some(200),
            enable_thinking: Some(false),
            ..ChatRequest::default()
        }
    }

    fn ask(&self, input: &DecisionInput<'_>) -> Result<(Decision, String), String> {
        let response = self
            .provider
            .chat(&Self::request(input))
            .map_err(|error| error.to_string())?;
        let content = response
            .content
            .ok_or_else(|| "the model gave no answer".to_owned())?;
        let choice = lfm::parse_choice(&content).map_err(|reason| reason.to_string())?;
        Ok((choice.decision, choice.reason))
    }
}

impl LfmEngine {
    /// Asks the model about a case the rules or the primary engine escalated. Unlike [`DecisionEngine::decide`],
    /// it asks even when the rules decided, because the escalation is the reason for asking. A failed call keeps
    /// the escalated decision, so the case still goes to the person.
    pub fn escalate(&self, input: &DecisionInput<'_>, escalated: DecisionResult) -> DecisionResult {
        let started = Instant::now();
        let outcome = self.ask(input);
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match outcome {
            Ok((decision, reason)) => DecisionResult {
                decision,
                confidence: None,
                provider: format!("{ESCALATED_PREFIX} ({})", self.provider.model_id()),
                latency_ms,
                model_reason: Some(reason),
                ..escalated
            },
            Err(error) => DecisionResult {
                provider: format!(
                    "{ESCALATED_PREFIX} failed ({error}); kept {}",
                    escalated.provider
                ),
                latency_ms,
                ..escalated
            },
        }
    }
}

impl DecisionEngine for LfmEngine {
    fn provider(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn decide(&self, input: &DecisionInput<'_>) -> DecisionResult {
        let rules = RulesDecisionEngine.decide(input);
        if !lfm::is_stalled(rules.reason_code) {
            return rules;
        }
        let started = Instant::now();
        let outcome = self.ask(input);
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match outcome {
            Ok((decision, reason)) => DecisionResult {
                decision,
                confidence: None,
                reason_code: rules.reason_code,
                provider: PROVIDER_NAME.to_owned(),
                latency_ms,
                model_reason: Some(reason),
            },
            Err(error) => DecisionResult {
                provider: format!("rules (fallback from {PROVIDER_NAME}: {error})"),
                latency_ms,
                ..rules
            },
        }
    }
}

/// The name of a loop signal, without its values.
pub(crate) fn signal_name(signal: &LoopSignal) -> &'static str {
    match signal {
        LoopSignal::RepeatedCommand { .. } => "repeated command",
        LoopSignal::SameFailure { .. } => "same failure",
        LoopSignal::RepeatedEdit { .. } => "repeated edit",
        LoopSignal::IterationLimit { .. } => "iteration limit",
        LoopSignal::ImplementationRetries { .. } => "implementation retries",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::providers::{ChatResponse, MockProvider, ProviderError};
    use harness_core::task::TaskState;

    fn stalled() -> Vec<LoopSignal> {
        vec![LoopSignal::SameFailure {
            fingerprint: "fp_v1:aaaa".to_owned(),
            count: 3,
        }]
    }

    fn answer(text: &str) -> Result<ChatResponse, ProviderError> {
        Ok(ChatResponse {
            model: "lfm".to_owned(),
            content: Some(text.to_owned()),
            ..ChatResponse::default()
        })
    }

    fn engine(responses: Vec<Result<ChatResponse, ProviderError>>) -> LfmEngine {
        LfmEngine::new(Box::new(MockProvider::scripted("lfm", responses)))
    }

    fn decide(engine: &LfmEngine, signals: &[LoopSignal]) -> DecisionResult {
        let state = TaskState::new("task-1", "repo", "p");
        engine.decide(&DecisionInput {
            state: &state,
            signals,
        })
    }

    #[test]
    fn a_valid_answer_to_a_stalled_loop_is_the_decision() {
        let result = decide(
            &engine(vec![answer(
                r#"{"decision": "REPLAN", "reason": "try another file"}"#,
            )]),
            &stalled(),
        );
        assert_eq!(result.decision, Decision::Replan);
        assert_eq!(result.provider, PROVIDER_NAME);
        assert_eq!(result.model_reason.as_deref(), Some("try another file"));
        assert_eq!(
            result.reason_code,
            harness_core::decision::ReasonCode::SameFailure
        );
    }

    #[test]
    fn an_invalid_answer_falls_back_to_the_rules() {
        let result = decide(&engine(vec![answer("I think you should stop")]), &stalled());
        assert_eq!(result.decision, Decision::Escalate);
        assert!(
            result.provider.starts_with("rules (fallback"),
            "{}",
            result.provider
        );
        assert_eq!(result.model_reason, None);
    }

    #[test]
    fn a_provider_error_falls_back_to_the_rules() {
        let result = decide(
            &engine(vec![Err(ProviderError::Unreachable(
                "connection refused".to_owned(),
            ))]),
            &stalled(),
        );
        assert_eq!(result.decision, Decision::Escalate);
        assert!(
            result.provider.contains("connection refused"),
            "{}",
            result.provider
        );
    }

    #[test]
    fn a_limit_is_never_sent_to_the_model() {
        let limit = [LoopSignal::IterationLimit {
            iteration: 8,
            limit: 8,
        }];
        let mock = MockProvider::scripted("lfm", vec![answer(r#"{"decision": "REPLAN"}"#)]);
        let engine = LfmEngine::new(Box::new(mock));
        let result = decide(&engine, &limit);
        assert_eq!(
            result.decision,
            Decision::TaskBlocked,
            "the rules decide a limit"
        );
        assert_eq!(result.provider, "rules");
    }

    #[test]
    fn the_request_holds_the_summary_and_no_prompt_text() {
        let mut state = TaskState::new("task-1", "repo", "SECRET OBJECTIVE TEXT");
        state.last_action = Some("bash: pytest".to_owned());
        let request = LfmEngine::request(&DecisionInput {
            state: &state,
            signals: &stalled(),
        });
        let text: String = request
            .messages
            .iter()
            .map(|m| format!("{:?}", m))
            .collect();
        assert!(text.contains("same failure"));
        assert!(
            !text.contains("SECRET OBJECTIVE TEXT"),
            "the prompt is not sent"
        );
        assert_eq!(request.enable_thinking, Some(false));
    }

    /// The rules' escalation for the stalled loop, as the primary engine would hand it to the deep slot.
    fn escalated_by_rules() -> DecisionResult {
        let state = TaskState::new("task-1", "repo", "p");
        RulesDecisionEngine.decide(&DecisionInput {
            state: &state,
            signals: &stalled(),
        })
    }

    fn escalate(engine: &LfmEngine) -> DecisionResult {
        let state = TaskState::new("task-1", "repo", "p");
        engine.escalate(
            &DecisionInput {
                state: &state,
                signals: &stalled(),
            },
            escalated_by_rules(),
        )
    }

    #[test]
    fn an_escalation_takes_the_deep_answer_and_keeps_its_reason_code() {
        let result = escalate(&engine(vec![answer(
            r#"{"decision": "REPLAN", "reason": "the fix is in another file"}"#,
        )]));
        assert_eq!(result.decision, Decision::Replan);
        assert!(
            result.provider.starts_with(ESCALATED_PREFIX),
            "{}",
            result.provider
        );
        assert_eq!(
            result.reason_code,
            harness_core::decision::ReasonCode::SameFailure
        );
        assert_eq!(
            result.model_reason.as_deref(),
            Some("the fix is in another file")
        );
    }

    #[test]
    fn a_failed_escalation_keeps_the_escalated_decision_and_says_why() {
        let result = escalate(&engine(vec![Err(ProviderError::Unreachable(
            "connection refused".to_owned(),
        ))]));
        assert_eq!(
            result.decision,
            Decision::Escalate,
            "the case still goes on"
        );
        assert!(
            result
                .provider
                .starts_with(&format!("{ESCALATED_PREFIX} failed")),
            "{}",
            result.provider
        );
        assert!(
            result.provider.contains("kept rules"),
            "{}",
            result.provider
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod deep_live_tests {
    use super::*;
    use crate::providers::provider_for;
    use harness_core::config::{Config, ModelRole};
    use harness_core::task::TaskState;

    /// A live check of escalation: the deep slot answers an escalated stalled loop with a decision from the set. The
    /// endpoint comes from `BRIDLE_DEEP_URL` (default: the `deep` slot of the shipped profile). Run with `--ignored`
    /// while the deep model is served, and the main model is stopped (one GPU model at a time).
    #[test]
    #[ignore = "needs the deep model served on its endpoint"]
    fn the_deep_slot_answers_an_escalated_loop() {
        let profile = include_str!("../../../configs/local-with-jev.toml");
        let mut config: Config = toml::from_str(profile).unwrap();
        let slot = config
            .models
            .values_mut()
            .find(|m| m.role == ModelRole::DeepReasoning)
            .expect("the profile has a deep_reasoning slot");
        if let Ok(url) = std::env::var("BRIDLE_DEEP_URL") {
            slot.endpoint = url;
        }
        slot.timeout_ms = slot.timeout_ms.min(ESCALATION_TIMEOUT_MS);
        let provider = provider_for(slot, &|name| std::env::var(name).ok())
            .expect("the deep slot builds a provider");
        let engine = LfmEngine::new(provider);

        let state = TaskState::new("task-live", "repo", "fix the failing test");
        let signals = [harness_core::loops::LoopSignal::SameFailure {
            fingerprint: "fp_v1:live".to_owned(),
            count: 3,
        }];
        let input = DecisionInput {
            state: &state,
            signals: &signals,
        };
        let primary = RulesDecisionEngine.decide(&input);
        assert_eq!(primary.decision, Decision::Escalate, "the rules escalate a same failure");

        let result = engine.escalate(&input, primary);
        // A failed escalation carries the same prefix, so the check is that the label has no failure in it.
        assert!(
            result.provider.starts_with(ESCALATED_PREFIX) && !result.provider.contains("failed"),
            "the deep answer was not used: {}",
            result.provider
        );
        assert!(result.model_reason.is_some(), "the deep model gave no reason");
        assert!(
            matches!(
                result.decision,
                Decision::Replan | Decision::Escalate | Decision::AskUser
            ),
            "{:?}",
            result.decision
        );
    }
}
