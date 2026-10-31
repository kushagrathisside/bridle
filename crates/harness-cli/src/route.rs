//! Routing for a request: which slot answers it, and why. The chat calls it for each turn when `[routing] auto_slot`
//! is on, and `agent route` calls it for one text.
//!
//! The rules place the request first (`harness_core::routing::place`). A request they cannot place goes to the decision
//! model, Laya, when `[external.laya]` is enabled. The deep slot is used only when Bridle reports it running; otherwise
//! the request goes to `main`, and the reason says so.

use std::path::Path;

use harness_core::routing::{self, Answer, Placed, Slot};
use harness_runtime::laya::{self, LayaClient};
use serde::Deserialize;
use std::time::Duration;

use crate::app::App;
use crate::commands::models;

/// The two choices the decision model may give, with what each one is for.
const CRITERIA: [(&str, &str); 2] = [
    (
        "MAIN",
        "everyday work: small edits, questions about the code, running tests, documentation",
    ),
    (
        "DEEP",
        "work that needs a wider view: designing an architecture, finding a cause across several modules, a long review",
    ),
];

/// The slot a request goes to, and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routed {
    pub slot: String,
    pub reason: String,
}

/// The routed slot for a request sent to `requested`. Routing applies only when `[routing] auto_slot` is on and the
/// request was sent to the default slot: a slot the person chose is kept.
pub fn for_request(app: &App, requested: &str, text: &str) -> Option<Routed> {
    if !app.config.routing.auto_slot || requested != app.config.agent.default_model {
        return None;
    }
    let placed = routing::place(text);
    let answer = match placed {
        Placed::Undecided => ask_laya(app, text),
        Placed::Rule { .. } => None,
    };
    let deep_serving = models::is_running(app, "deep").unwrap_or(false);
    let (slot, reason) = routing::choose(placed, answer, deep_serving);
    Some(Routed {
        slot: slot.name().to_owned(),
        reason,
    })
}

/// Asks the decision model for an undecided request. `None` when Laya is off, so the rules and the default decide.
fn ask_laya(app: &App, text: &str) -> Option<Result<Answer, String>> {
    let laya = &app.config.external.laya;
    if !laya.enabled {
        return None;
    }
    let answer = (|| {
        let endpoint = laya
            .endpoint
            .as_deref()
            .ok_or_else(|| "external.laya has no endpoint".to_owned())?;
        let client = LayaClient::new(endpoint, Duration::from_millis(laya.timeout_ms))?;
        let body = laya::route_request(text, &CRITERIA);
        let choices: Vec<&str> = CRITERIA.iter().map(|(choice, _)| *choice).collect();
        let answer = client.ask(&body, laya::ROUTE_QUESTION, &choices)?;
        let slot = match answer.choice.as_str() {
            "MAIN" => Slot::Main,
            _ => Slot::Deep,
        };
        let probability = answer
            .probabilities
            .get(&answer.choice)
            .copied()
            .unwrap_or(0.0);
        Ok(Answer {
            slot,
            reason: format!(
                "the decision model chose {} ({:.0}% probability)",
                answer.choice,
                probability * 100.0
            ),
        })
    })();
    Some(answer)
}

/// `agent route <text>`: prints the slot a request would go to, and the reason. Sends nothing when the rules place the
/// request; an undecided request is sent to the decision model only when `[external.laya]` is enabled.
pub fn run(app: &App, text: &str) -> anyhow::Result<std::process::ExitCode> {
    let default = app.config.agent.default_model.clone();
    match for_request(app, &default, text) {
        Some(routed) => println!("slot: {}\nreason: {}", routed.slot, routed.reason),
        None => println!(
            "slot: {default}\nreason: routing is off (routing.auto_slot is false), so the default slot answers"
        ),
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// One measured decision: the slot chosen, where the choice came from, and how long it took.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub slot: Slot,
    pub source: &'static str,
    pub reason: String,
    pub latency_ms: u128,
}

/// The choice for a request, for measurement. Unlike [`for_request`] it does not ask whether the deep slot is serving, so
/// the choice itself is what gets measured.
pub fn decide(app: &App, text: &str) -> Measured {
    let started = std::time::Instant::now();
    let placed = routing::place(text);
    let source = match &placed {
        Placed::Rule { .. } => "rule",
        Placed::Undecided if app.config.external.laya.enabled => "decision model",
        Placed::Undecided => "no decision model",
    };
    let answer = match placed {
        Placed::Undecided => ask_laya(app, text),
        Placed::Rule { .. } => None,
    };
    let (slot, reason) = routing::choose(placed, answer, true);
    Measured {
        slot,
        source,
        reason,
        latency_ms: started.elapsed().as_millis(),
    }
}

/// One labelled request of the routing corpus (`evals/routing/cases.json`).
#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    text: String,
    expected: String,
}

/// `agent eval-routing <cases>`: runs each labelled request through the placement and prints the agreement, by source.
/// The labels are a judgement that a person must review; the report says how many of each source agreed.
pub fn eval(
    app: &App,
    cases_path: &Path,
    out: Option<&Path>,
) -> anyhow::Result<std::process::ExitCode> {
    let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(cases_path)?)?;
    let mut rows = Vec::new();
    for case in &cases {
        let measured = decide(app, &case.text);
        let got = measured.slot.name().to_uppercase();
        let correct = got == case.expected.to_uppercase();
        println!(
            "{:<22} expected {:<4} got {:<4} {:<18} {:>5} ms  {}",
            case.name,
            case.expected,
            got,
            measured.source,
            measured.latency_ms,
            if correct { "ok" } else { "DIFFERS" }
        );
        rows.push(serde_json::json!({
            "name": case.name,
            "expected": case.expected,
            "got": got,
            "source": measured.source,
            "reason": measured.reason,
            "latency_ms": measured.latency_ms,
            "correct": correct,
        }));
    }
    let total = rows.len();
    let correct = rows.iter().filter(|row| row["correct"] == true).count();
    println!("agreement: {correct} of {total}");
    for source in ["rule", "decision model", "no decision model"] {
        let of: Vec<&serde_json::Value> =
            rows.iter().filter(|row| row["source"] == source).collect();
        if of.is_empty() {
            continue;
        }
        let agreed = of.iter().filter(|row| row["correct"] == true).count();
        println!("  {source}: {agreed} of {}", of.len());
    }
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&rows)? + "\n")?;
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_criteria_name_exactly_the_two_slots_the_router_knows() {
        let names: Vec<&str> = CRITERIA.iter().map(|(choice, _)| *choice).collect();
        assert_eq!(names, ["MAIN", "DEEP"]);
    }
}
