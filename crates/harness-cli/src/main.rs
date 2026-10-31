//! `agent`: the harness command-line interface. Wiring only; logic lives in the library crates
//! and, for the OpenCode integration, in [`host::opencode_v1`].

mod app;
mod chat;
mod commands;
mod host;
mod image;
mod output;
mod prompt;
mod route;
mod tui;
mod workflow;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use harness_core::identity;

#[derive(Debug, Parser)]
#[command(name = identity::CLI_NAME, version, about = "Local-first coding-agent harness")]
struct Cli {
    /// Explicit config file (highest-precedence layer). Also `LOCAL_AGENT_CONFIG`.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Workspace (repository) root. Defaults to the current directory.
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check hardware, runtimes, models, endpoints and the OpenCode integration.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Run functional health checks against model endpoints.
    Health {
        /// Model slot to check (default: every slot).
        slot: Option<String>,
        /// Only liveness, model id and a one-word completion.
        #[arg(long)]
        quick: bool,
        #[arg(long)]
        json: bool,
    },
    /// Inspect the effective configuration.
    Config {
        #[command(subcommand)]
        action: commands::config::ConfigAction,
    },
    /// Wire the harness into a repository: config, OpenCode config, plugin shim, .gitignore.
    Init(commands::init::InitArgs),
    /// Start local model servers and record model manifests.
    Models {
        #[command(subcommand)]
        action: commands::models::ModelsAction,
    },
    /// OpenCode plugin bridge: read a hook payload on stdin, write the verdict on stdout.
    #[command(hide = true)]
    Hook { event: host::opencode_v1::HookEvent },
    /// Serve the harness tools over MCP on stdio.
    Mcp,
    /// Start, resume, inspect, reset, repair and prune tasks.
    Task {
        #[command(subcommand)]
        action: commands::task::TaskAction,
    },
    /// Explain the recent decisions of a task.
    Explain {
        /// Task id (default: the latest task).
        task: Option<String>,
        /// How many decisions to show, at most 1000.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// A/B run of the rules and the micro model on recorded loop cases.
    EvalDecisions {
        /// A JSON file of cases: name, task state and loop signals.
        cases: PathBuf,
        /// The model slot that decides the stalled cases.
        #[arg(long, default_value = "micro")]
        slot: String,
        /// Also write the report to this file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Show which slot a request would go to, and why: the rules first, then the decision model (ADR-0010).
    Route {
        /// The request text.
        text: String,
    },
    /// List every outbound destination of this configuration, and the proxy state. Sends nothing.
    Egress,
    /// List the plugin manifests and their tools, with the class each tool is offered under, and what was not loaded.
    Plugins,
    /// Ask the subagent tier: a small local model reads a few bounded inputs and answers with cited evidence.
    Subagent {
        /// `explore` answers a task about the inputs; `summarise` condenses one text.
        #[arg(long)]
        role: String,
        /// The question or the instruction. At most 500 characters.
        #[arg(long)]
        task: String,
        /// A file to read, relative to the workspace. Repeat for more files.
        #[arg(long = "path")]
        paths: Vec<String>,
        /// A literal text to condense, named `text` in the answer.
        #[arg(long)]
        text: Option<String>,
    },
    /// Run a task corpus under a condition, or summarise the results.
    Bench {
        #[command(subcommand)]
        action: commands::bench::BenchAction,
    },
    /// Open the interactive Control Center (needs a terminal).
    Tui,
    /// Show the ordered record of a task.
    Trace {
        /// Task id (default: the latest task).
        task: Option<String>,
        /// How many records to show, at most 1000.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Hook { event } => {
            // The hook must always answer with JSON, even when the config is broken, so the
            // shim can fail closed with a useful message instead of a crash.
            return commands::hook::run(event, cli.config.as_deref(), cli.workspace.as_deref());
        }
        Command::Init(args) => {
            commands::init::run(&args, cli.config.as_deref(), cli.workspace.as_deref())
        }
        Command::Bench { action } => commands::bench::run(&action),
        // The TUI loads the configuration itself: a broken config is a state to show, not a crash.
        Command::Tui => tui::run(tui::data::LoadArgs {
            config: cli.config.clone(),
            workspace: cli.workspace.clone(),
        }),
        command => match app::App::load(cli.config.as_deref(), cli.workspace.as_deref()) {
            Ok(app) => match command {
                Command::Doctor { json } => commands::doctor::run(&app, json),
                Command::Health { slot, quick, json } => {
                    commands::health::run(&app, slot.as_deref(), quick, json)
                }
                Command::Config { action } => commands::config::run(&app, &action),
                Command::Models { action } => commands::models::run(&app, &action),
                Command::Mcp => commands::mcp::run(&app),
                Command::Task { action } => commands::task::run(&app, &action),
                Command::Explain { task, limit } => {
                    commands::task::explain(&app, task.as_deref(), limit)
                }
                Command::Trace { task, limit } => {
                    commands::task::trace(&app, task.as_deref(), limit)
                }
                Command::Egress => commands::egress::run(&app),
                Command::Subagent {
                    role,
                    task,
                    paths,
                    text,
                } => commands::subagent::run(&app, &role, &task, &paths, text.as_deref()),
                Command::Route { text } => route::run(&app, &text),
                Command::Plugins => commands::plugins::run(&app),
                Command::EvalDecisions { cases, slot, out } => {
                    commands::eval::run(&app, &slot, &cases, out.as_deref())
                }
                Command::Hook { .. } | Command::Init(_) | Command::Bench { .. } | Command::Tui => {
                    unreachable!("handled above")
                }
            },
            Err(e) => Err(e),
        },
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{}: error: {e:#}", identity::CLI_NAME);
            ExitCode::FAILURE
        }
    }
}
