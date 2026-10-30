//! The orchestrator's hook glue.
//!
//! Before a call runs, the loop rules look at the task's recent calls and may refuse the call. After a
//! call, the result of a verification is recorded. Both run only when `[workflow].enabled` is true, and
//! only after the policy has allowed the call, so the policy keeps the final say. The OpenCode payload
//! mapping lives here and in `host::opencode_v1`; the rules and the records live in `harness-core`.

use std::time::Duration;

use anyhow::Result;
use harness_core::config::{Config, DecisionProviderKind, Hardware, ModelRole};
use harness_core::decision::{
    DecisionEngine, DecisionInput, DecisionResult, ReasonCode, RulesDecisionEngine, escalation_due,
    refuses,
};
use harness_core::failures::parse_failures;
use harness_core::fingerprint::{Context, args_hash, command_hash, fingerprints};
use harness_core::journal::{Outcome, Record};
use harness_core::loops::{Thresholds, detect};
use harness_core::models::ModelRegistry;
use harness_core::task::TaskState;
use harness_runtime::decision::{ESCALATED_PREFIX, ESCALATION_TIMEOUT_MS, LfmEngine};
use harness_runtime::jev::JevEngine;
use harness_runtime::launcher;
use harness_runtime::mcp::record::is_verification_tool;
use harness_runtime::providers::provider_for;
use harness_runtime::state::{DecisionRecord, StateDir, Task};
use serde_json::Value;

use crate::app::App;
use crate::commands::models;

/// The prefix OpenCode adds to MCP tool names.
const MCP_PREFIX: &str = "local-agent_";
/// Built-in tools that edit a file, and the argument that names the file.
const EDIT_TOOLS: [&str; 5] = ["edit", "write", "multiedit", "patch", "apply_patch"];
/// Command words that run a test suite. Their output is parsed for failures.
const TEST_RUNNERS: [&str; 8] = [
    "pytest",
    "unittest",
    "cargo test",
    "go test",
    "npm test",
    "ctest",
    "gtest",
    "tox",
];

/// Runs the loop rules before an allowed call. Returns the refusal message when the call is refused.
pub fn before(app: &App, input: &Value, read_only: bool) -> Result<Option<String>> {
    if !app.config.workflow.enabled {
        return Ok(None);
    }
    let session = str_field(input, "sessionID");
    let call_id = str_field(input, "callID");
    let tool = str_field(input, "tool");
    let args = input.get("args").cloned().unwrap_or(Value::Null);
    let command = args.get("command").and_then(Value::as_str);

    let state = StateDir::open(&app.workspace)?;
    let repository = app.workspace.display().to_string();
    let task_id = state.bind_session(session, &repository)?;
    let task = state.task(&task_id)?;
    let journal = task.snapshot()?;

    let thresholds = Thresholds::from_config(&app.config.agent, &app.config.workflow);
    let signals = detect(&journal.ring, &journal.state, &thresholds);
    let answered = answered_loop(&task, &journal.state)?;
    let result = decide_for(
        app,
        &DecisionInput {
            state: &journal.state,
            signals: &signals,
        },
        journal.state.escalation_count,
        answered.as_ref(),
    );
    // An escalation made here, rather than at the failure that caused it, is counted before the call is judged. An
    // answer that is already open was counted when it was given.
    if is_escalated(&result) && !journal.state.escalation_open {
        task.append(Record::Escalated {
            call_id: call_id.to_owned(),
        })?;
    }

    // Read-only calls are never refused: they cannot repeat a failure, and the model needs them to read it.
    if !read_only
        && refuses(
            &result,
            journal.state.consecutive_refusals,
            app.config.workflow.refusal_cap,
        )
    {
        let message = refusal_message(&result);
        task.append(Record::Refused {
            call_id: call_id.to_owned(),
            rule: rule_name(result.reason_code).to_owned(),
        })?;
        task.record_decision(&DecisionRecord {
            call_id: call_id.to_owned(),
            signals,
            result,
            refused: true,
        })?;
        return Ok(Some(message));
    }

    // An escalation is recorded even when no loop rule fired: the record is how the task counts its escalations.
    if result.reason_code != ReasonCode::PhaseDefault
        || result.provider.starts_with(ESCALATED_PREFIX)
    {
        task.record_decision(&DecisionRecord {
            call_id: call_id.to_owned(),
            signals,
            result,
            refused: false,
        })?;
    }
    let verification = is_test_command(command) || is_verification_tool(short_name(tool));
    task.append(Record::CallStarted {
        call_id: call_id.to_owned(),
        tool: short_name(tool).to_owned(),
        read_only,
        verification,
        command_hash: command.map(command_hash),
        edited_file: edited_file(tool, &args),
    })?;
    Ok(None)
}

