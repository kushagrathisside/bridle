//! `agent bench` (M10, R-39, R-40, R-57.10): runs a task corpus under one condition, and reports the results.
//!
//! A run copies each task's fixture into a fresh folder, writes the condition's config there, wires OpenCode to the
//! harness with `agent init`, starts OpenCode with the task prompt, and checks the result with the task's `verify.py`.
//! The verify script lives in the corpus, outside the folder the model can edit. The metrics come from the harness's own
//! logs in that folder. Each task and each condition gets its own folder, so no state is shared between them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

/// One task of the corpus: `tasks/<id>/task.toml`, a `fixture/` folder and a `verify.py`.
#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub category: String,
    pub prompt: String,
}

/// OpenCode's start-up network steps, switched off for every benchmark run. Without them a start can wait on the
/// network (the model catalog, the update check, the default plugins, the language-server download) and never reach the
/// model. Each name is one the OpenCode 1.19.3 binary reads. The runs are then local-only, which matches the egress audit.
const OPENCODE_STARTUP_SWITCHES: [(&str, &str); 4] = [
    ("OPENCODE_DISABLE_MODELS_FETCH", "1"),
    ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
    ("OPENCODE_DISABLE_DEFAULT_PLUGINS", "1"),
    ("OPENCODE_DISABLE_LSP_DOWNLOAD", "1"),
];
/// The package registry OpenCode's dependency install uses. OpenCode starts that install for the project's `.opencode`
/// folder when the folder holds a plugin, and the start waits for it. The proxy is removed for benchmark runs, so the
/// install reaches the registry directly and hangs. Pointing the registry at a closed loopback port makes the install
/// fail after a bounded wait (about 70 s, measured 2026-10-27), and the start goes on. Nothing leaves the machine.
const BLOCKED_REGISTRY: &str = "http://127.0.0.1:9/";
/// The longest a start may take before the model is reached. Longer means the start stalled.
const STARTUP_WATCHDOG: Duration = Duration::from_secs(300);
/// Starts per task before the task counts as failed.
const MAX_ATTEMPTS: u32 = 3;

/// Added to every prompt, so both conditions get the same instruction. A run has no one to answer a question.
pub const PROTOCOL: &str = "Work without asking questions: finish the task yourself. If a command is refused by the policy, \
use another tool to finish.";

/// The outcome of one task under one condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub condition: String,
    pub task: String,
    pub category: String,
    pub success: bool,
    pub verify_exit: Option<i32>,
    pub opencode_exit: Option<i32>,
    pub timed_out: bool,
    pub wall_s: f64,
    pub tool_calls: u64,
    pub blocked_calls: u64,
    pub loop_refusals: u64,
    pub context_bytes_before: u64,
    pub context_bytes_after: u64,
    /// How many times OpenCode was started for this task: more than one means a start stalled before the model.
    #[serde(default)]
    pub attempts: u32,
}

/// The counts read from the harness's logs in one run folder.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LogCounts {
    pub tool_calls: u64,
    pub blocked_calls: u64,
    pub loop_refusals: u64,
    pub context_bytes_before: u64,
    pub context_bytes_after: u64,
}

/// The totals and means of one condition.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub condition: String,
    pub runs: usize,
    pub successes: usize,
    pub timeouts: usize,
    pub mean_wall_s: f64,
    pub mean_tool_calls: f64,
    pub mean_blocked_calls: f64,
    pub mean_context_bytes_before: f64,
    pub mean_context_bytes_after: f64,
}

