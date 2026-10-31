//! What the system prompt gets beyond its fixed text: the repository memory, then the task state. Both are checked
//! before they are added, and neither changes a verdict. The hook and the chat call the same function, so the model
//! sees the same text in both hosts.

use anyhow::Result;
use harness_core::security::redact::Redactor;
use harness_runtime::memory;
use serde_json::Value;

use crate::app::App;
use crate::workflow;

/// The extra text for the system prompt. `degraded` names a part that could not be read. The other part is still
/// returned, so a broken memory file does not hide the task state.
pub struct Extras {
    pub text: Option<String>,
    pub degraded: Option<anyhow::Error>,
}

/// The memory block, then the task block, joined by a blank line. `input` is the hook payload or the chat's
/// `{ "sessionID": ... }`; only the task block reads it.
pub fn extras(app: &App, input: &Value) -> Extras {
    let mut parts = Vec::new();
    let mut degraded = None;
    match memory_block(app) {
        Ok(Some(block)) => parts.push(block),
        Ok(None) => {}
        Err(error) => degraded = Some(error),
    }
    match workflow::system_block(app, input) {
        Ok(Some(block)) => parts.push(block),
        Ok(None) => {}
        Err(error) => degraded = degraded.or(Some(error)),
    }
    Extras {
        text: (!parts.is_empty()).then(|| parts.join("\n\n")),
        degraded,
    }
}

/// The repository memory, read through the same policy the tools use.
fn memory_block(app: &App) -> Result<Option<String>> {
    let policy = app.policy()?;
    memory::load(
        &app.config.memory,
        &app.workspace,
        &policy,
        &Redactor::new(),
    )
    .map_err(anyhow::Error::msg)
}
