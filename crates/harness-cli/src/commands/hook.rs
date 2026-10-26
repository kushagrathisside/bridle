//! `agent hook <event>`: the OpenCode plugin shim's bridge into the policy engine.
//!
//! Protocol: one JSON payload on stdin, one JSON answer on stdout, exit code 0. Even when the
//! config or workspace is broken the command still answers (fail-closed answers for policy
//! hooks), so the shim never has to guess.

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use harness_core::security::redact::Redactor;
use harness_core::telemetry::{Event, EventKind};
use serde_json::{Value, json};

use crate::app::App;
use crate::host::opencode_v1::{self, HookContext, HookEvent};
use crate::workflow;

pub fn run(event: HookEvent, config: Option<&Path>, workspace: Option<&Path>) -> ExitCode {
    let mut raw = String::new();
    let answer = match std::io::stdin().read_to_string(&mut raw) {
        Err(e) => opencode_v1::failure_answer(event, &format!("cannot read hook payload: {e}")),
        Ok(_) => match serde_json::from_str::<Value>(&raw) {
            Err(e) => opencode_v1::failure_answer(event, &format!("invalid hook payload: {e}")),
            Ok(input) => answer(event, &input, config, workspace),
        },
    };
    let mut out = std::io::stdout().lock();
    // If stdout is gone the shim has already given up; nothing useful left to do.
    let _ = writeln!(out, "{answer}");
    ExitCode::SUCCESS
}

fn answer(
    event: HookEvent,
    input: &Value,
    config: Option<&Path>,
    workspace: Option<&Path>,
) -> Value {
    let app = match App::load(config, workspace) {
        Ok(app) => app,
        Err(e) => return opencode_v1::failure_answer(event, &format!("{e:#}")),
    };
    if app.has_config_errors() {
        return opencode_v1::failure_answer(
            event,
            "configuration has errors; run `agent config validate`",
        );
    }
    let policy = match app.policy() {
        Ok(p) => p,
        Err(e) => return opencode_v1::failure_answer(event, &format!("{e:#}")),
    };
    let redactor = Redactor::new();
    let ctx = HookContext {
        config: &app.config,
        policy: &policy,
        redactor: &redactor,
    };
    let (mut answer, mut telemetry) = opencode_v1::handle(event, input, &ctx);
    // The policy has answered. The orchestrator only adds refusals to allowed calls, and records
    // outcomes; an orchestrator error keeps the policy's answer and is logged, never silently dropped.
    match event {
        HookEvent::ToolBefore if answer["action"] == "allow" => {
            let read_only = answer["class"] == "READ_ONLY";
            match workflow::before(&app, input, read_only) {
                Ok(None) => {}
                Ok(Some(message)) => {
                    telemetry = Some(loop_refusal_event(input, &message));
                    answer = json!({
                        "action": "block",
                        "message": message,
                        "class": answer["class"],
                        "verdict": "deny",
                        "rule": "loop_rule",
                    });
                }
                Err(error) => telemetry = Some(degraded_event(event, input, &error)),
            }
        }
        HookEvent::ToolAfter => {
            if let Err(error) = workflow::after(&app, input) {
                telemetry = Some(degraded_event(event, input, &error));
            }
        }
        HookEvent::SystemTransform => match workflow::system_block(&app, input) {
            Ok(Some(text)) => answer = json!({ "text": text }),
            Ok(None) => {}
            Err(error) => telemetry = Some(degraded_event(event, input, &error)),
        },
        _ => {}
    }
    if let Some(ev) = telemetry {
        // Telemetry must never break the agent loop.
        let _ = app.event_log().append(ev);
    }
    answer
}

/// The log entry for a call refused by a loop rule.
fn loop_refusal_event(input: &Value, message: &str) -> Event {
    let mut ev = Event::now(EventKind::ToolBefore);
    ev.session_id = input
        .get("sessionID")
        .and_then(Value::as_str)
        .map(str::to_owned);
    ev.call_id = input
        .get("callID")
        .and_then(Value::as_str)
        .map(str::to_owned);
    ev.status = Some("blocked".to_owned());
    ev.with("rule", "loop_rule").with("message", message)
}

/// The log entry for an orchestrator error. The policy's answer still stands.
fn degraded_event(event: HookEvent, input: &Value, error: &anyhow::Error) -> Event {
    let kind = match event {
        HookEvent::ToolAfter => EventKind::ToolAfter,
        HookEvent::MessagesTransform | HookEvent::SystemTransform => EventKind::Context,
        _ => EventKind::ToolBefore,
    };
    let mut ev = Event::now(kind);
    ev.session_id = input
        .get("sessionID")
        .and_then(Value::as_str)
        .map(str::to_owned);
    ev.call_id = input
        .get("callID")
        .and_then(Value::as_str)
        .map(str::to_owned);
    ev.status = Some("workflow_degraded".to_owned());
    ev.with("error", format!("{error:#}"))
}
