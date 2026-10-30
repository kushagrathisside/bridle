//! `agent mcp`: serve the harness tools over MCP on stdio for the coding-agent host.

use std::process::ExitCode;

use harness_core::identity;
use harness_core::security::redact::Redactor;
use harness_runtime::mcp::{
    McpTool, Server, ServerInfo, ToolContext, adapter_tools, default_tools,
};

use crate::app::App;

/// The tools the harness offers for this configuration: the default set, the declared adapters, and the optional subagent
/// and task-status tools. Shared with the chat, which adds its file tools to the same list.
pub(crate) fn tools_for(app: &App) -> Vec<Box<dyn McpTool>> {
    let mut tools = default_tools();
    tools.extend(adapter_tools(&app.config));
    if app.config.subagents.enabled {
        tools.push(Box::new(
            harness_runtime::mcp::tools::subagent::SubagentTool,
        ));
    }
    if app.config.workflow.task_status {
        tools.push(Box::new(harness_runtime::mcp::tools::task::TaskStatusTool));
    }
    tools
}

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
    let mut server = Server::new(info, tools_for(app), ctx);
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    server.serve(stdin, stdout)?;
    Ok(ExitCode::SUCCESS)
}
