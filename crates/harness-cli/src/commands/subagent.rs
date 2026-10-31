//! `agent subagent`: runs the subagent tier from the command line, with the same checks as the `subagent` tool.
//! It is the way to test a slot by hand before a model uses the tool.

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use harness_core::security::redact::Redactor;
use harness_core::subagent::Role;
use harness_runtime::subagent::{Request, run_for_config};

use crate::app::App;

pub fn run(
    app: &App,
    role: &str,
    task: &str,
    paths: &[String],
    text: Option<&str>,
) -> Result<ExitCode> {
    app.require_valid_config()?;
    let role = Role::parse(role).ok_or_else(|| anyhow!("role must be `explore` or `summarise`"))?;
    let policy = app.policy()?;
    let redactor = Redactor::new();
    let request = Request {
        role,
        task: task.to_owned(),
        paths: paths.to_vec(),
        text: text.map(str::to_owned),
    };
    match run_for_config(
        &request,
        &app.config,
        &app.workspace,
        &policy,
        &redactor,
        &|name| std::env::var(name).ok(),
    ) {
        Ok(outcome) => {
            print!("{}", redactor.redact(&outcome.rendered));
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("agent: {error}");
            Ok(ExitCode::FAILURE)
        }
    }
}
