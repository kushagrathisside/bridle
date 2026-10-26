//! The Jev decision backend (M7, R-9.1, R-9.5): the strict check of an answer from the Jev service.
//!
//! Jev is an optional, external decision model. Like the micro model, it decides stalled loops only (ADR-0007).
//! It answers with a decision and the probability it gives that decision. The host checks every field: an answer
//! outside the contract is rejected, and the caller falls back to the rules. The network code is in
//! `harness-runtime`.

use serde_json::Value;

use crate::decision::Decision;
use crate::lfm;

/// The path of the decision call, relative to the configured endpoint.
pub const DECISION_PATH: &str = "/v1/decisions";
/// The contract name sent with every request, so that a service can refuse a version it does not know.
pub const SCHEMA: &str = "bridle.loop_decision.v1";
/// The longest reason kept from an answer.
pub const MAX_REASON: usize = 200;
/// The longest decision name echoed back in a rejection.
const MAX_ECHO: usize = 40;

/// A checked answer from Jev.
#[derive(Debug, Clone, PartialEq)]
pub struct JevChoice {
    pub decision: Decision,
    /// The probability Jev gives this decision, from 0 to 1 (R-9.5: Jev's output is kept, not rounded).
    pub confidence: f32,
    /// A short reason, cleaned and capped. Optional.
    pub reason: Option<String>,
}

/// Why an answer was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevReject {
    NotJson,
    NotAnObject,
    MissingDecision,
    UnknownDecision(String),
    MissingConfidence,
    ConfidenceOutOfRange,
}

impl std::fmt::Display for JevReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotJson => write!(f, "the answer is not JSON"),
            Self::NotAnObject => write!(f, "the answer is not a JSON object"),
            Self::MissingDecision => write!(f, "the answer has no decision"),
            Self::UnknownDecision(name) => {
                write!(f, "the decision `{name}` is not one of the choices")
            }
            Self::MissingConfidence => write!(f, "the answer has no confidence"),
            Self::ConfidenceOutOfRange => write!(f, "the confidence is outside 0 to 1"),
        }
    }
}

/// Parses and checks one answer. A fenced block is accepted; anything else around the object is not.
pub fn parse_decision(text: &str) -> Result<JevChoice, JevReject> {
    let body = lfm::strip_fence(text.trim());
    let value: Value = serde_json::from_str(body).map_err(|_| JevReject::NotJson)?;
    let object = value.as_object().ok_or(JevReject::NotAnObject)?;
    let name = object
        .get("decision")
        .and_then(Value::as_str)
        .ok_or(JevReject::MissingDecision)?;
    let decision = lfm::decision_for_choice(name)
        .ok_or_else(|| JevReject::UnknownDecision(clean(name, MAX_ECHO)))?;
    let confidence = object
        .get("confidence")
        .and_then(Value::as_f64)
        .ok_or(JevReject::MissingConfidence)?;
    if !(0.0..=1.0).contains(&confidence) {
        return Err(JevReject::ConfidenceOutOfRange);
    }
    let reason = object
        .get("reason")
        .and_then(Value::as_str)
        .map(|text| clean(text, MAX_REASON))
        .filter(|text| !text.is_empty());
    Ok(JevChoice {
        decision,
        confidence: confidence as f32,
        reason,
    })
}

/// Control characters become spaces, and the text is cut to `max` characters.
fn clean(text: &str, max: usize) -> String {
    text.chars()
        .take(max)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_checked_answer_keeps_its_probability_and_reason() {
        let choice = parse_decision(
            r#"{"decision":"ESCALATE","confidence":0.86,"reason":"the same test failed three times"}"#,
        )
        .unwrap();
        assert_eq!(choice.decision, Decision::Escalate);
        assert!((choice.confidence - 0.86).abs() < 1e-6);
        assert_eq!(
            choice.reason.as_deref(),
            Some("the same test failed three times")
        );
    }

    #[test]
    fn a_fenced_answer_is_accepted_and_the_reason_is_optional() {
        let choice =
            parse_decision("```json\n{\"decision\":\"REPLAN\",\"confidence\":0.5}\n```").unwrap();
        assert_eq!(choice.decision, Decision::Replan);
        assert_eq!(choice.reason, None);
    }

    #[test]
    fn answers_outside_the_contract_are_rejected() {
        assert_eq!(parse_decision("not json"), Err(JevReject::NotJson));
        assert_eq!(parse_decision("[1]"), Err(JevReject::NotAnObject));
        assert_eq!(
            parse_decision(r#"{"confidence":0.5}"#),
            Err(JevReject::MissingDecision)
        );
        assert_eq!(
            parse_decision(r#"{"decision":"CONTINUE","confidence":0.5}"#),
            Err(JevReject::UnknownDecision("CONTINUE".to_owned()))
        );
        assert_eq!(
            parse_decision(r#"{"decision":"REPLAN"}"#),
            Err(JevReject::MissingConfidence)
        );
        assert_eq!(
            parse_decision(r#"{"decision":"REPLAN","confidence":1.5}"#),
            Err(JevReject::ConfidenceOutOfRange)
        );
        assert_eq!(
            parse_decision(r#"{"decision":"REPLAN","confidence":-0.1}"#),
            Err(JevReject::ConfidenceOutOfRange)
        );
    }

    #[test]
    fn a_long_or_control_filled_reason_is_cleaned_and_capped() {
        let raw = format!(
            "{{\"decision\":\"REPLAN\",\"confidence\":0.5,\"reason\":\"a\\nb{}\"}}",
            "x".repeat(400)
        );
        let reason = parse_decision(&raw).unwrap().reason.unwrap();
        assert!(reason.starts_with("a b"), "{reason}");
        assert_eq!(reason.chars().count(), MAX_REASON);
    }

    #[test]
    fn a_rejected_decision_name_is_capped_in_the_message() {
        let raw = format!(r#"{{"decision":"{}","confidence":0.5}}"#, "Z".repeat(300));
        let Err(JevReject::UnknownDecision(name)) = parse_decision(&raw) else {
            panic!("expected an unknown decision");
        };
        assert_eq!(name.chars().count(), MAX_ECHO);
    }
}