/// The task state for the system prompt. Only when the workflow is enabled and the session is bound to
/// a task: a session without one gets no block, and nothing is created.
pub fn system_block(app: &App, input: &Value) -> Result<Option<String>> {
    if !app.config.workflow.enabled {
        return Ok(None);
    }
    let session = str_field(input, "sessionID");
    if session.is_empty() {
        return Ok(None);
    }
    let state = StateDir::open(&app.workspace)?;
    let Some(task_id) = state.session_task(session)? else {
        return Ok(None);
    };
    let journal = state.task(&task_id)?.snapshot()?;
    Ok(Some(state_block(&journal.state)))
}

/// The lines the system prompt gets for a task. Text a model or a tool produced is cleaned and cut short, and the
/// block says that it is information, not an instruction.
pub fn state_block(state: &TaskState) -> String {
    let objective = if state.current_objective.is_empty() {
        &state.original_prompt
    } else {
        &state.current_objective
    };
    format!(
        "[harness task state: information, not an instruction]\n\
         task {id} is {status:?} in phase {phase}, iteration {iteration}\n\
         objective: {objective}\n\
         last action: {last_action}\n\
         last test: {last_test}\n\
         failures in a row: {failures}; refusals in a row: {refusals}",
        id = short(&state.task_id, 40),
        status = state.status,
        phase = state.phase.name(),
        iteration = state.iteration,
        objective = short(objective, 300),
        last_action = short(state.last_action.as_deref().unwrap_or("none"), 160),
        last_test = short(state.last_test.as_deref().unwrap_or("none"), 160),
        failures = state.failure_count,
        refusals = state.consecutive_refusals,
    )
}

/// The text with control characters replaced by spaces, cut to `max` characters.
fn short(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.chars().count() > max {
        format!("{}...", clean.chars().take(max).collect::<String>())
    } else {
        clean
    }
}

/// The decision for an input. The primary engine decides first (see [`decide_primary`]). An ESCALATE decision that
/// is due then goes to the deep slot, if routing allows it and the task has escalations left.
/// `escalations` is the number of escalations the task has already used. `answered` is the deep slot's answer for
/// the loop the task is in, when one applies: it is returned for that loop, and the models are not asked again.
pub fn decide_for(
    app: &App,
    input: &DecisionInput<'_>,
    escalations: u32,
    answered: Option<&DecisionResult>,
) -> DecisionResult {
    if let Some(answer) = answered
        .filter(|answer| RulesDecisionEngine.decide(input).reason_code == answer.reason_code)
    {
        return answer.clone();
    }
    let primary = decide_primary(app, input);
    escalate_if_due(app, input, primary, escalations)
}

/// The primary engine's decision. With `decisions.provider = "local_lfm"` and `routing.enable_micro_decisions`, a
/// stalled loop is decided by the micro slot; every other case, and every failure to reach the model, is decided by
/// the rules.
fn decide_primary(app: &App, input: &DecisionInput<'_>) -> DecisionResult {
    let env = |name: &str| std::env::var(name).ok();
    if app.config.decisions.provider == DecisionProviderKind::Jev {
        let engine = if app.config.external.jev.enabled {
            JevEngine::from_config(&app.config.external.jev, &env)
        } else {
            Err("external.jev is not enabled".to_owned())
        };
        return match engine {
            Ok(engine) => engine.decide(input),
            Err(error) => fallback_to_rules(input, "jev", &error),
        };
    }
    let registry = ModelRegistry::from_config(&app.config);
    if let (DecisionProviderKind::LocalLfm, true, Some((_, model))) = (
        app.config.decisions.provider,
        app.config.routing.enable_micro_decisions,
        registry.first_with_role(ModelRole::Decision),
    ) {
        return match provider_for(model, &env) {
            Ok(provider) => LfmEngine::new(provider).decide(input),
            Err(error) => fallback_to_rules(input, "local_lfm", &error.to_string()),
        };
    }
    RulesDecisionEngine.decide(input)
}

