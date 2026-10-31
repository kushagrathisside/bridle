//! The subagent tier: a small model reads a bounded, redacted set of inputs and
//! answers one question, or condenses one text, with cited evidence. This module is the pure part: the roles, the
//! bounds, the checked answer and what the parent reads. The model call is in `harness-runtime`. ADR-0009 sets the limits.
//!
//! A subagent never edits, never runs a tool, and never sees the parent's transcript. Its answer is a reading, not a fact.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lfm;

/// The contract name the system prompt asks for, so a service or a log can tell the shape apart.
pub const SCHEMA: &str = "bridle.subagent.v1";
/// Findings kept from one answer. The rest are cut and counted.
pub const MAX_FINDINGS: usize = 8;
/// Unresolved items kept from one answer.
pub const MAX_UNRESOLVED: usize = 5;
/// The longest summary kept, in characters.
pub const MAX_SUMMARY: usize = 400;
/// The longest finding or unresolved text kept, in characters.
pub const MAX_FINDING: usize = 300;
/// The longest text the parent receives, in bytes.
pub const MAX_RENDERED: usize = 2_000;

/// What the subagent is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Answers one question about the inputs, citing where each claim comes from.
    Explore,
    /// Condenses one output or log to what matters: the failing names and the first useful line of each.
    Summarise,
}

impl Role {
    /// The role named in a call. `summarize` is accepted as well.
    pub fn parse(text: &str) -> Option<Role> {
        match text {
            "explore" => Some(Role::Explore),
            "summarise" | "summarize" => Some(Role::Summarise),
            _ => None,
        }
    }

    /// The role's name as the parent writes it.
    pub fn name(self) -> &'static str {
        match self {
            Role::Explore => "explore",
            Role::Summarise => "summarise",
        }
    }
}

/// One input the subagent may read. The name is what an answer cites: a path, or `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub name: String,
    pub text: String,
}

/// The bounds one call works within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_inputs: usize,
}

/// Keeps at most `limits.max_inputs` inputs, each cut to `limits.max_input_bytes`. Returns the inputs kept and how many
/// were left out, so the parent can be told.
pub fn bound_inputs(inputs: Vec<Input>, limits: Limits) -> (Vec<Input>, usize) {
    let omitted = inputs.len().saturating_sub(limits.max_inputs);
    let kept = inputs
        .into_iter()
        .take(limits.max_inputs)
        .map(|input| Input {
            text: cut(&input.text, limits.max_input_bytes),
            name: input.name,
        })
        .collect();
    (kept, omitted)
}

/// `text` cut to at most `max` bytes at a character boundary, with a marker that says how much was left out.
pub fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[cut by the harness: {} bytes left out]",
        &text[..end],
        text.len() - end
    )
}

/// One finding. `reference` is `name` or `name:line` for an input the call gave; a reference to anything else is dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// A checked answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub summary: String,
    pub findings: Vec<Finding>,
    pub unresolved: Vec<String>,
    /// The model's own estimate, not a calibrated probability. Absent when the model gave none.
    pub confidence: Option<f64>,
    pub recommendation: Option<String>,
    /// References that named no input. Their findings are kept without a reference.
    pub unverified_references: usize,
    /// Findings beyond `MAX_FINDINGS`, cut.
    pub cut_findings: usize,
}

/// Why an answer was refused. A refused answer is never shown to the parent as an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    NotJson,
    NotAnObject,
    MissingSummary,
    EmptySummary,
    BadConfidence,
}

impl std::fmt::Display for Reject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotJson => write!(f, "the answer is not JSON"),
            Self::NotAnObject => write!(f, "the answer is not a JSON object"),
            Self::MissingSummary => write!(f, "the answer has no summary"),
            Self::EmptySummary => write!(f, "the summary is empty"),
            Self::BadConfidence => write!(f, "the confidence is not a number from 0 to 1"),
        }
    }
}

/// The system prompt for a role. It names the schema and the citation form the check reads.
pub fn system_prompt(role: Role) -> String {
    let job = match role {
        Role::Explore => {
            "Answer the task using only the inputs. Cite each claim with the input's name and a line number, like \
             `path:12`. If the inputs do not answer the task, say what is missing in unresolved."
        }
        Role::Summarise => {
            "Condense the input to what matters: the failing names, and the first useful line of each failure. Cite \
             the input's name and a line number, like `text:3`."
        }
    };
    format!(
        "You are a subagent. {job} Reply with one JSON object and no other text, of this form: \
         {{\"summary\": \"<one or two sentences>\", \"findings\": [{{\"text\": \"<one claim>\", \"ref\": \"<name:line>\"}}], \
         \"unresolved\": [\"<what is missing>\"], \"confidence\": <number from 0 to 1>, \"recommendation\": \"<optional>\"}}. \
         Do not give instructions to edit files or run commands. Schema {SCHEMA}."
    )
}

