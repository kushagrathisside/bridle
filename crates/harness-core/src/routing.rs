//! Routing between model slots: which slot answers a request.
//!
//! The rules place a request first. A request they cannot place goes to the decision model, which answers one of
//! two choices, `MAIN` or `DEEP`. Its answer is checked against that closed set: anything else is rejected and the
//! request goes to `MAIN`. A slot that is not serving is never chosen, so `DEEP` is used only when it is running.
//!
//! This module is the pure part: the rules, the answer check and the final choice. The model call is in
//! `harness-runtime::routing`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The slot names the router may choose. The shipped profiles name them `main` and `deep`.
pub const MAIN: &str = "main";
pub const DEEP: &str = "deep";
/// The answers the decision model may give, in the order the prompt lists them.
pub const CHOICES: [&str; 2] = ["MAIN", "DEEP"];
/// The longest reason kept from the model.
pub const MAX_REASON: usize = 200;
/// A request shorter than this, with no hard word in it, goes to `MAIN` by rule.
pub const SHORT_REQUEST: usize = 80;

/// Words that mark a request as possibly hard. Such a request is not placed by the rules; the decision model
/// answers it. The list is deliberately short: a long request with none of these is placed by its length alone.
const HARD_WORDS: [&str; 9] = [
    "architecture",
    "design",
    "root cause",
    "across the codebase",
    "migrate",
    "investigate",
    "why does",
    "refactor",
    "security",
];

/// A slot the router may choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    Main,
    Deep,
}

impl Slot {
    /// The slot name in the configuration.
    pub fn name(self) -> &'static str {
        match self {
            Slot::Main => MAIN,
            Slot::Deep => DEEP,
        }
    }
}

/// What the rules said about a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placed {
    /// The rules chose a slot, with the reason to show.
    Rule { slot: Slot, reason: String },
    /// The rules cannot place the request; the decision model answers.
    Undecided,
}

/// Places a request by rule. An explicit `/deep` or `/main` at the start wins. An empty or short request with no hard
/// word goes to `MAIN`. Everything else is undecided.
pub fn place(text: &str) -> Placed {
    let trimmed = text.trim();
    if starts_with_word(trimmed, "/deep") {
        return Placed::Rule {
            slot: Slot::Deep,
            reason: "requested with /deep".to_owned(),
        };
    }
    if starts_with_word(trimmed, "/main") {
        return Placed::Rule {
            slot: Slot::Main,
            reason: "requested with /main".to_owned(),
        };
    }
    let lower = trimmed.to_lowercase();
    if HARD_WORDS.iter().any(|word| lower.contains(word)) {
        return Placed::Undecided;
    }
    if trimmed.chars().count() < SHORT_REQUEST {
        return Placed::Rule {
            slot: Slot::Main,
            reason: "a short request with no hard word".to_owned(),
        };
    }
    Placed::Undecided
}

/// Whether `text` starts with `word` as a whole word: the word ends the text or is followed by whitespace.
fn starts_with_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// A checked answer of the decision model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub slot: Slot,
    pub reason: String,
}

/// Why an answer was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    NotJson,
    NotAnObject,
    MissingChoice,
    UnknownChoice(String),
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RejectReason::NotJson => f.write_str("the answer is not JSON"),
            RejectReason::NotAnObject => f.write_str("the answer is not a JSON object"),
            RejectReason::MissingChoice => f.write_str("the answer has no choice"),
            RejectReason::UnknownChoice(value) => {
                write!(f, "the choice {value:?} is not one of {CHOICES:?}")
            }
        }
    }
}

/// Parses and checks an answer: one JSON object whose `choice` is in [`CHOICES`]. A Markdown code fence around it is
/// allowed, and nothing else is.
pub fn parse_answer(text: &str) -> Result<Answer, RejectReason> {
    let body = strip_fence(text.trim());
    let value: Value = serde_json::from_str(body).map_err(|_| RejectReason::NotJson)?;
    let object = value.as_object().ok_or(RejectReason::NotAnObject)?;
    let choice = object
        .get("choice")
        .and_then(Value::as_str)
        .ok_or(RejectReason::MissingChoice)?;
    let slot = match choice {
        "MAIN" => Slot::Main,
        "DEEP" => Slot::Deep,
        other => return Err(RejectReason::UnknownChoice(other.to_owned())),
    };
    let reason: String = object
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_REASON)
        .collect();
    Ok(Answer { slot, reason })
}