/// How long the deep slot may take to answer health after it starts, and how long a parked slot may take to answer
/// health again. Both run inside the hook, whose limit is 150 s in the shim (see `opencode-plugin/local-agent.ts`).
const DEEP_START_WAIT: Duration = Duration::from_secs(45);
const PARKED_RESTORE_WAIT: Duration = Duration::from_secs(45);

/// Sends a due escalation to the deep slot. Routing must be on, the task must have escalations left,
/// and a deep slot must be defined (validation V39). When the deep slot runs on the GPU and is not serving, the GPU
/// slots that run are parked for the call and started again afterwards. Any failure keeps the escalated
/// decision, which goes to the person.
fn escalate_if_due(
    app: &App,
    input: &DecisionInput<'_>,
    primary: DecisionResult,
    escalations: u32,
) -> DecisionResult {
    let routing = &app.config.routing;
    if !routing.enable_deep_escalation
        || escalations >= app.config.agent.max_deep_escalations
        || !escalation_due(&primary, routing.escalation_threshold)
    {
        return primary;
    }
    let registry = ModelRegistry::from_config(&app.config);
    let Some((deep_name, deep)) = registry.first_with_role(ModelRole::DeepReasoning) else {
        return primary;
    };
    let deep_name = deep_name.to_owned();
    let mut deep = deep.clone();
    deep.timeout_ms = deep.timeout_ms.min(ESCALATION_TIMEOUT_MS);
    let env = |name: &str| std::env::var(name).ok();
    let provider = match provider_for(&deep, &env) {
        Ok(provider) => provider,
        Err(error) => return failed_escalation(primary, &error.to_string()),
    };
    let needs_swap = deep.hardware == Hardware::Gpu
        && !launcher::is_healthy(&models::health_url(&deep.endpoint));
    if !needs_swap {
        return LfmEngine::new(provider).escalate(input, primary);
    }
    let fallback = primary.clone();
    match with_deep_on_gpu(app, &deep_name, || {
        LfmEngine::new(provider).escalate(input, primary)
    }) {
        Ok(answer) => answer,
        Err(error) => failed_escalation(fallback, &error.to_string()),
    }
}

/// The escalated decision kept, labelled with the reason the escalation did not happen.
fn failed_escalation(primary: DecisionResult, reason: &str) -> DecisionResult {
    DecisionResult {
        provider: format!(
            "{ESCALATED_PREFIX} failed ({reason}); kept {}",
            primary.provider
        ),
        ..primary
    }
}

/// The GPU slots other than `deep`, in name order: the ones a deep call on the GPU must park.
fn other_gpu_slots(config: &Config, deep: &str) -> Vec<String> {
    config
        .models
        .iter()
        .filter(|(name, model)| name.as_str() != deep && model.hardware == Hardware::Gpu)
        .map(|(name, _)| name.clone())
        .collect()
}

/// Runs `call` with the deep slot serving. The running GPU slots are stopped, the deep slot starts, and afterwards the
/// deep slot stops and the parked slots start again, whatever `call` returned. Nothing is printed: the hook's stdout
/// carries the answer. Returns the error when the deep slot did not start; `call` is then not run.
fn with_deep_on_gpu<T>(app: &App, deep: &str, call: impl FnOnce() -> T) -> anyhow::Result<T> {
    let mut parked = Vec::new();
    for slot in other_gpu_slots(&app.config, deep) {
        // A slot that cannot be stopped stays where it is; the slots already parked are still restored below.
        if models::is_running(app, &slot).unwrap_or(false) && models::stop_quiet(app, &slot).is_ok()
        {
            parked.push(slot);
        }
    }
    let answer = models::start_quiet(app, deep, DEEP_START_WAIT).map(|()| call());
    let _ = models::stop_quiet(app, deep);
    for slot in &parked {
        let _ = models::start_quiet(app, slot, PARKED_RESTORE_WAIT);
    }
    answer
}

/// Whether a decision came from the deep slot, successful or not.
fn is_escalated(result: &DecisionResult) -> bool {
    result.provider.starts_with(ESCALATED_PREFIX)
}

/// The deep slot's answer for the loop the task is in, while that answer applies (see `TaskState::escalation_open`).
/// The last decision from the deep slot is the answer: it is written before the journal counts the escalation.
fn answered_loop(task: &Task<'_>, state: &TaskState) -> Result<Option<DecisionResult>> {
    if !state.escalation_open {
        return Ok(None);
    }
    let answer = task
        .decisions()?
        .into_iter()
        .rev()
        .find(|record| is_escalated(&record.result))
        .map(|record| record.result);
    Ok(answer)
}

