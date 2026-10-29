//! `agent eval-decisions`: an A/B run of the rules and the micro model on recorded cases.
//!
//! Each case is a task state and the loop signals it produced. The rules decide every case. The micro
//! slot decides the stalled ones. The report shows whether each model answer is one the harness allows, how
//! often the model agrees with the rules, how often it fell back, and how long it took.

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use harness_core::decision::{DecisionEngine, DecisionInput, DecisionResult, RulesDecisionEngine};
use harness_core::lfm;
use harness_core::loops::LoopSignal;
use harness_core::task::TaskState;
use harness_runtime::decision::{LfmEngine, PROVIDER_NAME};
use harness_runtime::providers::provider_for;
use serde::{Deserialize, Serialize};

use crate::app::App;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    state: TaskState,
    signals: Vec<LoopSignal>,
}

#[derive(Debug, Serialize)]
struct Row {
    name: String,
    stalled: bool,
    rules: String,
    source: &'static str,
    model: Option<String>,
    model_reason: Option<String>,
    agrees: Option<bool>,
    latency_ms: u64,
}

#[derive(Debug, Serialize)]
struct Summary {
    cases: usize,
    stalled: usize,
    model_answers: usize,
    fallbacks: usize,
    /// Model answers that are in the allowed set. Anything else would be a bug in the engine.
    answers_allowed: usize,
    agreements: usize,
    latency_p50_ms: u64,
    latency_p95_ms: u64,
}

#[derive(Debug, Serialize)]
struct Report {
    model: String,
    summary: Summary,
    rows: Vec<Row>,
}

pub fn run(app: &App, slot: &str, cases_path: &Path, out: Option<&Path>) -> Result<ExitCode> {
    let text = std::fs::read_to_string(cases_path)
        .with_context(|| format!("cannot read {}", cases_path.display()))?;
    let cases: Vec<Case> =
        serde_json::from_str(&text).context("the cases file is not valid JSON")?;
    let Some(model) = app.config.models.get(slot) else {
        bail!("no model slot `{slot}` is configured");
    };
    let provider = provider_for(model, &|name| std::env::var(name).ok())
        .map_err(|error| anyhow::anyhow!("cannot build the micro provider: {error}"))?;
    let lfm = LfmEngine::new(provider);

    let mut rows = Vec::with_capacity(cases.len());
    for case in &cases {
        let input = DecisionInput {
            state: &case.state,
            signals: &case.signals,
        };
        let rules = RulesDecisionEngine.decide(&input);
        let stalled = lfm::is_stalled(rules.reason_code);
        let started = Instant::now();
        let model_result = lfm.decide(&input);
        let wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        rows.push(row(case, stalled, &rules, &model_result, wall_ms));
    }

    let report = Report {
        model: model.id.clone(),
        summary: summarise(&rows),
        rows,
    };
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = out {
        std::fs::write(path, format!("{json}\n"))
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn row(
    case: &Case,
    stalled: bool,
    rules: &DecisionResult,
    model: &DecisionResult,
    wall_ms: u64,
) -> Row {
    let source = if !stalled {
        "rules"
    } else if model.provider == PROVIDER_NAME {
        "model"
    } else {
        "fallback"
    };
    let answered = source == "model";
    // A fallback keeps its cause in the report, so a failed call is not read as a rules decision.
    let model_reason = if source == "fallback" {
        Some(model.provider.clone())
    } else {
        model.model_reason.clone()
    };
    Row {
        name: case.name.clone(),
        stalled,
        rules: format!("{:?}", rules.decision),
        source,
        model: answered.then(|| format!("{:?}", model.decision)),
        model_reason,
        agrees: answered.then(|| model.decision == rules.decision),
        latency_ms: if stalled { wall_ms } else { 0 },
    }
}

fn summarise(rows: &[Row]) -> Summary {
    let stalled = rows.iter().filter(|row| row.stalled).count();
    let model_answers = rows.iter().filter(|row| row.source == "model").count();
    let fallbacks = rows.iter().filter(|row| row.source == "fallback").count();
    let answers_allowed = rows
        .iter()
        .filter(|row| row.source == "model")
        .filter(|row| {
            matches!(
                row.model.as_deref(),
                Some("Escalate" | "AskUser" | "Replan")
            )
        })
        .count();
    let agreements = rows.iter().filter(|row| row.agrees == Some(true)).count();
    let mut timings: Vec<u64> = rows
        .iter()
        .filter(|row| row.stalled)
        .map(|row| row.latency_ms)
        .collect();
    timings.sort_unstable();
    Summary {
        cases: rows.len(),
        stalled,
        model_answers,
        fallbacks,
        answers_allowed,
        agreements,
        latency_p50_ms: percentile(&timings, 50),
        latency_p95_ms: percentile(&timings, 95),
    }
}

fn percentile(sorted: &[u64], pct: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() * pct).div_ceil(100)).saturating_sub(1);
    sorted[index.min(sorted.len() - 1)]
}
