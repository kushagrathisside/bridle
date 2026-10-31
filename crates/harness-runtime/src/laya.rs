//! The Laya decision backend: the request for `POST /v1/systemone` and the check of its answer.
//!
//! Laya is a local, non-autoregressive decision model served by `laya-serve` (Apache 2.0; see ADR-0015). It answers
//! a typed question with one of the choices the question names, a probability for each choice, and a confidence. It
//! does not generate text, so an answer outside the named choices is a fault, not a suggestion.
//!
//! This module is the wire contract: the request, and the answer as the server returns it. The HTTP call is not here.
//! The request carries the loop facts the rules already use, and no prompt text, command text or tool output.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// The path of the decision route on `laya-serve`.
pub const PATH: &str = "/v1/systemone";
/// The name of the one question this client asks. The server returns its answer under this name.
pub const QUESTION: &str = "loop";

/// The answer to one choice question, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// The request for a stalled loop: the state facts, the signal names, and the choices with what each one means.
pub fn request(state: Value, signals: &[&str], criteria: &[(&str, &str)]) -> Value {
    let criteria: serde_json::Map<String, Value> = criteria
        .iter()
        .map(|(choice, meaning)| ((*choice).to_owned(), json!(meaning)))
        .collect();
    json!({
        "state": {
            "phase": state["phase"],
            "iteration": state["iteration"],
            "failure_count": state["failure_count"],
            "consecutive_refusals": state["consecutive_refusals"],
            "last_action": state["last_action"],
            "signals": signals,
        },
        "questions": {
            QUESTION: {
                "type": "choice",
                "instructions": "The agent's loop has stalled. What should happen next?",
                "criteria": criteria,
            }
        }
    })
}

/// Checks an answer: the named question must be present and of type `choice`, its choice must be one of `choices`,
/// and the probabilities must name exactly those choices, each within 0 to 1.
pub fn parse(text: &str, choices: &[&str]) -> Result<Answer, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("the answer is not JSON: {error}"))?;
    let loop_answer = value
        .get("answers")
        .and_then(|answers| answers.get(QUESTION))
        .ok_or_else(|| format!("the answer has no `{QUESTION}` question"))?;
    if loop_answer.get("type").and_then(Value::as_str) != Some("choice") {
        return Err("the answer is not of type choice".to_owned());
    }
    let choice = loop_answer
        .get("choice")
        .and_then(Value::as_str)
        .ok_or_else(|| "the answer has no choice".to_owned())?;
    if !choices.contains(&choice) {
        return Err(format!("the choice {choice:?} is not one of {choices:?}"));
    }
    let raw = loop_answer
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| "the answer has no probabilities".to_owned())?;
    let mut probabilities = BTreeMap::new();
    for (name, value) in raw {
        let p = value
            .as_f64()
            .filter(|p| (0.0..=1.0).contains(p))
            .ok_or_else(|| format!("the probability for {name:?} is not between 0 and 1"))?;
        probabilities.insert(name.clone(), p);
    }
    let named: Vec<&str> = probabilities.keys().map(String::as_str).collect();
    let mut expected: Vec<&str> = choices.to_vec();
    expected.sort_unstable();
    if named != expected {
        return Err(format!(
            "the probabilities name {named:?}, not {expected:?}"
        ));
    }
    let confidence = loop_answer
        .get("confidence")
        .and_then(Value::as_f64)
        .filter(|c| (0.0..=1.0).contains(c))
        .ok_or_else(|| "the answer has no confidence between 0 and 1".to_owned())?;
    Ok(Answer {
        choice: choice.to_owned(),
        probabilities,
        confidence,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The answer `laya-serve` returned for a same-failure state, with the typed-decisions checkpoint (2026-10-31).
    const RECORDED: &str = r#"{"model":"laya-rl-agent","answers":{"loop":{"type":"choice","choice":"ESCALATE",
        "probabilities":{"ESCALATE":0.564,"ASK_USER":0.2359,"REPLAN":0.2001},"confidence":0.7,
        "answer_confidence":0.564,"action":{"act_probability":1.0}}},"routing":{"model":"typed-decisions"}}"#;

    const CHOICES: [&str; 3] = ["ESCALATE", "ASK_USER", "REPLAN"];

    #[test]
    fn the_recorded_answer_is_read() {
        let answer = parse(RECORDED, &CHOICES).unwrap();
        assert_eq!(answer.choice, "ESCALATE");
        assert_eq!(answer.probabilities.len(), 3);
        assert!((answer.probabilities["ESCALATE"] - 0.564).abs() < 1e-9);
        assert!((answer.confidence - 0.7).abs() < 1e-9);
    }

    #[test]
    fn a_choice_outside_the_named_choices_is_rejected() {
        let text = RECORDED.replace("\"choice\":\"ESCALATE\"", "\"choice\":\"PLAN\"");
        assert!(parse(&text, &CHOICES).unwrap_err().contains("PLAN"));
    }

    #[test]
    fn probabilities_must_name_exactly_the_choices() {
        let text = RECORDED.replace("\"REPLAN\":0.2001", "\"PLAN\":0.2001");
        assert!(
            parse(&text, &CHOICES)
                .unwrap_err()
                .contains("probabilities name")
        );
    }

    #[test]
    fn a_probability_outside_zero_to_one_is_rejected() {
        let text = RECORDED.replace("\"ESCALATE\":0.564", "\"ESCALATE\":1.5");
        assert!(
            parse(&text, &CHOICES)
                .unwrap_err()
                .contains("between 0 and 1")
        );
    }

    #[test]
    fn a_missing_question_or_a_non_choice_answer_is_rejected() {
        assert!(
            parse("{\"answers\":{}}", &CHOICES)
                .unwrap_err()
                .contains("no `loop` question")
        );
        let text = RECORDED.replace("\"type\":\"choice\"", "\"type\":\"score\"");
        assert!(
            parse(&text, &CHOICES)
                .unwrap_err()
                .contains("not of type choice")
        );
    }

    #[test]
    fn the_request_names_the_question_the_choices_and_the_state_only() {
        let state = json!({"phase": "DIAGNOSE", "iteration": 2, "failure_count": 3,
            "consecutive_refusals": 0, "last_action": "run_tests", "prompt": "secret text"});
        let body = request(
            state,
            &["same_failure"],
            &[
                ("ESCALATE", "a stronger model should look"),
                ("ASK_USER", "the person decides"),
            ],
        );
        let question = &body["questions"][QUESTION];
        assert_eq!(question["type"], "choice");
        assert_eq!(
            question["criteria"]["ESCALATE"],
            "a stronger model should look"
        );
        assert_eq!(body["state"]["signals"][0], "same_failure");
        assert!(body["state"].get("prompt").is_none(), "{body}");
    }
}
