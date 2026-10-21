//! `agent models serve|manifest|list`. IMPLEMENTATION SPEC: see
//! crates/harness-cli/DOCTOR_HEALTH_MODELS_SPEC.md.

use std::process::ExitCode;

use clap::Subcommand;

use crate::app::App;

#[derive(Debug, Subcommand)]
pub enum ModelsAction {
    /// List configured model slots.
    List,
    /// Start the local inference server for a slot.
    Serve {
        slot: String,
        /// Return once the server is healthy, leaving it running in the background.
        #[arg(long)]
        detach: bool,
        /// Seconds to wait for the server to become healthy.
        #[arg(long, default_value_t = 300)]
        wait_secs: u64,
    },
    /// Stop a server started with `serve --detach`.
    Stop { slot: String },
    /// Record model manifests (hash, revision, runtime build, hardware) under .local-agent/manifests.
    Manifest {
        /// Slot (default: every slot with a launch section).
        slot: Option<String>,
        /// Skip hashing (manifests then carry no sha256).
        #[arg(long)]
        no_hash: bool,
    },
}

pub fn run(app: &App, action: &ModelsAction) -> anyhow::Result<ExitCode> {
    let _ = (app, action);
    todo!("wave 2")
}