/// Runs the loop rules when a verification has just failed, so a due escalation is taken at the failure rather than
/// when the agent next makes a call. The answer is kept for the calls after it (see [`answered_loop`]). Nothing happens
/// unless the call is a failed verification the journal knows, and an answer that is already open is not asked for
/// again.
fn escalate_on_failure(app: &App, task: &Task<'_>, call_id: &str) -> Result<()> {
    let journal = task.snapshot()?;
    let failed = journal
        .ring
        .iter()
        .rev()
        .find(|call| call.call_id == call_id)
        .is_some_and(|call| call.passed == Some(false));
    if !failed || journal.state.escalation_open {
        return Ok(());
    }
    let thresholds = Thresholds::from_config(&app.config.agent, &app.config.workflow);
    let signals = detect(&journal.ring, &journal.state, &thresholds);
    let result = decide_for(
        app,
        &DecisionInput {
            state: &journal.state,
            signals: &signals,
        },
        journal.state.escalation_count,
        None,
    );
    if !is_escalated(&result) {
        return Ok(());
    }
    task.record_decision(&DecisionRecord {
        call_id: call_id.to_owned(),
        signals,
        result,
        refused: false,
    })?;
    task.append(Record::Escalated {
        call_id: call_id.to_owned(),
    })?;
    Ok(())
}

/// The rules' decision, labelled as a fallback from the engine that could not decide.
fn fallback_to_rules(input: &DecisionInput<'_>, engine: &str, error: &str) -> DecisionResult {
    let mut result = RulesDecisionEngine.decide(input);
    result.provider = format!("rules (fallback from {engine}: {error})");
    result
}

/// Records the outcome of a verification after the call ran. A call that is not a verification is
/// ignored.
pub fn after(app: &App, input: &Value) -> Result<()> {
    if !app.config.workflow.enabled {
        return Ok(());
    }
    let state = StateDir::open(&app.workspace)?;
    let session = str_field(input, "sessionID");
    let Some(task_id) = state.session_task(session)? else {
        return Ok(());
    };
    let task = state.task(&task_id)?;
    let call_id = str_field(input, "callID");
    let tool = str_field(input, "tool");

    if is_verification_tool(short_name(tool)) {
        let args = input.get("args").cloned().unwrap_or(Value::Null);
        let claimed = state.claim_mcp_result(short_name(tool), &args_hash(&args))?;
        let (outcome, fingerprints) = match claimed {
            Some(result) => (result.outcome, result.fingerprints),
            None => (Outcome::Unknown, Vec::new()),
        };
        task.append(Record::VerificationResult {
            call_id: call_id.to_owned(),
            outcome,
            fingerprints,
        })?;
        return escalate_on_failure(app, &task, call_id);
    }

    // A shell test run: its output is parsed here. The call was marked as a verification before it ran.
    let journal = task.snapshot()?;
    let Some(call) = journal
        .ring
        .iter()
        .rev()
        .find(|call| call.call_id == call_id)
    else {
        return Ok(());
    };
    if !call.verification {
        return Ok(());
    }
    let output = input.get("output").and_then(Value::as_str).unwrap_or("");
    let report = parse_failures(output);
    let command = call.command_hash.clone().unwrap_or_default();
    let fingerprints = fingerprints(
        &report,
        Context {
            command: Some(&command),
            exit_code: None,
        },
    );
    // Parsed output that names no failure is not a pass: the runner's own success is not visible here.
    let outcome = if fingerprints.is_empty() {
        Outcome::Unknown
    } else {
        Outcome::Failed
    };
    task.append(Record::VerificationResult {
        call_id: call_id.to_owned(),
        outcome,
        fingerprints,
    })?;
    escalate_on_failure(app, &task, call_id)
}

/// The refusal text: the rule that fired, why, and the next action that is still allowed.
pub fn refusal_message(result: &DecisionResult) -> String {
    let mut message = rule_message(result);
    if let Some(reason) = &result.model_reason {
        message.push_str(&format!(
            " The model's reason: {}.",
            reason.trim_end_matches('.')
        ));
    }
    message
}

