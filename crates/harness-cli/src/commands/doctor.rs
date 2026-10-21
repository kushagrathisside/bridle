//! `agent doctor` (§43). IMPLEMENTATION SPEC: see crates/harness-cli/DOCTOR_HEALTH_MODELS_SPEC.md.

use std::process::ExitCode;

use crate::app::App;

pub fn run(app: &App, json: bool) -> anyhow::Result<ExitCode> {
    let _ = (app, json);
    todo!("wave 2")
}