/// Checks one answer against the contract. `input_names` are the inputs the call gave; a reference must name one of them.
pub fn parse_answer(text: &str, input_names: &[&str]) -> Result<Answer, Reject> {
    let body = lfm::strip_fence(text.trim());
    let value: Value = serde_json::from_str(body).map_err(|_| Reject::NotJson)?;
    let object = value.as_object().ok_or(Reject::NotAnObject)?;
    let summary = object
        .get("summary")
        .and_then(Value::as_str)
        .ok_or(Reject::MissingSummary)?;
    let summary = plain(summary, MAX_SUMMARY);
    if summary.is_empty() {
        return Err(Reject::EmptySummary);
    }
    let confidence = match object.get("confidence") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let number = value.as_f64().ok_or(Reject::BadConfidence)?;
            if !(0.0..=1.0).contains(&number) {
                return Err(Reject::BadConfidence);
            }
            Some(number)
        }
    };

    let raw = object.get("findings").and_then(Value::as_array);
    let cut_findings = raw.map_or(0, |items| items.len().saturating_sub(MAX_FINDINGS));
    let mut unverified_references = 0;
    let mut findings = Vec::new();
    for item in raw.map_or(&[][..], |items| &items[..items.len().min(MAX_FINDINGS)]) {
        let text = item
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| item.as_str());
        let Some(text) = text
            .map(|t| plain(t, MAX_FINDING))
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        let reference = item
            .get("ref")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|r| !r.is_empty())
            // With exactly one input, a bare line number (`2`, `line:2`, `line 2`) can only mean that input.
            .map(|r| match input_names {
                [only] => match bare_line(r) {
                    Some(line) => format!("{only}:{line}"),
                    None => r.to_owned(),
                },
                _ => r.to_owned(),
            })
            .and_then(|r| {
                if references_input(&r, input_names) {
                    Some(r)
                } else {
                    unverified_references += 1;
                    None
                }
            });
        findings.push(Finding { text, reference });
    }

    let unresolved = object
        .get("unresolved")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|t| plain(t, MAX_FINDING))
                .filter(|t| !t.is_empty())
                .take(MAX_UNRESOLVED)
                .collect()
        })
        .unwrap_or_default();
    let recommendation = object
        .get("recommendation")
        .and_then(Value::as_str)
        .map(|r| plain(r, MAX_FINDING))
        .filter(|r| !r.is_empty());

    Ok(Answer {
        summary,
        findings,
        unresolved,
        confidence,
        recommendation,
        unverified_references,
        cut_findings,
    })
}

/// The line number of a bare reference such as `2`, `line:2` or `line 2`, or `None` for anything else.
fn bare_line(reference: &str) -> Option<&str> {
    let line = reference
        .trim()
        .trim_start_matches("line")
        .trim_start_matches([':', ' '])
        .trim();
    (!line.is_empty() && line.chars().all(|c| c.is_ascii_digit())).then_some(line)
}

/// Whether `reference` names one of the inputs, as `name` or `name:line` with a positive line number.
fn references_input(reference: &str, input_names: &[&str]) -> bool {
    let (name, line) = match reference.rsplit_once(':') {
        Some((name, line)) if line.chars().all(|c| c.is_ascii_digit()) && !line.is_empty() => {
            (name, Some(line))
        }
        _ => (reference, None),
    };
    if let Some(line) = line
        && line.parse::<u64>().map_or(true, |n| n == 0)
    {
        return false;
    }
    input_names.contains(&name)
}