fn rule_message(result: &DecisionResult) -> String {
    let (why, next) = match result.reason_code {
        ReasonCode::IterationLimit => (
            "the task reached its iteration limit",
            "stop and report to the user, or run `agent task reset` to continue",
        ),
        ReasonCode::ImplementationRetries => (
            "IMPLEMENT was entered more often than the retry limit allows",
            "explain the blocker to the user and wait for a decision",
        ),
        ReasonCode::SameFailure => (
            "the same failure came back in consecutive verifications",
            "read the failure and change the approach, or ask the user",
        ),
        ReasonCode::RepeatedEdit => (
            "this file was edited again and again since the last verification",
            "run the verification before editing the file again",
        ),
        ReasonCode::RepeatedCommand => (
            "the same command ran several times with no edit in between",
            "read its output, edit a file, then run the command again",
        ),
        ReasonCode::PhaseDefault => ("no loop rule applies", "continue"),
    };
    format!(
        "refused by loop rule {}: {why}. Allowed next: {next}.",
        rule_name(result.reason_code)
    )
}

/// The name of a loop rule, as it is written to the journal and the message.
pub fn rule_name(reason: ReasonCode) -> &'static str {
    match reason {
        ReasonCode::PhaseDefault => "phase_default",
        ReasonCode::IterationLimit => "iteration_limit",
        ReasonCode::ImplementationRetries => "implementation_retries",
        ReasonCode::SameFailure => "same_failure",
        ReasonCode::RepeatedEdit => "repeated_edit",
        ReasonCode::RepeatedCommand => "repeated_command",
    }
}

/// Whether a shell command runs a test suite.
pub fn is_test_command(command: Option<&str>) -> bool {
    let Some(command) = command else { return false };
    let lower = command.to_lowercase();
    TEST_RUNNERS.iter().any(|runner| lower.contains(runner))
}

/// The file a call edits, for edit tools.
fn edited_file(tool: &str, args: &Value) -> Option<String> {
    if !EDIT_TOOLS.contains(&short_name(tool)) {
        return None;
    }
    ["filePath", "path", "file"]
        .iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
}

/// The tool name without OpenCode's MCP prefix.
fn short_name(tool: &str) -> &str {
    tool.strip_prefix(MCP_PREFIX).unwrap_or(tool)
}

