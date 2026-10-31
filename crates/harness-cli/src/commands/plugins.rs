//! `agent plugins`: lists the plugin tools the configuration offers, with the class each is offered under, and says what
//! was not loaded and why (ADR-0013). It reads the manifests; it runs nothing.

use std::process::ExitCode;

use anyhow::Result;

use crate::app::App;

pub fn run(app: &App) -> Result<ExitCode> {
    let loaded = harness_runtime::mcp::tools::plugins::load(&app.config);
    if app.config.plugins.manifests.is_empty() {
        println!("no plugin manifests are configured ([plugins] manifests, in the machine config)");
        return Ok(ExitCode::SUCCESS);
    }
    for tool in &loaded.tools {
        let class = loaded
            .classes
            .get(tool.name())
            .map(|class| format!("{class:?}"));
        println!(
            "{}  {}  ({})",
            tool.name(),
            class.unwrap_or_else(|| "unknown".to_owned()),
            tool.description()
        );
    }
    for report in &loaded.reports {
        println!("not loaded: {report}");
    }
    Ok(ExitCode::SUCCESS)
}