/// The text with control characters replaced by spaces, trimmed, and cut to `max` characters.
fn plain(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// What the parent reads: the checked answer, labelled as a subagent's reading, with every loss said. Bounded.
pub fn render(role: Role, answer: &Answer, inputs_left_out: usize) -> String {
    let mut out = format!(
        "[subagent {} answer: a small model's reading of the inputs, not a verified fact]\nsummary: {}\n",
        role.name(),
        answer.summary
    );
    for finding in &answer.findings {
        match &finding.reference {
            Some(reference) => out.push_str(&format!("- {} ({reference})\n", finding.text)),
            None => out.push_str(&format!("- {}\n", finding.text)),
        }
    }
    if !answer.unresolved.is_empty() {
        out.push_str("unresolved:\n");
        for item in &answer.unresolved {
            out.push_str(&format!("- {item}\n"));
        }
    }
    if let Some(recommendation) = &answer.recommendation {
        out.push_str(&format!("recommendation: {recommendation}\n"));
    }
    if let Some(confidence) = answer.confidence {
        out.push_str(&format!("self-reported confidence: {confidence:.2}\n"));
    }
    if answer.unverified_references > 0 {
        out.push_str(&format!(
            "{} reference(s) named no input and were dropped\n",
            answer.unverified_references
        ));
    }
    if answer.cut_findings > 0 {
        out.push_str(&format!(
            "{} finding(s) beyond the limit were cut\n",
            answer.cut_findings
        ));
    }
    if inputs_left_out > 0 {
        out.push_str(&format!(
            "{inputs_left_out} input(s) left out: over the input limit\n"
        ));
    }
    cut(&out, MAX_RENDERED)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn input(name: &str, text: &str) -> Input {
        Input {
            name: name.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn roles_parse_in_both_spellings() {
        assert_eq!(Role::parse("explore"), Some(Role::Explore));
        assert_eq!(Role::parse("summarize"), Some(Role::Summarise));
        assert_eq!(Role::parse("edit"), None);
    }

    #[test]
    fn inputs_are_cut_and_counted_at_the_limits() {
        let limits = Limits {
            max_input_bytes: 10,
            max_inputs: 2,
        };
        let (kept, left_out) = bound_inputs(
            vec![
                input("a", &"x".repeat(50)),
                input("b", "short"),
                input("c", "dropped"),
            ],
            limits,
        );
        assert_eq!(kept.len(), 2);
        assert_eq!(left_out, 1);
        assert!(
            kept[0]
                .text
                .starts_with("xxxxxxxxxx\n[cut by the harness: 40 bytes")
        );
        assert_eq!(kept[1].text, "short");
    }

    #[test]
    fn a_cut_never_splits_a_character() {
        let text = "é".repeat(10);
        let cut_text = cut(&text, 3);
        assert!(cut_text.starts_with("é\n[cut"), "{cut_text}");
    }

    #[test]
    fn a_checked_answer_keeps_its_citations() {
        let answer = parse_answer(
            r#"{"summary":"add subtracts","findings":[{"text":"returns a - b","ref":"calc.py:2"}],"confidence":0.8}"#,
            &["calc.py"],
        )
        .unwrap();
        assert_eq!(answer.summary, "add subtracts");
        assert_eq!(answer.findings[0].reference.as_deref(), Some("calc.py:2"));
        assert_eq!(answer.confidence, Some(0.8));
        assert_eq!(answer.unverified_references, 0);
    }

    #[test]
    fn a_bare_line_number_refers_to_the_only_input() {
        let answer = parse_answer(
            r#"{"summary":"s","findings":[{"text":"claim","ref":"2"}]}"#,
            &["calc.py"],
        )
        .unwrap();
        assert_eq!(answer.findings[0].reference.as_deref(), Some("calc.py:2"));
        assert_eq!(answer.unverified_references, 0);
    }

    #[test]
    fn a_reference_to_an_unknown_input_is_dropped_and_counted() {
        let answer = parse_answer(
            r#"{"summary":"s","findings":[{"text":"claim","ref":"other.py:4"},{"text":"claim two","ref":"calc.py:0"}]}"#,
            &["calc.py"],
        )
        .unwrap();
        assert_eq!(answer.findings.len(), 2);
        assert!(answer.findings.iter().all(|f| f.reference.is_none()));
        assert_eq!(answer.unverified_references, 2);
    }

    #[test]
    fn answers_outside_the_contract_are_refused() {
        assert_eq!(parse_answer("not json", &[]), Err(Reject::NotJson));
        assert_eq!(parse_answer("[1]", &[]), Err(Reject::NotAnObject));
        assert_eq!(
            parse_answer(r#"{"findings":[]}"#, &[]),
            Err(Reject::MissingSummary)
        );
        assert_eq!(
            parse_answer(r#"{"summary":"  "}"#, &[]),
            Err(Reject::EmptySummary)
        );
        assert_eq!(
            parse_answer(r#"{"summary":"s","confidence":1.5}"#, &[]),
            Err(Reject::BadConfidence)
        );
    }

    #[test]
    fn findings_and_lists_are_capped() {
        let findings: Vec<String> = (0..12).map(|i| format!(r#"{{"text":"f{i}"}}"#)).collect();
        let text = format!(
            r#"{{"summary":"s","findings":[{}],"unresolved":["a","b","c","d","e","f","g"]}}"#,
            findings.join(",")
        );
        let answer = parse_answer(&text, &[]).unwrap();
        assert_eq!(answer.findings.len(), MAX_FINDINGS);
        assert_eq!(answer.cut_findings, 4);
        assert_eq!(answer.unresolved.len(), MAX_UNRESOLVED);
    }

    #[test]
    fn the_parent_reads_a_labelled_bounded_rendering() {
        let answer = parse_answer(
            r#"{"summary":"s","findings":[{"text":"claim","ref":"calc.py:3"}],"unresolved":["what is missing"]}"#,
            &["calc.py"],
        )
        .unwrap();
        let text = render(Role::Explore, &answer, 2);
        assert!(text.starts_with("[subagent explore answer: a small model's reading"));
        assert!(text.contains("- claim (calc.py:3)"));
        assert!(text.contains("unresolved:\n- what is missing"));
        assert!(text.contains("2 input(s) left out"));
        assert!(text.len() <= MAX_RENDERED + 80);
    }
}
