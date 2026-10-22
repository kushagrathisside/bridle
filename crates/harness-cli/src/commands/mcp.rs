//! `agent mcp`: serve the harness tools over MCP on stdio for the coding-agent host.

use std::process::ExitCode;

use harness_core::identity;
use harness_core::security::redact::Redactor;
use harness_runtime::mcp::{Server, ServerInfo, ToolContext, default_tools};

use crate::app::App;

pub fn run(app: &App) -> anyhow::Result<ExitCode> {
    app.require_valid_config()?;
    let ctx = ToolContext {
        workspace: app.workspace.clone(),
        config: app.config.clone(),
        verification: app.config.verification.clone(),
        policy: app.policy()?,
        redactor: Redactor::new(),
    };
    let info = ServerInfo {
        name: identity::MCP_SERVER_NAME.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        instructions: format!(
            "Deterministic tools from {}. Prefer these over reconstructing repository state by \
             hand: git_status and git_diff for changes, run_verification to run the project's \
             configured checks (tests, lint) — a change is not done until verification passes.",
            identity::PRODUCT_ID
        ),
    };
    let mut server = Server::new(info, default_tools(), ctx);
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    server.serve(stdin, stdout)?;
    Ok(ExitCode::SUCCESS)
}
