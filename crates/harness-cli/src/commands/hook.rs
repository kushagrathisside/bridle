//! `agent hook <event>`: the OpenCode plugin shim's bridge into the policy engine.
//!
//! Protocol: one JSON payload on stdin, one JSON answer on stdout, exit code 0. Even when the
//! config or workspace is broken the command still answers (fail-closed answers for policy
//! hooks), so the shim never has to guess.

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use harness_core::security::redact::Redactor;
use serde_json::Value;

use crate::app::App;
use crate::host::opencode_v1::{self, HookContext, HookEvent};

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
    let (answer, telemetry) = opencode_v1::handle(event, input, &ctx);
    if let Some(ev) = telemetry {
        // Telemetry must never break the agent loop.
        let _ = app.event_log().append(ev);
    }
    answer
}