#[derive(Debug, Subcommand)]
pub enum BenchAction {
    /// Run every task of a corpus under one condition.
    Run {
        /// The condition: a complete config file (evals/bench/conditions/*.toml). Its file name is the label.
        #[arg(long)]
        condition: PathBuf,
        /// The corpus folder.
        #[arg(long, default_value = "evals/tasks")]
        tasks: PathBuf,
        /// The OpenCode executable.
        #[arg(long)]
        opencode: PathBuf,
        /// The folder for the runs and `results.jsonl`. It must not exist yet.
        #[arg(long)]
        out: PathBuf,
        /// Seconds allowed for one task.
        #[arg(long, default_value_t = 900)]
        timeout_s: u64,
        /// Run only these task ids.
        #[arg(long = "only")]
        only: Vec<String>,
    },
    /// Summarise the result folders given, one condition per folder.
    Report {
        /// Folders written by `bench run`.
        #[arg(required = true)]
        folders: Vec<PathBuf>,
    },
}

/// Runs or reports a benchmark. It does not need a repository config: each run has its own.
pub fn run(action: &BenchAction) -> Result<ExitCode> {
    match action {
        BenchAction::Run {
            condition,
            tasks,
            opencode,
            out,
            timeout_s,
            only,
        } => run_corpus(condition, tasks, opencode, out, *timeout_s, only),
        BenchAction::Report { folders } => report(folders),
    }
}

fn run_corpus(
    condition: &Path,
    tasks_dir: &Path,
    opencode: &Path,
    out: &Path,
    timeout_s: u64,
    only: &[String],
) -> Result<ExitCode> {
    if out.exists() {
        bail!(
            "{} already exists; a run never reuses a folder",
            out.display()
        );
    }
    let label = condition
        .file_stem()
        .and_then(|s| s.to_str())
        .context("the condition file needs a name")?
        .to_owned();
    let config = fs::read_to_string(condition)
        .with_context(|| format!("cannot read {}", condition.display()))?;
    // The verify script runs in the run folder, so the corpus path must be absolute.
    let tasks_dir = std::path::absolute(tasks_dir)
        .with_context(|| format!("cannot resolve {}", tasks_dir.display()))?;
    let tasks = load_tasks(&tasks_dir)?;
    let selected: Vec<&(Task, PathBuf)> = tasks
        .iter()
        .filter(|(task, _)| only.is_empty() || only.contains(&task.id))
        .collect();
    if selected.is_empty() {
        bail!("no task matches the selection in {}", tasks_dir.display());
    }
    fs::create_dir_all(out)?;
    let agent = std::env::current_exe().context("cannot find the agent executable")?;
    for (task, dir) in selected {
        let result = run_task(
            &RunPlan {
                label: &label,
                config: &config,
                opencode,
                agent: &agent,
                out,
                timeout: Duration::from_secs(timeout_s),
            },
            task,
            dir,
        )?;
        println!(
            "{label} {} ({}): {} in {:.0}s (tool calls {}, blocked {}, context {} -> {} bytes)",
            task.id,
            task.title,
            if result.success { "passed" } else { "failed" },
            result.wall_s,
            result.tool_calls,
            result.blocked_calls,
            result.context_bytes_before,
            result.context_bytes_after,
        );
        append_result(out, &result)?;
    }
    Ok(ExitCode::SUCCESS)
}

/// What one run needs besides its task.
struct RunPlan<'a> {
    label: &'a str,
    config: &'a str,
    opencode: &'a Path,
    agent: &'a Path,
    out: &'a Path,
    timeout: Duration,
}

