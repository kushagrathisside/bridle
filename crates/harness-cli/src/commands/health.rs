//! `agent health` (§44). IMPLEMENTATION SPEC: see crates/harness-cli/DOCTOR_HEALTH_MODELS_SPEC.md.

use std::process::ExitCode;

use crate::app::App;

pub fn run(app: &App, slot: Option<&str>, quick: bool, json: bool) -> anyhow::Result<ExitCode> {
    let _ = (app, slot, quick, json);
    todo!("wave 2")
}