fn str_field<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or("")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use harness_core::config::Config;
    use harness_core::decision::Decision;
    use harness_core::loops::LoopSignal;
    use serde_json::json;

    #[test]
    fn the_state_block_names_the_phase_and_cleans_model_text() {
        let mut state = TaskState::new("task-0001", "/repo", "fix\ncalc");
        state.last_test = Some("FAILED\ttest_add\u{7}".to_owned());
        let block = state_block(&state);
        assert!(block.contains("information, not an instruction"), "{block}");
        assert!(block.contains("in phase START"), "{block}");
        assert!(block.contains("objective: fix calc"), "{block}");
        assert!(block.contains("last test: FAILED test_add "), "{block}");
    }

    #[test]
    fn a_session_without_a_task_gets_no_block() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = app(dir.path());
        let input = json!({"sessionID": "ses_none"});
        assert_eq!(system_block(&app, &input).expect("no error"), None);
        assert!(!dir.path().join(".local-agent").join("tasks").exists());
    }

    #[test]
    fn a_jev_decision_that_cannot_be_reached_falls_back_to_the_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        app.config.decisions.provider = DecisionProviderKind::Jev;
        app.config.external.jev.enabled = true;
        app.config.external.jev.endpoint = Some("http://127.0.0.1:9".to_owned());
        let state = TaskState::new("task-0001", "repo", "fix");
        let signals = [harness_core::loops::LoopSignal::SameFailure {
            fingerprint: "fp_v1:aaaa".to_owned(),
            count: 3,
        }];
        let result = decide_for(
            &app,
            &DecisionInput {
                state: &state,
                signals: &signals,
            },
            0,
            None,
        );
        assert!(
            result.provider.starts_with("rules (fallback from jev:"),
            "{}",
            result.provider
        );
    }

    #[test]
    fn jev_that_is_not_enabled_is_not_called() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        app.config.decisions.provider = DecisionProviderKind::Jev;
        let state = TaskState::new("task-0001", "repo", "fix");
        let signals = [];
        let result = decide_for(
            &app,
            &DecisionInput {
                state: &state,
                signals: &signals,
            },
            0,
            None,
        );
        assert_eq!(
            result.provider,
            "rules (fallback from jev: external.jev is not enabled)"
        );
    }

    /// Adds a deep slot that answers with the mock provider. The mock's reply is not a decision, so an escalation
    /// to it fails, which is the path these tests check.
    fn with_deep_slot(app: &mut App) {
        let deep: harness_core::config::ModelConfig = serde_json::from_value(json!({
            "provider": "mock",
            "id": "deep-mock",
            "role": "deep_reasoning",
            "endpoint": "http://127.0.0.1:9/v1",
            "context": 8192,
            "hardware": "gpu",
        }))
        .expect("test model config is valid");
        app.config.models.insert("deep".to_owned(), deep);
    }

    #[test]
    fn only_the_other_gpu_slots_are_parked_for_a_deep_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        for (name, role, hardware) in [
            ("main", "engineer", "gpu"),
            ("micro", "decision", "cpu"),
            ("deep", "deep_reasoning", "gpu"),
        ] {
            let model: harness_core::config::ModelConfig = serde_json::from_value(json!({
                "provider": "mock",
                "id": name,
                "role": role,
                "endpoint": "http://127.0.0.1:9/v1",
                "context": 8192,
                "hardware": hardware,
            }))
            .expect("test model config is valid");
            app.config.models.insert(name.to_owned(), model);
        }
        // The CPU micro model stays where it is; the GPU main model is parked while the deep slot runs.
        assert_eq!(
            other_gpu_slots(&app.config, "deep"),
            vec!["main".to_owned()]
        );
    }

    /// Live check of the escalation swap (2026-10-28). Needs the main model served, the environment naming the machine's
    /// model and runtime folders, and `BRIDLE_SWAP_WORKSPACE` naming a workspace whose config has launch sections for
    /// `main` and `deep`. It stops and restarts the main model, so it is run by hand.
    #[test]
    #[ignore = "needs the main model served and a workspace with launch sections"]
    fn a_deep_escalation_swaps_the_gpu_model_and_restores_it() {
        let workspace = std::env::var("BRIDLE_SWAP_WORKSPACE").expect("set BRIDLE_SWAP_WORKSPACE");
        let app =
            App::load(None, Some(std::path::Path::new(&workspace))).expect("the workspace loads");
        assert!(
            models::is_running(&app, "main").expect("main's record reads"),
            "the main model must be running before the swap"
        );
        let deep_endpoint = app.config.models["deep"].endpoint.clone();
        let deep_answered = with_deep_on_gpu(&app, "deep", || {
            launcher::is_healthy(&models::health_url(&deep_endpoint))
        })
        .expect("the swap runs");
        assert!(
            deep_answered,
            "the deep slot answered health while main was parked"
        );
        assert!(
            models::is_running(&app, "main").expect("main's record reads"),
            "main is started again after the swap"
        );
        assert!(
            !models::is_running(&app, "deep").expect("deep's record reads"),
            "deep is stopped after the swap"
        );
    }

    fn same_failure() -> Vec<LoopSignal> {
        vec![LoopSignal::SameFailure {
            fingerprint: "fp_v1:aaaa".to_owned(),
            count: 3,
        }]
    }

    fn decide_with(app: &App, signals: &[LoopSignal], escalations: u32) -> DecisionResult {
        let state = TaskState::new("task-0001", "repo", "fix");
        decide_for(
            app,
            &DecisionInput {
                state: &state,
                signals,
            },
            escalations,
            None,
        )
    }

    #[test]
    fn an_escalation_is_off_unless_routing_enables_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        with_deep_slot(&mut app);
        let result = decide_with(&app, &same_failure(), 0);
        assert_eq!(result.decision, Decision::Escalate);
        assert_eq!(result.provider, "rules", "the deep slot is not asked");
    }

    #[test]
    fn a_due_escalation_goes_to_the_deep_slot_and_a_failure_keeps_the_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        with_deep_slot(&mut app);
        app.config.routing.enable_deep_escalation = true;
        let result = decide_with(&app, &same_failure(), 0);
        assert_eq!(
            result.decision,
            Decision::Escalate,
            "the escalated case goes to the person"
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

    #[test]
    fn the_escalations_a_task_may_use_are_capped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        with_deep_slot(&mut app);
        app.config.routing.enable_deep_escalation = true;
        let used = app.config.agent.max_deep_escalations;
        let result = decide_with(&app, &same_failure(), used);
        assert_eq!(result.provider, "rules", "no escalation past the cap");
    }

    #[test]
    fn an_escalation_needs_a_deep_slot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        app.config.routing.enable_deep_escalation = true;
        let result = decide_with(&app, &same_failure(), 0);
        assert_eq!(result.provider, "rules", "no deep slot, no escalation");
    }

    #[test]
    fn only_an_escalate_decision_is_escalated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        with_deep_slot(&mut app);
        app.config.routing.enable_deep_escalation = true;
        let repeated = [LoopSignal::RepeatedCommand {
            command_hash: "h".to_owned(),
            count: 3,
        }];
        let result = decide_with(&app, &repeated, 0);
        assert_eq!(
            result.provider, "rules",
            "a diagnose decision is not escalated"
        );
    }

    /// The output of a failing test run. Every run of it has the same fingerprint.
    fn failing_run(call: &str) -> Value {
        json!({
            "sessionID": "ses_test",
            "callID": call,
            "tool": "bash",
            "output": "tests/test_calc.py:5: in test_add\nE       assert -1 == 3\nFAILED tests/test_calc.py::test_add - assert -1 == 3\n"
        })
    }

    /// Runs the test command `count` times in a row, each run failing, after the edit that starts the task.
    fn fail_tests_in_a_row(app: &App, count: usize) {
        before(app, &edit("e0", "calc.py"), false).unwrap();
        for run in 0..count {
            let call = format!("t{run}");
            assert_eq!(
                before(app, &bash(&call, "python -m pytest -q"), false).unwrap(),
                None,
                "run {run}"
            );
            after(app, &failing_run(&call)).unwrap();
        }
    }

    #[test]
    fn a_third_failing_verification_is_escalated_at_the_failure_and_not_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        with_deep_slot(&mut app);
        app.config.routing.enable_deep_escalation = true;
        fail_tests_in_a_row(&app, 3);
        let state = StateDir::open(dir.path()).unwrap();
        let journal = state.task("task-0001").unwrap().snapshot().unwrap();
        assert_eq!(journal.state.escalation_count, 1, "counted at the failure");
        assert!(journal.state.escalation_open);
        // The next call does not ask the deep slot again: the answer for the loop is what refuses it.
        let refused = before(&app, &bash("t3", "python -m pytest -q"), false)
            .unwrap()
            .expect("refused");
        assert!(refused.contains("same_failure"), "{refused}");
        let journal = state.task("task-0001").unwrap().snapshot().unwrap();
        assert_eq!(journal.state.escalation_count, 1, "no second escalation");
    }

    #[test]
    fn the_threshold_decides_whether_the_third_failure_reaches_the_deep_slot() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        with_deep_slot(&mut app);
        app.config.routing.enable_deep_escalation = true;
        app.config.routing.escalation_threshold = 0.9;
        fail_tests_in_a_row(&app, 3);
        let state = StateDir::open(dir.path()).unwrap();
        let journal = state.task("task-0001").unwrap().snapshot().unwrap();
        assert_eq!(
            journal.state.escalation_count, 0,
            "three repeats give 0.75, below 0.9"
        );
        assert!(!journal.state.escalation_open);
    }

    #[test]
    fn the_answer_of_the_deep_slot_is_kept_for_the_loop_it_answered() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let answer = DecisionResult {
            decision: Decision::AskUser,
            confidence: None,
            reason_code: ReasonCode::SameFailure,
            provider: format!("{ESCALATED_PREFIX} (deep-mock)"),
            latency_ms: 0,
            model_reason: Some("the schema cannot hold both rules".to_owned()),
        };
        let state = TaskState::new("task-0001", "repo", "fix");
        let same = same_failure();
        let kept = decide_for(
            &app,
            &DecisionInput {
                state: &state,
                signals: &same,
            },
            1,
            Some(&answer),
        );
        assert_eq!(kept, answer);
        let other = [LoopSignal::RepeatedEdit {
            file: "a.py".to_owned(),
            count: 4,
        }];
        let decided_afresh = decide_for(
            &app,
            &DecisionInput {
                state: &state,
                signals: &other,
            },
            1,
            Some(&answer),
        );
        assert_eq!(
            decided_afresh.reason_code,
            ReasonCode::RepeatedEdit,
            "another loop is decided afresh"
        );
    }

    fn app(dir: &std::path::Path) -> App {
        let mut config = Config::default();
        config.workflow.enabled = true;
        App {
            workspace: dir.to_path_buf(),
            config,
            config_files: Vec::new(),
            issues: Vec::new(),
            origins: std::collections::BTreeMap::new(),
        }
    }

    fn bash(call: &str, command: &str) -> Value {
        json!({"sessionID": "ses_test", "callID": call, "tool": "bash", "args": {"command": command}})
    }

    fn edit(call: &str, file: &str) -> Value {
        json!({"sessionID": "ses_test", "callID": call, "tool": "edit", "args": {"filePath": file}})
    }

    #[test]
    fn the_third_identical_command_in_a_row_is_refused_with_the_rule_and_the_next_action() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        for (i, call) in ["c1", "c2", "c3"].iter().enumerate() {
            assert_eq!(
                before(&app, &bash(call, "ls src"), false).unwrap(),
                None,
                "call {i}"
            );
        }
        let refused = before(&app, &bash("c4", "ls src"), false)
            .unwrap()
            .expect("refused");
        assert!(refused.contains("repeated_command"), "{refused}");
        assert!(refused.contains("Allowed next"), "{refused}");
    }

    #[test]
    fn an_edit_between_identical_commands_clears_the_loop() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        before(&app, &bash("c1", "ls src"), false).unwrap();
        before(&app, &bash("c2", "ls src"), false).unwrap();
        before(&app, &edit("c3", "calc.py"), false).unwrap();
        before(&app, &bash("c4", "ls src"), false).unwrap();
        assert_eq!(before(&app, &bash("c5", "ls src"), false).unwrap(), None);
    }

    #[test]
    fn a_disabled_workflow_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        app.config.workflow.enabled = false;
        for call in ["c1", "c2", "c3", "c4"] {
            assert_eq!(before(&app, &bash(call, "ls"), false).unwrap(), None);
        }
        assert!(!dir.path().join(".local-agent").join("tasks").exists());
    }

    #[test]
    fn a_failing_shell_test_run_is_recorded_as_a_failure_with_its_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        // An edit comes first, as it does in a real session: a test run before any edit has no phase move.
        before(&app, &edit("e0", "calc.py"), false).unwrap();
        before(&app, &bash("c1", "python -m pytest -q"), false).unwrap();
        let input = json!({
            "sessionID": "ses_test",
            "callID": "c1",
            "tool": "bash",
            "output": "tests/test_calc.py:5: in test_add\nE       assert -1 == 3\nFAILED tests/test_calc.py::test_add - assert -1 == 3\n"
        });
        after(&app, &input).unwrap();
        let state = StateDir::open(dir.path()).unwrap();
        let journal = state.task("task-0001").unwrap().snapshot().unwrap();
        assert_eq!(journal.state.phase, harness_core::task::Phase::Diagnose);
        let call = journal.ring.iter().find(|c| c.call_id == "c1").unwrap();
        assert_eq!(call.passed, Some(false));
        assert_eq!(call.fingerprints.len(), 1);
    }

    #[test]
    fn a_test_run_without_parsed_failures_is_unknown_never_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        before(&app, &bash("c1", "cargo test"), false).unwrap();
        let input =
            json!({"sessionID": "ses_test", "callID": "c1", "tool": "bash", "output": "all good"});
        after(&app, &input).unwrap();
        let state = StateDir::open(dir.path()).unwrap();
        let journal = state.task("task-0001").unwrap().snapshot().unwrap();
        let call = journal.ring.iter().find(|c| c.call_id == "c1").unwrap();
        assert_eq!(call.passed, None);
        assert_ne!(journal.state.phase, harness_core::task::Phase::Review);
    }

    #[test]
    fn a_verification_result_from_the_server_is_claimed_by_the_matching_call() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let args = json!({"steps": ["unit"]});
        let start = json!({"sessionID": "ses_test", "callID": "m1", "tool": "local-agent_run_tests", "args": args});
        before(&app, &start, false).unwrap();
        let state = StateDir::open(dir.path()).unwrap();
        state
            .append_mcp_result(
                "run_tests",
                &args_hash(&args),
                Outcome::Failed,
                vec!["fp_v1:0123456789abcdef".to_owned()],
            )
            .unwrap();
        after(&app, &start).unwrap();
        let journal = state.task("task-0001").unwrap().snapshot().unwrap();
        let call = journal.ring.iter().find(|c| c.call_id == "m1").unwrap();
        assert_eq!(call.passed, Some(false));
        assert_eq!(call.fingerprints, vec!["fp_v1:0123456789abcdef".to_owned()]);
        // The same result is never claimed twice.
        assert!(
            state
                .claim_mcp_result("run_tests", &args_hash(&args))
                .unwrap()
                .is_none()
        );
    }
}
