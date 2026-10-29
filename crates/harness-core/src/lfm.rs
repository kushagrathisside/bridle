//! The contract of the LFM decision engine: which answers a stalled loop may take, and how a model
//! answer is checked.
//!
//! The model is consulted only when a loop is stalled: the same failure, a repeated command, or a repeated
//! edit. It may answer ESCALATE, ASK_USER or REPLAN. It never sets a limit, never allows or denies a call,
//! and never answers for the iteration or retry limits, which stay deterministic. Anything else it returns
//! is rejected, and the rules decide instead.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decision::{Decision, ReasonCode};

/// The answers a stalled loop may take from the model.
pub const CHOICES: [&str; 3] = ["ESCALATE", "ASK_USER", "REPLAN"];
/// The longest reason kept from the model.
pub const MAX_REASON: usize = 200;

/// Whether a loop reason is a stalled loop, the only kind the model may decide.
pub fn is_stalled(reason: ReasonCode) -> bool {
    matches!(
        reason,
        ReasonCode::SameFailure | ReasonCode::RepeatedCommand | ReasonCode::RepeatedEdit
    )
}

/// A checked answer of the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LfmChoice {
    pub decision: Decision,
    pub reason: String,
}

/// Why an answer was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    NotJson,
    NotAnObject,
    MissingDecision,
    UnknownDecision(String),
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RejectReason::NotJson => f.write_str("the answer is not JSON"),
            RejectReason::NotAnObject => f.write_str("the answer is not a JSON object"),
            RejectReason::MissingDecision => f.write_str("the answer has no decision"),
            RejectReason::UnknownDecision(value) => {
                write!(f, "the decision {value:?} is not one of {CHOICES:?}")
            }
        }
    }
}

/// Parses and checks an answer. The answer must be one JSON object with a `decision` from [`CHOICES`]; a
/// Markdown code fence around it is allowed, and nothing else is.
pub fn parse_choice(text: &str) -> Result<LfmChoice, RejectReason> {
    let body = strip_fence(text.trim());
    let value: Value = serde_json::from_str(body).map_err(|_| RejectReason::NotJson)?;
    let object = value.as_object().ok_or(RejectReason::NotAnObject)?;
    let name = object
        .get("decision")
        .and_then(Value::as_str)
        .ok_or(RejectReason::MissingDecision)?;
    let decision =
        decision_for_choice(name).ok_or_else(|| RejectReason::UnknownDecision(name.to_owned()))?;
    let reason: String = object
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_REASON)
        .collect();
    Ok(LfmChoice { decision, reason })
}

pub(crate) fn strip_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.trim().strip_suffix("```").map_or(text, str::trim)
}

/// The decision a choice name stands for, or `None` for a name that is not in `CHOICES`.
pub fn decision_for_choice(name: &str) -> Option<Decision> {
    match name {
        "ESCALATE" => Some(Decision::Escalate),
        "ASK_USER" => Some(Decision::AskUser),
        "REPLAN" => Some(Decision::Replan),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_in_the_set_is_accepted() {
        let choice =
            parse_choice(r#"{"decision": "REPLAN", "reason": "the fix is in another file"}"#)
                .unwrap();
        assert_eq!(choice.decision, Decision::Replan);
        assert_eq!(choice.reason, "the fix is in another file");
    }

    #[test]
    fn a_code_fence_around_the_object_is_allowed() {
        let text = "```json\n{\"decision\": \"ASK_USER\"}\n```";
        assert_eq!(parse_choice(text).unwrap().decision, Decision::AskUser);
    }

    #[test]
    fn text_around_the_object_is_rejected() {
        assert_eq!(
            parse_choice("Sure. {\"decision\": \"ESCALATE\"}"),
            Err(RejectReason::NotJson)
        );
    }

    #[test]
    fn a_decision_outside_the_set_is_rejected_even_if_it_looks_like_a_limit() {
        for bad in ["TASK_BLOCKED", "NEXT_ACTION", "ALLOW", "retry"] {
            let text = format!(r#"{{"decision": "{bad}"}}"#);
            assert!(
                matches!(parse_choice(&text), Err(RejectReason::UnknownDecision(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_missing_decision_or_a_non_object_is_rejected() {
        assert_eq!(
            parse_choice(r#"{"reason": "x"}"#),
            Err(RejectReason::MissingDecision)
        );
        assert_eq!(
            parse_choice(r#"["ESCALATE"]"#),
            Err(RejectReason::NotAnObject)
        );
        assert_eq!(parse_choice(""), Err(RejectReason::NotJson));
    }

    #[test]
    fn the_reason_is_capped_and_stripped_of_control_characters() {
        let long = "a".repeat(500);
        let text = format!("{{\"decision\": \"ESCALATE\", \"reason\": \"{long}\\u0007\"}}");
        let choice = parse_choice(&text).unwrap();
        assert_eq!(choice.reason.chars().count(), MAX_REASON);
    }

    #[test]
    fn only_stalled_loops_reach_the_model() {
        assert!(is_stalled(ReasonCode::SameFailure));
        assert!(is_stalled(ReasonCode::RepeatedCommand));
        assert!(is_stalled(ReasonCode::RepeatedEdit));
        assert!(!is_stalled(ReasonCode::IterationLimit));
        assert!(!is_stalled(ReasonCode::ImplementationRetries));
        assert!(!is_stalled(ReasonCode::PhaseDefault));
    }
}
