//! `agent config show|validate|paths`.

use std::process::ExitCode;

use clap::Subcommand;
use harness_core::config::{Severity, machine_config_path};
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
            for i in &app.issues {
                let mark = if i.severity == Severity::Error {
                    "✗"
                } else {
                    "⚠"
                };
                println!("{mark} [{}] {}: {}", i.rule, i.path, i.message);
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