/// The slot a request goes to. A `DEEP` answer is used only if the deep slot is serving; otherwise the request goes to
/// `MAIN`, and the reason says so.
pub fn choose(
    placed: Placed,
    answer: Option<Result<Answer, String>>,
    deep_serving: bool,
) -> (Slot, String) {
    let (slot, reason) = match placed {
        Placed::Rule { slot, reason } => (slot, reason),
        Placed::Undecided => match answer {
            Some(Ok(answer)) => (answer.slot, answer.reason),
            Some(Err(error)) => (
                Slot::Main,
                format!("the decision answer was not used: {error}"),
            ),
            None => (Slot::Main, "no decision answer was asked".to_owned()),
        },
    };
    if slot == Slot::Deep && !deep_serving {
        return (
            Slot::Main,
            format!("{reason}; the deep slot is not serving"),
        );
    }
    (slot, reason)
}

fn strip_fence(text: &str) -> &str {
    let text = text.trim();
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.trim().strip_suffix("```").unwrap_or(rest).trim()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_slot_at_the_start_wins() {
        assert_eq!(
            place("/deep design the cache"),
            Placed::Rule {
                slot: Slot::Deep,
                reason: "requested with /deep".to_owned()
            }
        );
        assert_eq!(
            place("/main refactor this"),
            Placed::Rule {
                slot: Slot::Main,
                reason: "requested with /main".to_owned()
            }
        );
        // A word that only starts like the prefix is not the prefix: this one is a short request, so it goes to main by
        // length, not by the explicit choice.
        assert_eq!(
            place("/deepen the test"),
            Placed::Rule {
                slot: Slot::Main,
                reason: "a short request with no hard word".to_owned()
            }
        );
    }

    #[test]
    fn a_short_request_with_no_hard_word_goes_to_main() {
        assert!(matches!(
            place("fix the typo in README"),
            Placed::Rule {
                slot: Slot::Main,
                ..
            }
        ));
    }

    #[test]
    fn a_hard_word_leaves_the_request_to_the_decision_model() {
        assert_eq!(
            place("Redesign the storage architecture"),
            Placed::Undecided
        );
        assert_eq!(
            place("Why does the parser panic on empty input?"),
            Placed::Undecided
        );
    }

    #[test]
    fn a_long_request_without_a_hard_word_is_undecided() {
        let long = "Add a unit test for the parser that covers an empty file, a file with only comments, \
                    and a file with a trailing newline, and keep the existing tests unchanged.";
        assert_eq!(place(long), Placed::Undecided);
    }

    #[test]
    fn a_valid_answer_is_parsed_and_a_fence_is_allowed() {
        let answer = parse_answer(
            "```json\n{\"choice\": \"DEEP\", \"reason\": \"needs a wider view\"}\n```",
        )
        .unwrap();
        assert_eq!(
            answer,
            Answer {
                slot: Slot::Deep,
                reason: "needs a wider view".to_owned()
            }
        );
        assert_eq!(
            parse_answer("{\"choice\": \"MAIN\"}").unwrap().slot,
            Slot::Main
        );
    }

    #[test]
    fn an_answer_outside_the_closed_set_is_rejected() {
        assert_eq!(parse_answer("not json"), Err(RejectReason::NotJson));
        assert_eq!(parse_answer("[1]"), Err(RejectReason::NotAnObject));
        assert_eq!(
            parse_answer("{\"reason\": \"x\"}"),
            Err(RejectReason::MissingChoice)
        );
        assert_eq!(
            parse_answer("{\"choice\": \"PLAN\"}"),
            Err(RejectReason::UnknownChoice("PLAN".to_owned()))
        );
        assert_eq!(
            parse_answer("{\"choice\": \"deep\"}"),
            Err(RejectReason::UnknownChoice("deep".to_owned()))
        );
    }

    #[test]
    fn a_rejected_answer_goes_to_main_with_the_reason() {
        let placed = Placed::Undecided;
        let rejected = parse_answer("{\"choice\": \"PLAN\"}").map_err(|reject| reject.to_string());
        let (slot, reason) = choose(placed, Some(rejected), true);
        assert_eq!(slot, Slot::Main);
        assert!(
            reason.contains("not used") && reason.contains("PLAN"),
            "{reason}"
        );
    }

    #[test]
    fn deep_is_used_only_when_it_is_serving() {
        let deep = Placed::Rule {
            slot: Slot::Deep,
            reason: "requested with /deep".to_owned(),
        };
        let (slot, reason) = choose(deep.clone(), None, true);
        assert_eq!(slot, Slot::Deep);
        assert_eq!(reason, "requested with /deep");
        let (slot, reason) = choose(deep, None, false);
        assert_eq!(slot, Slot::Main);
        assert!(reason.ends_with("the deep slot is not serving"), "{reason}");
    }

    #[test]
    fn an_undecided_request_with_no_answer_goes_to_main() {
        let (slot, _) = choose(Placed::Undecided, None, true);
        assert_eq!(slot, Slot::Main);
    }
}
