//! `bridle desktop screenshot PROGRAM [ARGS...]`: the `window_screenshot` tool for a person at the terminal (ADR-0021). It runs the same
//! tool with the same checks as the model gets: the program must be a bare name on PATH, its arguments plain, and the command policy must
//! allow it (add the program to `security.safe_commands` in the machine configuration for a program you use). It needs
//! `[desktop] screenshots_enabled` in the machine configuration.

use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::Subcommand;
use harness_core::security::redact::Redactor;
use harness_runtime::mcp::tools::{McpTool, ToolContext, window::WindowScreenshotTool};
use serde_json::json;

use crate::app::App;

#[derive(Debug, Subcommand)]
pub enum DesktopAction {
    /// Start a program, wait for its window, save a picture of it, and stop the program.
    Screenshot {
        /// The program (a bare name on PATH), then its arguments.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
        /// How long to wait for the window, in milliseconds.
        #[arg(long)]
        wait_ms: Option<u64>,
        /// Text the wanted window's title contains, if the program opens several windows.
        #[arg(long)]
        title: Option<String>,
    },
}

pub fn run(app: &App, action: &DesktopAction) -> Result<ExitCode> {
    match action {
        DesktopAction::Screenshot {
            command,
            wait_ms,
            title,
        } => {
            if !app.config.desktop.screenshots_enabled {
                bail!(
                    "window screenshots are off: set [desktop] screenshots_enabled = true in the machine configuration"
                );
            }
            let ctx = ToolContext {
                workspace: app.workspace.clone(),
                config: app.config.clone(),
                verification: app.config.verification.clone(),
                policy: app.policy()?,
                redactor: Redactor::new(),
            };
            let (program, words) = command
                .split_first()
                .expect("clap requires at least the program");
            let mut args = json!({"program": program, "args": words});
            if let Some(ms) = wait_ms {
                args["wait_ms"] = json!(ms);
            }
            if let Some(t) = title {
                args["title_contains"] = json!(t);
            }
            match WindowScreenshotTool.call(&args, &ctx) {
                Ok(out) => {
                    print!("{}", out.text);
                    let blocked = out
                        .structured
                        .as_ref()
                        .is_some_and(|s| s["status"] == "blocked");
                    Ok(if blocked {
                        ExitCode::FAILURE
                    } else {
                        ExitCode::SUCCESS
                    })
                }
                Err(message) => {
                    eprintln!("bridle: {message}");
                    Ok(ExitCode::FAILURE)
                }
            }
        }
    }
}