fn run_task(plan: &RunPlan<'_>, task: &Task, task_dir: &Path) -> Result<RunResult> {
    let run_dir = plan.out.join(plan.label).join(&task.id);
    copy_tree(&task_dir.join("fixture"), &run_dir)?;
    fs::write(run_dir.join("local-agent.toml"), plan.config)?;
    // `agent init` wires OpenCode to the harness; it never overwrites the config written above.
    let init = Command::new(plan.agent)
        .args(["init", "--workspace"])
        .arg(&run_dir)
        .status()
        .context("cannot run agent init")?;
    if !init.success() {
        bail!("agent init failed in {}", run_dir.display());
    }
    git(&run_dir, &["init", "-q"])?;
    git(&run_dir, &["add", "-A"])?;
    git(
        &run_dir,
        &[
            "-c",
            "user.name=bench",
            "-c",
            "user.email=bench@invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
    )?;

    let started = Instant::now();
    let mut attempts = 0;
    let (opencode_exit, timed_out) = loop {
        attempts += 1;
        let mut child = start_opencode(plan, task, &run_dir)?;
        match supervise(&mut child, plan.timeout, &run_dir)? {
            Supervised::Exited(code) => break (code, false),
            Supervised::TimedOut => break (None, true),
            Supervised::Stalled if attempts < MAX_ATTEMPTS => {
                // OpenCode never reached the model. Start it again on a clean state folder.
                let _ = fs::remove_dir_all(run_dir.join(".local-agent"));
            }
            Supervised::Stalled => break (None, false),
        }
    };
    let wall_s = started.elapsed().as_secs_f64();

    let verify_exit = Command::new("python")
        .arg(task_dir.join("verify.py"))
        .current_dir(&run_dir)
        .status()
        .ok()
        .and_then(|s| s.code());
    let counts = read_counts(&run_dir.join(".local-agent"));
    Ok(RunResult {
        condition: plan.label.to_owned(),
        task: task.id.clone(),
        category: task.category.clone(),
        success: verify_exit == Some(0),
        verify_exit,
        opencode_exit,
        timed_out,
        wall_s,
        tool_calls: counts.tool_calls,
        blocked_calls: counts.blocked_calls,
        loop_refusals: counts.loop_refusals,
        context_bytes_before: counts.context_bytes_before,
        context_bytes_after: counts.context_bytes_after,
        attempts,
    })
}

/// Starts OpenCode on the task in its run folder. Its output goes to `opencode.log`, appended across attempts.
fn start_opencode(plan: &RunPlan<'_>, task: &Task, run_dir: &Path) -> Result<Child> {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join("opencode.log"))?;
    Command::new(plan.opencode)
        .arg("run")
        .arg(format!("{} {PROTOCOL}", task.prompt))
        .current_dir(run_dir)
        .envs(OPENCODE_STARTUP_SWITCHES)
        .env("NPM_CONFIG_REGISTRY", BLOCKED_REGISTRY)
        .env("BUN_CONFIG_REGISTRY", BLOCKED_REGISTRY)
        // Each run gets its own session database, so runs share no state and stay out of the user's history.
        .env("OPENCODE_DB", run_dir.join("opencode.db"))
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("cannot start OpenCode")
}

/// How one attempt ended.
enum Supervised {
    Exited(Option<i32>),
    TimedOut,
    /// No model request had reached the harness after the startup watchdog: OpenCode stalled before the model.
    Stalled,
}

/// Waits for an attempt. The harness writes `events.jsonl` on the first model request, so its absence after the
/// watchdog means the start stalled.
fn supervise(child: &mut Child, timeout: Duration, run_dir: &Path) -> Result<Supervised> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Supervised::Exited(status.code()));
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Supervised::TimedOut);
        }
        if elapsed >= STARTUP_WATCHDOG
            && !run_dir.join(".local-agent").join("events.jsonl").exists()
        {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Supervised::Stalled);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .context("cannot run git")?;
    if !status.success() {
        bail!("git {args:?} failed in {}", dir.display());
    }
    Ok(())
}

/// Reads the corpus: each folder with a `task.toml`, sorted by id.
fn load_tasks(dir: &Path) -> Result<Vec<(Task, PathBuf)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let path = entry?.path();
        let toml = path.join("task.toml");
        if !toml.is_file() {
            continue;
        }
        let text = fs::read_to_string(&toml)?;
        let task: Task = toml::from_str(&text)
            .with_context(|| format!("{} is not a valid task", toml.display()))?;
        if !path.join("verify.py").is_file() || !path.join("fixture").is_dir() {
            bail!("{} needs a verify.py and a fixture/ folder", path.display());
        }
        out.push((task, path));
    }
    out.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    Ok(out)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// The counts in the harness's own logs: `tool-events.jsonl` for the calls, `events.jsonl` for the context records.
