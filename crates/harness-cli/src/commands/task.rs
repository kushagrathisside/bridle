//! `agent task`, `agent explain` and `agent trace` (M3 step 8, R-54.2, R-54.3).
//!
//! Output is plain text, one fact per line. Anything that came from the model or the host passes through
//! [`clean`], so a control character cannot rewrite the terminal.

use std::process::ExitCode;

use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use harness_core::journal::{Journal, Record};
use harness_runtime::state::{INDEX_FILE, SessionIndex, StateDir};

use crate::app::App;

/// The most decisions or entries a single command prints.
const MAX_LIMIT: usize = 1000;

#[derive(Debug, Subcommand)]
pub enum TaskAction {
    /// Start a task. The next new OpenCode session works on it.
    Start {
        /// The task's objective. Stored redacted.
        #[arg(long, default_value = "")]
        prompt: String,
        /// Run the task in its own Git worktree under `.worktrees/` (R-26.1).
        #[arg(long)]
        worktree: bool,
    },
    /// Show the state of a task (default: the latest task).
    Status {
        task: Option<String>,
        /// Print the state as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Make the next new OpenCode session work on an existing task.
    Resume { task: String },
    /// End a block and start the loop counters again.
    Reset { task: Option<String> },
    /// Rebuild the task caches from their journals.
    Repair,
    /// Remove the directories of finished tasks and their session bindings.
    Prune,
}

pub fn run(app: &App, action: &TaskAction) -> Result<ExitCode> {
    let state = StateDir::open(&app.workspace)?;
    match action {
        TaskAction::Start { prompt, worktree } => {
            if *worktree {
                start_in_worktree(app, &state, prompt)?;
            } else {
                let id = state.start_task(&app.workspace.display().to_string(), prompt)?;
                println!("started {id}; the next new OpenCode session works on it");
            }
        }
        TaskAction::Status { task, json } => {
            let id = resolve(&state, task.as_deref())?;
            let journal = task_state(app, &id)?.task(&id)?.snapshot()?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&journal.state)?);
            } else {
                print_status(&journal);
            }
        }
        TaskAction::Resume { task } => {
            task_state(app, task)?.resume_task(task)?;
            println!("the next new OpenCode session works on {task}");
        }
        TaskAction::Reset { task } => {
            let id = resolve(&state, task.as_deref())?;
            task_state(app, &id)?.task(&id)?.append(Record::Reset {
                reason: "agent task reset".to_owned(),
            })?;
            println!("reset {id}: the loop counters start again");
        }
        TaskAction::Repair => {
            let ids = state.repair_tasks()?;
            println!("rebuilt the cache of {} task(s)", ids.len());
        }
        TaskAction::Prune => {
            let removed = state.prune_finished()?;
            println!(
                "removed {} finished task(s): {}",
                removed.len(),
                removed.join(", ")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `agent explain`: the most recent decisions of a task, with the rule and the signals behind each.
pub fn explain(app: &App, task: Option<&str>, limit: usize) -> Result<ExitCode> {
    let state = StateDir::open(&app.workspace)?;
    let id = resolve(&state, task)?;
    let decisions = task_state(app, &id)?.task_decisions(&id)?;
    let shown = &decisions[decisions.len().saturating_sub(limit.min(MAX_LIMIT))..];
    println!(
        "{id}: {} decision(s) recorded, showing {}",
        decisions.len(),
        shown.len()
    );
    for record in shown {
        let signals: Vec<String> = record
            .signals
            .iter()
            .map(|s| clean(&format!("{s:?}")))
            .collect();
        let confidence = record
            .result
            .confidence
            .map_or("none".to_owned(), |c| format!("{c:.2}"));
        println!(
            "call {}: {} because {} (confidence {}, provider {}){}",
            clean(&record.call_id),
            clean(&format!("{:?}", record.result.decision)),
            clean(&format!("{:?}", record.result.reason_code)),
            confidence,
            clean(&record.result.provider),
            if record.refused { ", refused" } else { "" },
        );
        if !signals.is_empty() {
            println!("  signals: {}", signals.join("; "));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `agent trace`: the ordered record of a task, one line per journal entry.
pub fn trace(app: &App, task: Option<&str>, limit: usize) -> Result<ExitCode> {
    let state = StateDir::open(&app.workspace)?;
    let id = resolve(&state, task)?;
    let entries = task_state(app, &id)?.task(&id)?.entries()?;
    let shown = &entries[entries.len().saturating_sub(limit.min(MAX_LIMIT))..];
    println!("{id}: {} record(s), showing {}", entries.len(), shown.len());
    for entry in shown {
        println!("{:>6}  {}", entry.seq, describe(&entry.record));
    }
    Ok(ExitCode::SUCCESS)
}

/// The task id to act on: the one given, or the latest task.
fn resolve(state: &StateDir, task: Option<&str>) -> Result<String> {
    if let Some(id) = task {
        state.task(id)?;
        return Ok(id.to_owned());
    }
    let index: SessionIndex = state.read_json(INDEX_FILE)?.unwrap_or_default();
    if index.next_task == 0 {
        bail!("no task exists yet; start one with `agent task start`");
    }
    Ok(format!("task-{:04}", index.next_task))
}

/// The state directory that holds a task: the repository's own, or the worktree's for a worktree task.
fn task_state(app: &App, id: &str) -> Result<StateDir> {
    let primary = StateDir::open(&app.workspace)?;
    match primary.worktree_of(id)? {
        Some(relative) => Ok(StateDir::open(&app.workspace.join(relative))?),
        None => Ok(primary),
    }
}

/// `agent task start --worktree`: creates `.worktrees/agent-task-NNN` on a new branch from HEAD, copies the
/// generated config files that git does not track, and starts the task in the worktree's own state.
fn start_in_worktree(app: &App, state: &StateDir, prompt: &str) -> Result<()> {
    let repo = &app.workspace;
    let head = git(repo, &["rev-parse", "HEAD"])?.trim().to_owned();
    let dirty_files = git(repo, &["status", "--porcelain"])?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let id = state.reserve_task_id(None)?;
    let number: u32 = id.trim_start_matches("task-").parse()?;
    let branch = format!("agent-task-{number:03}");
    let relative = format!(".worktrees/{branch}");
    let worktree = repo.join(&relative);
    state.register_worktree(&id, &relative)?;
    if let Err(error) = git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &worktree.display().to_string(),
            &head,
        ],
    ) {
        state.release_task_id(&id)?;
        return Err(error);
    }
    for name in ["local-agent.toml", "opencode.json"] {
        let from = repo.join(name);
        if from.is_file() {
            std::fs::copy(&from, worktree.join(name))
                .with_context(|| format!("copy {name} into the worktree"))?;
        }
    }
    let worktree_state = StateDir::open(&worktree)?;
    worktree_state.start_task_with_id(&id, &worktree.display().to_string(), prompt)?;
    worktree_state.task(&id)?.append(Record::Worktree {
        branch: branch.clone(),
        path: relative.clone(),
        base_commit: head,
        dirty_files: u32::try_from(dirty_files).unwrap_or(u32::MAX),
    })?;
    println!("started {id} in {}", worktree.display());
    println!("open OpenCode in that folder; its first session works on {id}");
    Ok(())
}

/// Runs `git -C <dir> <args>` and returns its standard output.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("cannot run git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn print_status(journal: &Journal) {
    let s = &journal.state;
    println!("task: {}", clean(&s.task_id));
    if let Some(path) = &s.worktree {
        println!(
            "worktree: {} (branch {})",
            clean(path),
            clean(s.branch.as_deref().unwrap_or("?"))
        );
    }
    println!("phase: {}", s.phase.name());
    println!("status: {:?}", s.status);
    println!("iteration: {}", s.iteration);
    println!(
        "last action: {}",
        clean(s.last_action.as_deref().unwrap_or("none"))
    );
    println!(
        "last test: {}",
        clean(s.last_test.as_deref().unwrap_or("none"))
    );
    println!("failures in a row: {}", s.failure_count);
    println!("refusals in a row: {}", s.consecutive_refusals);
    println!("journal records: {}", journal.last_seq);
}

fn describe(record: &Record) -> String {
    match record {
        Record::TaskStarted {
            task_id,
            repository,
            ..
        } => {
            format!(
                "task_started {} repository={}",
                clean(task_id),
                clean(repository)
            )
        }
        Record::CallStarted {
            tool,
            read_only,
            verification,
            edited_file,
            ..
        } => format!(
            "call_started tool={} read_only={read_only} verification={verification}{}",
            clean(tool),
            edited_file
                .as_deref()
                .map_or(String::new(), |file| format!(" edits={}", clean(file))),
        ),
        Record::VerificationResult {
            outcome,
            fingerprints,
            ..
        } => format!(
            "verification outcome={outcome:?} failures={}",
            fingerprints.len()
        ),
        Record::Refused { rule, .. } => format!("refused rule={}", clean(rule)),
        Record::Reset { reason } => format!("reset reason={}", clean(reason)),
        Record::Worktree {
            branch,
            path,
            base_commit,
            dirty_files,
        } => format!(
            "worktree branch={} path={} base={} dirty_files={dirty_files}",
            clean(branch),
            clean(path),
            clean(&base_commit.chars().take(12).collect::<String>())
        ),
    }
}

/// Replaces control characters, including line breaks, with a space.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
