//! `bridle fleet`: the small-model fleet and its scheduler for a person at the terminal (plan F1 and F0.2, ADR-0022).
//!
//! A fleet is a few small models that run **together** on the GPU while the big one is unloaded, doing bounded work in parallel: a
//! `worker` (one server with several slots, so one copy of the weights serves several tasks at once) and, optionally, a `manager`. The
//! scheduler (`harness_runtime::scheduler`) decides what to stop and what to start; this command does it with the same code as
//! `bridle models serve` and `stop`, so the video-memory check and the pid-reuse safeguards apply. **A write-ahead record of what was stopped
//! is written before anything is stopped**, so a crash between "stopped the big model" and "started it again" is put right by
//! `bridle fleet recover` (and `down` finishes the same restore).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use harness_core::security::redact::Redactor;
use harness_core::subagent::{Limits, Role};
use harness_runtime::providers::provider_for;
use harness_runtime::scheduler::{self, Budget, Phase, Plan, Swap};
use harness_runtime::subagent::{Request, answer, gather_inputs};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::commands::models::{ServerState, serve, server_state, stop_quiet};

#[derive(Debug, Subcommand)]
pub enum FleetAction {
    /// Show the fleet's slots, whether each runs, what they need in video memory, and any unfinished swap.
    Status,
    /// Stop the GPU slots that are not in the fleet (recording them first), start the fleet's slots.
    Up {
        #[arg(long, default_value_t = 300)]
        wait_secs: u64,
    },
    /// Stop the fleet's slots and start again what `up` stopped.
    Down {
        #[arg(long, default_value_t = 300)]
        wait_secs: u64,
    },
    /// Finish a swap that a crash cut short: stop the fleet and start again what was stopped.
    Recover {
        #[arg(long, default_value_t = 300)]
        wait_secs: u64,
    },
    /// Bring the fleet up, send a file of tasks to the worker several at a time, and put things back.
    Run {
        /// A JSON file: a list of `{ "id", "role" ("explore" or "summarise"), "task", "paths": [...], "text" }`.
        tasks: PathBuf,
        /// Tasks at once (default `fleet.parallel`).
        #[arg(long)]
        parallel: Option<usize>,
        /// Write the results as JSON to this file.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Leave the fleet up afterwards instead of putting the big model back.
        #[arg(long)]
        keep_up: bool,
        #[arg(long, default_value_t = 300)]
        wait_secs: u64,
    },
}

pub fn run(app: &App, action: &FleetAction) -> Result<ExitCode> {
    app.require_valid_config()?;
    if !app.config.fleet.enabled {
        bail!(
            "the fleet is off: set [fleet] enabled = true (and its slots) in the machine configuration"
        );
    }
    match action {
        FleetAction::Status => status(app),
        FleetAction::Up { wait_secs } => up(app, *wait_secs).map(|_| ExitCode::SUCCESS),
        FleetAction::Down { wait_secs } | FleetAction::Recover { wait_secs } => {
            down(app, *wait_secs).map(|_| ExitCode::SUCCESS)
        }
        FleetAction::Run {
            tasks,
            parallel,
            out,
            keep_up,
            wait_secs,
        } => run_tasks(app, tasks, *parallel, out.as_deref(), *keep_up, *wait_secs),
    }
}

/// The GPU slots whose servers Bridle started and that run now.
fn running_gpu_slots(app: &App) -> Vec<String> {
    app.config
        .models
        .iter()
        .filter(|(_, m)| m.hardware == harness_core::config::Hardware::Gpu)
        .filter(|(slot, _)| matches!(server_state(app, slot), ServerState::Running { .. }))
        .map(|(slot, _)| slot.clone())
        .collect()
}

/// What each slot would need on the GPU, from its model file's header; `None` when it cannot be read.
fn estimate_mib(app: &App, slot: &str) -> Option<u64> {
    let model = app.config.models.get(slot)?;
    let launch = model.launch.as_ref()?;
    let file = app.config.resolve_model_file(slot)?;
    let projector = app.config.resolve_mmproj_file(slot);
    harness_runtime::vram::estimate(
        &file,
        projector.as_deref(),
        launch.mmproj_offload,
        model.context,
        model.kv_cache.as_deref(),
    )
    .ok()
    .map(|e| e.total_mib())
}

