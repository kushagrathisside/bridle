//! `agent config show|validate|paths`.

use std::process::ExitCode;

use clap::Subcommand;
use harness_core::config::{ConfigIssue, Severity, machine_config_path};
use harness_core::security::redact::Redactor;

use crate::app::App;

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the effective (merged) config as TOML, redacted.
    Show,
    /// Validate the effective config; exit code 1 on errors.
    Validate,
    /// Show which config files were loaded and where the machine config lives.
    Paths,
}

pub fn run(app: &App, action: &ConfigAction) -> anyhow::Result<ExitCode> {
    match action {
        ConfigAction::Show => {
            let text = toml::to_string_pretty(&app.config)?;
            print!("{}", Redactor::new().redact(&text));
            Ok(ExitCode::SUCCESS)
        }
        ConfigAction::Validate => {
            if app.issues.is_empty() {
                println!(
                    "✓ configuration is valid ({} file(s))",
                    app.config_files.len()
                );
                return Ok(ExitCode::SUCCESS);
            }
            let redactor = Redactor::new();
            for issue in &app.issues {
                println!("{}", issue_line(&redactor, issue));
            }
            Ok(if app.has_config_errors() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        ConfigAction::Paths => {
            println!("workspace: {}", app.workspace.display());
            match machine_config_path() {
                Some(p) => println!(
                    "machine config: {}{}",
                    p.display(),
                    if p.exists() { "" } else { " (absent)" }
                ),
                None => println!("machine config: (no location)"),
            }
            for f in &app.config_files {
                println!("loaded: {}", f.display());
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// One printed line for a validation issue. Messages can echo a config value (an unknown
/// `default_model`, for example), so the whole line goes through the redactor.
fn issue_line(redactor: &Redactor, issue: &ConfigIssue) -> String {
    let mark = if issue.severity == Severity::Error {
        "✗"
    } else {
        "⚠"
    };
    let line = format!("{mark} [{}] {}: {}", issue.rule, issue.path, issue.message);
    redactor.redact(&line).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_lines_do_not_echo_secret_values() {
        let issue = ConfigIssue {
            severity: Severity::Error,
            rule: "V2",
            path: "agent.default_model".to_owned(),
            message: "`api_key=hunter2` is not a key of [models]".to_owned(),
        };
        // Failure messages do not print the line, so the value never reaches test output.
        let line = issue_line(&Redactor::new(), &issue);
        assert!(
            !line.contains("hunter2"),
            "the secret value survived redaction"
        );
        assert!(
            line.starts_with("✗ [V2] agent.default_model:"),
            "unexpected line prefix"
        );
    }
}