pub fn read_counts(state_dir: &Path) -> LogCounts {
    let mut counts = LogCounts::default();
    if let Ok(text) = fs::read_to_string(state_dir.join("tool-events.jsonl")) {
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if value["kind"] == "tool_before" {
                counts.tool_calls += 1;
                if value["status"] == "blocked" {
                    counts.blocked_calls += 1;
                }
                if value["data"]["rule"] == "loop_rule" {
                    counts.loop_refusals += 1;
                }
            }
        }
    }
    if let Ok(text) = fs::read_to_string(state_dir.join("events.jsonl")) {
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if value["kind"] == "context" {
                counts.context_bytes_before += value["data"]["bytes_before"].as_u64().unwrap_or(0);
                counts.context_bytes_after += value["data"]["bytes_after"].as_u64().unwrap_or(0);
            }
        }
    }
    counts
}

fn append_result(out: &Path, result: &RunResult) -> Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out.join("results.jsonl"))?;
    writeln!(file, "{}", serde_json::to_string(result)?)?;
    Ok(())
}

/// The means and totals of one condition's results.
pub fn summarise(condition: &str, rows: &[RunResult]) -> Summary {
    let n = rows.len().max(1) as f64;
    let mean = |f: fn(&RunResult) -> f64| rows.iter().map(f).sum::<f64>() / n;
    Summary {
        condition: condition.to_owned(),
        runs: rows.len(),
        successes: rows.iter().filter(|r| r.success).count(),
        timeouts: rows.iter().filter(|r| r.timed_out).count(),
        mean_wall_s: mean(|r| r.wall_s),
        mean_tool_calls: mean(|r| r.tool_calls as f64),
        mean_blocked_calls: mean(|r| r.blocked_calls as f64),
        mean_context_bytes_before: mean(|r| r.context_bytes_before as f64),
        mean_context_bytes_after: mean(|r| r.context_bytes_after as f64),
    }
}

/// `agent bench report`: one summary table per condition, one row per task, and the caveat about sample size.
fn report(folders: &[PathBuf]) -> Result<ExitCode> {
    let mut all: Vec<RunResult> = Vec::new();
    for folder in folders {
        let text = fs::read_to_string(folder.join("results.jsonl"))
            .with_context(|| format!("no results.jsonl in {}", folder.display()))?;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            all.push(serde_json::from_str(line)?);
        }
    }
    let mut labels: Vec<String> = all.iter().map(|r| r.condition.clone()).collect();
    labels.sort();
    labels.dedup();
    let markdown = render_markdown(&all, &labels);
    let target = folders
        .first()
        .map(|f| f.join("report.md"))
        .context("no folder")?;
    fs::write(&target, &markdown)?;
    print!("{markdown}");
    println!("written: {}", target.display());
    Ok(ExitCode::SUCCESS)
}