fn status(app: &App) -> Result<ExitCode> {
    let fleet = &app.config.fleet;
    println!(
        "fleet: manager `{}`, worker `{}`, slots {:?}, {} task(s) at once, swap budget {}",
        fleet.manager, fleet.worker, fleet.slots, fleet.parallel, fleet.swap_budget
    );
    let mut estimates = Vec::new();
    for slot in &fleet.slots {
        let state = match server_state(app, slot) {
            ServerState::Running { pid } => format!("running (pid {pid})"),
            other => format!("{other:?}").to_lowercase(),
        };
        let parallel = app
            .config
            .models
            .get(slot)
            .and_then(|m| m.launch.as_ref())
            .map_or(0, |l| l.parallel);
        match estimate_mib(app, slot) {
            Some(mib) => {
                println!(
                    "  {slot}: {state}; {parallel} server slot(s); needs about {mib} MiB on the GPU"
                );
                estimates.push((slot.clone(), mib));
            }
            None => {
                println!("  {slot}: {state}; {parallel} server slot(s); video memory not estimated")
            }
        }
    }
    let (gpus, _) = harness_runtime::sysprobe::probe_nvidia();
    if let (Some(gpu), false) = (gpus.first(), estimates.is_empty()) {
        match scheduler::fit(
            &estimates,
            gpu.vram_total_mib,
            230,
            harness_runtime::vram::RESERVE_MIB,
        ) {
            Ok(left) => println!(
                "  together they fit in {} MiB with about {left} MiB to spare (a desktop of 230 MiB and a reserve of {} MiB counted)",
                gpu.vram_total_mib,
                harness_runtime::vram::RESERVE_MIB
            ),
            Err(why) => println!("  they do NOT fit together: {why}"),
        }
    }
    println!("running GPU slots now: {:?}", running_gpu_slots(app));
    match Swap::pending(&app.workspace) {
        Some(swap) => println!(
            "an unfinished swap is recorded ({:?}): the fleet is up and {:?} were stopped; `bridle fleet down` puts them back",
            swap.phase, swap.stopped
        ),
        None => println!("no swap is pending"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Starts a slot, retrying while the video memory of a model that was just stopped is still being released.
fn start_with_retry(app: &App, slot: &str, wait_secs: u64) -> Result<()> {
    let mut last = String::new();
    for attempt in 0..10 {
        match serve(app, slot, true, wait_secs) {
            Ok(code) if code == ExitCode::SUCCESS => return Ok(()),
            Ok(_) => bail!("`{slot}` did not start"),
            Err(error) => {
                last = format!("{error:#}");
                if !last.contains("would not leave enough room") || attempt == 9 {
                    break;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    bail!("{last}")
}

/// Goes from what runs to the fleet. `Ok(true)` if it changed something.
fn up(app: &App, wait_secs: u64) -> Result<bool> {
    let fleet = &app.config.fleet;
    if let Some(swap) = Swap::pending(&app.workspace) {
        bail!(
            "a swap is unfinished (the fleet is up and {:?} were stopped); run `bridle fleet down` to put things back first",
            swap.stopped
        );
    }
    let running = running_gpu_slots(app);
    let Plan { stop, start } = scheduler::plan_up(&running, &fleet.slots);
    if stop.is_empty() && start.is_empty() {
        println!("· the fleet is already up");
        return Ok(false);
    }
    // Before anything is stopped: does the fleet fit at all?
    let estimates: Vec<(String, u64)> = fleet
        .slots
        .iter()
        .filter_map(|s| estimate_mib(app, s).map(|m| (s.clone(), m)))
        .collect();
    if let (Some(gpu), false) = (
        harness_runtime::sysprobe::probe_nvidia().0.first(),
        estimates.is_empty(),
    ) {
        scheduler::fit(
            &estimates,
            gpu.vram_total_mib,
            230,
            harness_runtime::vram::RESERVE_MIB,
        )
        .map_err(|why| anyhow::anyhow!("{why}; nothing was stopped"))?;
    }
    let mut budget = Budget::new(fleet.swap_budget);
    if !stop.is_empty() {
        budget
            .take(&format!("stop {stop:?} for the fleet"))
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    // The record first, then the first process is stopped.
    let swap = Swap::new(&fleet.slots, &stop);
    if !stop.is_empty() {
        swap.write(&app.workspace)
            .context("cannot write the swap record; nothing was stopped")?;
    }
    for slot in &stop {
        println!("· stopping `{slot}` to make room");
        stop_quiet(app, slot)?;
    }
    if !stop.is_empty() {
        std::thread::sleep(Duration::from_secs(3));
    }
    for slot in &start {
        if let Err(error) = start_with_retry(app, slot, wait_secs) {
            eprintln!("bridle: `{slot}` did not start: {error:#}; putting things back");
            let _ = down(app, wait_secs);
            return Err(error);
        }
    }
    println!("· the fleet is up: {:?}", fleet.slots);
    Ok(true)
}

/// Goes from the fleet back to what `up` stopped (from the record), and removes the record when that is done.
fn down(app: &App, wait_secs: u64) -> Result<()> {
    let fleet = &app.config.fleet;
    let record = Swap::pending(&app.workspace);
    let restore: Vec<String> = record
        .as_ref()
        .map(|r| r.stopped.clone())
        .unwrap_or_default();
    if let Some(mut swap) = record.clone() {
        swap.phase = Phase::Restoring;
        swap.write(&app.workspace)
            .context("cannot update the swap record")?;
    }
    let plan = scheduler::plan_down(&running_gpu_slots(app), &fleet.slots, &restore);
    for slot in &plan.stop {
        println!("· stopping `{slot}`");
        stop_quiet(app, slot)?;
    }
    if !plan.stop.is_empty() {
        std::thread::sleep(Duration::from_secs(3));
    }
    for slot in &plan.start {
        println!("· starting `{slot}` again");
        start_with_retry(app, slot, wait_secs).with_context(|| format!("`{slot}` could not be started again; the swap record is kept, run `bridle fleet recover`"))?;
    }
    Swap::finish(&app.workspace);
    if record.is_none() && plan.is_empty() {
        println!("· nothing to put back");
    } else {
        println!("· done: nothing is pending");
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------------------------ run

#[derive(Debug, Deserialize)]
struct TaskSpec {
    #[serde(default)]
    id: Option<String>,
    #[serde(default = "explore")]
    role: String,
    task: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    text: Option<String>,
}

fn explore() -> String {
    "explore".to_owned()
}

#[derive(Debug, Clone, Serialize)]
struct TaskResult {
    id: String,
    ok: bool,
    /// The checked answer, or the reason there is none.
    text: String,
    latency_ms: u64,
}

fn run_tasks(
    app: &App,
    file: &Path,
    parallel: Option<usize>,
    out: Option<&Path>,
    keep_up: bool,
    wait_secs: u64,
) -> Result<ExitCode> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let specs: Vec<TaskSpec> = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a list of tasks", file.display()))?;
    if specs.is_empty() || specs.len() > 200 {
        bail!("give between 1 and 200 tasks");
    }
    let parallel = parallel.unwrap_or(app.config.fleet.parallel).clamp(1, 16);
    let changed = up(app, wait_secs)?;
    let started = Instant::now();
    let results = match app.policy() {
        Ok(policy) => send(&app.config, &app.workspace, &policy, &specs, parallel),
        Err(e) => specs
            .iter()
            .enumerate()
            .map(|(i, s)| failed(i, s, format!("no policy: {e:#}")))
            .collect(),
    };
    let wall = started.elapsed();
    if !keep_up {
        if let Err(error) = down(app, wait_secs) {
            eprintln!("bridle: putting things back failed: {error:#}");
        }
    } else if changed {
        println!("· the fleet is left up; `bridle fleet down` puts the big model back");
    }
    let ok = results.iter().filter(|r| r.ok).count();
    let total_latency: u64 = results.iter().map(|r| r.latency_ms).sum();
    for r in &results {
        println!(
            "{} {} ({} ms)\n{}",
            if r.ok { "✓" } else { "✗" },
            r.id,
            r.latency_ms,
            indent(&r.text)
        );
    }
    println!(
        "{} task(s): {ok} answered, {} failed; {:.1} s wall time for {:.1} s of work, {} at once: about {:.1}x",
        results.len(),
        results.len() - ok,
        wall.as_secs_f64(),
        total_latency as f64 / 1000.0,
        parallel,
        total_latency as f64 / 1000.0 / wall.as_secs_f64().max(0.001)
    );
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&results)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(if ok == results.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sends the tasks to the worker slot, `parallel` at a time. Every task ends as a result, answered or not; none stops the others.
fn send(
    config: &harness_core::config::Config,
    workspace: &Path,
    policy: &harness_core::security::tool_policy::ToolPolicy,
    specs: &[TaskSpec],
    parallel: usize,
) -> Vec<TaskResult> {
    let worker = &config.fleet.worker;
    let results: Mutex<Vec<(usize, TaskResult)>> = Mutex::new(Vec::new());
    let next = Arc::new(AtomicUsize::new(0));
    let redactor = Redactor::new();
    let limits = Limits {
        max_input_bytes: config.subagents.max_input_bytes,
        max_inputs: config.subagents.max_inputs,
    };
    std::thread::scope(|scope| {
        for _ in 0..parallel.min(specs.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(spec) = specs.get(i) else { break };
                    let result = one(
                        config, workspace, worker, spec, i, policy, &redactor, limits,
                    );
                    results
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((i, result));
                }
            });
        }
    });
    let mut all = results.into_inner().unwrap_or_else(PoisonError::into_inner);
    all.sort_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, r)| r).collect()
}

fn failed(i: usize, spec: &TaskSpec, why: String) -> TaskResult {
    TaskResult {
        id: spec.id.clone().unwrap_or_else(|| format!("task-{}", i + 1)),
        ok: false,
        text: why,
        latency_ms: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn one(
    config: &harness_core::config::Config,
    workspace: &Path,
    worker: &str,
    spec: &TaskSpec,
    i: usize,
    policy: &harness_core::security::tool_policy::ToolPolicy,
    redactor: &Redactor,
    limits: Limits,
) -> TaskResult {
    let Some(role) = Role::parse(&spec.role) else {
        return failed(i, spec, "role must be `explore` or `summarise`".to_owned());
    };
    let request = Request {
        role,
        task: spec.task.clone(),
        paths: spec.paths.clone(),
        text: spec.text.clone(),
    };
    let Some(model) = config.models.get(worker) else {
        return failed(i, spec, format!("worker slot `{worker}` is not defined"));
    };
    let provider = match provider_for(model, &|n| std::env::var(n).ok()) {
        Ok(p) => p,
        Err(e) => return failed(i, spec, format!("the worker is unavailable: {e}")),
    };
    let inputs = match gather_inputs(&request, workspace, policy, redactor, limits) {
        Ok(inputs) => inputs,
        Err(e) => return failed(i, spec, e),
    };
    let started = Instant::now();
    match answer(provider.as_ref(), &request, inputs, limits) {
        Ok(outcome) => TaskResult {
            id: spec.id.clone().unwrap_or_else(|| format!("task-{}", i + 1)),
            ok: true,
            text: redactor.redact(&outcome.rendered).into_owned(),
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        },
        Err(e) => TaskResult {
            id: spec.id.clone().unwrap_or_else(|| format!("task-{}", i + 1)),
            ok: false,
            text: e,
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_task_file_reads_with_defaults_and_rejects_nonsense() {
        let specs: Vec<TaskSpec> = serde_json::from_str(r#"[{"task": "what does a.rs do?", "paths": ["a.rs"]}, {"id": "s", "role": "summarise", "task": "shorten", "text": "long text"}]"#).unwrap();
        assert_eq!(specs[0].role, "explore");
        assert_eq!(specs[0].id, None);
        assert_eq!(specs[1].id.as_deref(), Some("s"));
        assert!(
            serde_json::from_str::<Vec<TaskSpec>>(r#"[{"role": "explore"}]"#).is_err(),
            "a task needs its text"
        );
        assert!(serde_json::from_str::<Vec<TaskSpec>>("{}").is_err());
    }

    #[test]
    fn every_task_ends_as_a_result_and_a_bad_one_does_not_stop_the_others() {
        // A config with no worker defined: each task fails on its own with a reason, in order, and none panics.
        let dir = tempfile::tempdir().unwrap();
        let config = harness_core::config::Config::default();
        let policy =
            harness_core::security::tool_policy::ToolPolicy::new(dir.path(), &config.security)
                .unwrap();
        let specs = vec![
            TaskSpec {
                id: Some("a".into()),
                role: "explore".into(),
                task: "q".into(),
                paths: vec![],
                text: None,
            },
            TaskSpec {
                id: None,
                role: "nonsense".into(),
                task: "q".into(),
                paths: vec![],
                text: None,
            },
        ];
        let results = send(&config, dir.path(), &policy, &specs, 4);
        assert_eq!(results.len(), 2);
        assert_eq!((results[0].id.as_str(), results[0].ok), ("a", false));
        assert!(
            results[1].text.contains("role must be"),
            "{}",
            results[1].text
        );
    }
}