/// The report as Markdown. The caveat is part of the output, not a footnote.
pub fn render_markdown(rows: &[RunResult], labels: &[String]) -> String {
    let mut out = String::from("# Benchmark report\n\n");
    out.push_str("| Condition | Runs | Passed | Timeouts | Mean time (s) | Mean tool calls | Mean blocked | Mean context bytes before -> after |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for label in labels {
        let subset: Vec<RunResult> = rows
            .iter()
            .filter(|r| &r.condition == label)
            .cloned()
            .collect();
        let s = summarise(label, &subset);
        out.push_str(&format!(
            "| {} | {} | {} | {} | {:.0} | {:.1} | {:.1} | {:.0} -> {:.0} |\n",
            s.condition,
            s.runs,
            s.successes,
            s.timeouts,
            s.mean_wall_s,
            s.mean_tool_calls,
            s.mean_blocked_calls,
            s.mean_context_bytes_before,
            s.mean_context_bytes_after,
        ));
    }
    out.push_str("\n## Per task\n\n| Task | Category | Condition | Passed | Time (s) | Tool calls | Blocked |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    let mut sorted = rows.to_vec();
    sorted.sort_by(|a, b| {
        (a.task.as_str(), a.condition.as_str()).cmp(&(b.task.as_str(), b.condition.as_str()))
    });
    for r in &sorted {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {:.0} | {} | {} |\n",
            r.task,
            r.category,
            r.condition,
            if r.success { "yes" } else { "no" },
            r.wall_s,
            r.tool_calls,
            r.blocked_calls,
        ));
    }
    out.push_str(
        "\nThe sample is small: a few tasks, one run each. A difference in these numbers is a lead for the next run, not a \
         measured effect. Tokens, VRAM and the first-pass rate are not measured by this run.\n",
    );
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn row(condition: &str, task: &str, success: bool, wall: f64, tools: u64) -> RunResult {
        RunResult {
            condition: condition.to_owned(),
            task: task.to_owned(),
            category: "debugging".to_owned(),
            success,
            verify_exit: Some(if success { 0 } else { 1 }),
            opencode_exit: Some(0),
            timed_out: false,
            wall_s: wall,
            tool_calls: tools,
            blocked_calls: 0,
            loop_refusals: 0,
            context_bytes_before: 1000,
            context_bytes_after: 400,
            attempts: 1,
        }
    }

    #[test]
    fn a_summary_counts_passes_and_takes_means() {
        let rows = vec![
            row("a", "t1", true, 10.0, 4),
            row("a", "t2", false, 20.0, 8),
        ];
        let s = summarise("a", &rows);
        assert_eq!((s.runs, s.successes), (2, 1));
        assert!((s.mean_wall_s - 15.0).abs() < 1e-9);
        assert!((s.mean_tool_calls - 6.0).abs() < 1e-9);
    }

    #[test]
    fn the_log_counts_calls_refusals_and_context_bytes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("tool-events.jsonl"),
            concat!(
                r#"{"kind":"tool_before","status":"allowed"}"#,
                "\n",
                r#"{"kind":"tool_before","status":"blocked","data":{"rule":"loop_rule"}}"#,
                "\n",
                r#"{"kind":"tool_after","status":"completed"}"#,
                "\n",
            ),
        )
        .unwrap();
        fs::write(
            dir.path().join("events.jsonl"),
            concat!(
                r#"{"kind":"context","data":{"bytes_before":5000,"bytes_after":900}}"#,
                "\n",
                r#"{"kind":"context","data":{"bytes_before":300,"bytes_after":300}}"#,
                "\n",
            ),
        )
        .unwrap();
        let counts = read_counts(dir.path());
        assert_eq!(counts.tool_calls, 2);
        assert_eq!(counts.blocked_calls, 1);
        assert_eq!(counts.loop_refusals, 1);
        assert_eq!(
            (counts.context_bytes_before, counts.context_bytes_after),
            (5300, 1200)
        );
    }

    #[test]
    fn a_missing_log_counts_as_zero() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_counts(dir.path()), LogCounts::default());
    }

    #[test]
    fn the_report_carries_its_caveat_and_one_row_per_task() {
        let rows = vec![
            row("a", "t1", true, 10.0, 4),
            row("b", "t1", false, 30.0, 9),
        ];
        let md = render_markdown(&rows, &["a".to_owned(), "b".to_owned()]);
        assert!(md.contains("| a | 1 | 1 | 0 |"), "{md}");
        assert!(md.contains("| t1 | debugging | a | yes |"), "{md}");
        assert!(md.contains("The sample is small"), "{md}");
    }

    #[test]
    fn the_blocked_registry_is_loopback_so_nothing_leaves_the_machine() {
        assert!(
            BLOCKED_REGISTRY.starts_with("http://127.0.0.1:"),
            "{BLOCKED_REGISTRY}"
        );
    }
}
