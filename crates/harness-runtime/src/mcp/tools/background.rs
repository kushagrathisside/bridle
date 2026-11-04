//! `command_start`, `command_status` and `command_stop`: a command that runs in the background while the model goes on (ADR-0023).
//!
//! This is Infinity's "slow tools run concurrently, the agent does not wait" idea, kept inside Bridle's rules:
//!
//! * `command_start` takes exactly what `run_command` takes (a bare program name, plain words, a working directory in the workspace)
//!   and judges it with the same function (`run::authorize`) and the same sandbox, so a command the policy would not run is not
//!   started here either. It returns at once with an id such as `bg-1`.
//! * When the command ends, Bridle queues a **notice** (`crate::notices`) that is added to the next tool result. The notice is a
//!   hint; `command_status` is the record: it shows the state, the exit code and the last output (redacted, bounded) at any time.
//! * `command_stop` ends a command that Bridle started (the whole process tree). It is allowed like `browser_close`, so a runaway
//!   command can always be stopped.
//!
//! Limits: three commands at once per workspace, a time limit of at most ten minutes (default five), 64 KiB of output kept (the
//! tail), sixteen finished commands remembered. The commands belong to the tool server of the workspace: when it ends, they are
//! stopped. If Bridle itself is killed without a chance to clean up, a command that is running keeps running until it ends (its own
//! time limit is enforced by Bridle, which is gone then). Each running command therefore has a small record in `.bridle/background/`
//! (the program's name, never its arguments, and the process ids); `bridle doctor --repair` finds the records whose Bridle is gone and
//! stops the commands, after checking that the process id still belongs to that program. The tools are offered only when
//! `[workflow] background_commands = true`.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use harness_core::identity;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::run::{authorize, plain_word};
use super::{
    McpTool, ToolContext, ToolOutput, check_read_path, kill_tree, lossy, optional_string_list,
    redacted_output, tail_text, u64_arg,
};

const MAX_ARGS: usize = 32;
const MAX_RUNNING: usize = 3;
const MAX_KEPT: usize = 16;
const TAIL_BYTES: usize = 64 * 1024;
const SHOW_TAIL_BYTES: usize = 4_000;
const DEFAULT_TIMEOUT_MS: u64 = 300_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_WAIT_MS: u64 = 10_000;
const POLL: Duration = Duration::from_millis(50);

/// The tools for this configuration: none unless background commands are on.
pub fn tools(config: &harness_core::config::Config) -> Vec<Box<dyn McpTool>> {
    if config.workflow.background_commands {
        vec![
            Box::new(CommandStartTool),
            Box::new(CommandStatusTool),
            Box::new(CommandStopTool),
        ]
    } else {
        Vec::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Running,
    Finished {
        status: &'static str,
        exit_code: Option<i32>,
        duration_ms: u64,
    },
}

#[derive(Debug)]
struct Job {
    id: String,
    line: String,
    started: Instant,
    pid: u32,
    state: Mutex<State>,
    tail: Mutex<Vec<u8>>,
    stop: AtomicBool,
}

impl Job {
    fn state(&self) -> State {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn running(&self) -> bool {
        self.state() == State::Running
    }

    fn output(&self) -> String {
        lossy(&self.tail.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// What is written to `.bridle/background/` while a command runs, so a command left behind by a Bridle that was killed can be found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    id: String,
    /// The command's process id.
    pid: u32,
    /// The process id of the Bridle that started it.
    owner_pid: u32,
    /// The program's name only: arguments can hold secrets and are not kept.
    program: String,
    started_unix_s: u64,
}

fn records_dir(workspace: &Path) -> PathBuf {
    identity::state_dir(workspace).join("background")
}

fn record_path(workspace: &Path, id: &str, owner_pid: u32) -> PathBuf {
    records_dir(workspace).join(format!("{id}-{owner_pid}.json"))
}

fn write_record(workspace: &Path, record: &Record) {
    let _ = std::fs::create_dir_all(records_dir(workspace));
    if let Ok(text) = serde_json::to_string(record) {
        let _ = std::fs::write(record_path(workspace, &record.id, record.owner_pid), text);
    }
}

fn remove_record(workspace: &Path, id: &str) {
    let _ = std::fs::remove_file(record_path(workspace, id, std::process::id()));
}

/// Whether process image `image` (such as `ping.exe`) is the program `program` (such as `ping`).
fn same_program(image: &str, program: &str) -> bool {
    let stem = |name: &str| {
        let lower = name.to_ascii_lowercase();
        lower
            .strip_suffix(".exe")
            .map_or(lower.clone(), str::to_owned)
    };
    stem(image) == stem(program)
}

/// Finds the commands left behind by a Bridle that is gone and stops them. A record whose owner is a running Bridle is left alone; a
/// process id that now belongs to another program is not signalled. Returns one line for each record dealt with.
pub fn sweep_orphans(workspace: &Path) -> Vec<String> {
    sweep_with(workspace, &crate::sysprobe::process_image, &|pid| {
        let _ = kill_tree(pid);
    })
}

fn sweep_with(
    workspace: &Path,
    image_of: &dyn Fn(u32) -> Option<String>,
    kill: &dyn Fn(u32),
) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(entries) = std::fs::read_dir(records_dir(workspace)) else {
        return lines;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(record) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Record>(&t).ok())
        else {
            continue;
        };
        if record.owner_pid == std::process::id() {
            continue;
        }
        if image_of(record.owner_pid)
            .is_some_and(|image| image.to_ascii_lowercase().contains("bridle"))
        {
            continue;
        }
        match image_of(record.pid) {
            Some(image) if same_program(&image, &record.program) => {
                kill(record.pid);
                lines.push(format!(
                    "stopped background command {} (`{}`, pid {}): the Bridle that started it is gone",
                    record.id, record.program, record.pid
                ));
            }
            Some(image) => lines.push(format!(
                "background command {}: pid {} now belongs to `{image}`; removed the record without signalling it",
                record.id, record.pid
            )),
            None => lines.push(format!("background command {} (`{}`) was already gone; removed the record", record.id, record.program)),
        }
        let _ = std::fs::remove_file(&path);
    }
    lines
}

static JOBS: Mutex<Option<HashMap<PathBuf, Vec<Arc<Job>>>>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn with_jobs<T>(f: impl FnOnce(&mut HashMap<PathBuf, Vec<Arc<Job>>>) -> T) -> T {
    let mut slot = JOBS.lock().unwrap_or_else(PoisonError::into_inner);
    f(slot.get_or_insert_with(HashMap::new))
}

fn jobs_of(workspace: &Path) -> Vec<Arc<Job>> {
    with_jobs(|all| all.get(workspace).cloned().unwrap_or_default())
}

fn find(workspace: &Path, id: &str) -> Option<Arc<Job>> {
    jobs_of(workspace).into_iter().find(|job| job.id == id)
}

/// Waits until the job is no longer running or `limit` has passed. Returns whether it finished.
fn wait_finished(job: &Job, limit: Duration) -> bool {
    let end = Instant::now().checked_add(limit);
    loop {
        if !job.running() {
            return true;
        }
        if end.is_some_and(|end| Instant::now() >= end) {
            return false;
        }
        thread::sleep(POLL);
    }
}

/// Stops every command of `workspace` that is still running and waits briefly for them to end. Called when the tool server ends.
pub fn stop_all(workspace: &Path) {
    let jobs = jobs_of(workspace);
    for job in jobs.iter().filter(|job| job.running()) {
        job.stop.store(true, Ordering::SeqCst);
    }
    for job in &jobs {
        wait_finished(job, Duration::from_secs(3));
    }
}

/// Copies a pipe into the job's tail buffer until it closes.
fn pump<R: Read>(mut pipe: R, job: &Job) {
    let mut chunk = [0_u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut tail = job.tail.lock().unwrap_or_else(PoisonError::into_inner);
                tail.extend_from_slice(&chunk[..n]);
                if tail.len() > TAIL_BYTES {
                    let cut = tail.len() - TAIL_BYTES;
                    tail.drain(..cut);
                }
            }
        }
    }
}

fn blocked(ctx: &ToolContext, line: &str, rule: &str, reason: &str) -> ToolOutput {
    let text = ctx.redactor.redact(reason).into_owned();
    ToolOutput {
        text: text.clone(),
        structured: Some(json!({
            "status": "blocked",
            "command": ctx.redactor.redact(line).into_owned(),
            "rule": rule,
            "reason": text,
        })),
        is_error: false,
    }
}

/// `command_start`: run a program in the background.
#[derive(Debug)]
pub struct CommandStartTool;

impl McpTool for CommandStartTool {
    fn name(&self) -> &'static str {
        "command_start"
    }
    fn description(&self) -> &'static str {
        "Start one program in the background (same rules as run_command) and go on working. Returns an id at once; when it ends, a \
         notice follows a later result; read the output with command_status."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "program": {"type": "string", "description": "Program name on PATH, e.g. cargo, python, git."},
                "args": {"type": "array", "items": {"type": "string"}, "maxItems": MAX_ARGS,
                         "description": "Arguments; letters, digits and _ . / - = : , @ + only."},
                "cwd": {"type": "string", "description": "Working directory inside the workspace (default: workspace root)."},
                "timeout_ms": {"type": "integer", "minimum": 1000, "maximum": MAX_TIMEOUT_MS, "description": "Stopped after this long. Default 300000."}
            },
            "required": ["program"],
            "additionalProperties": false
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let program = args
            .get("program")
            .and_then(Value::as_str)
            .ok_or("`program` must be a string")?;
        if !plain_word(program) || program.contains(['/', '\\', ':']) {
            return Err("`program` must be a bare program name found on PATH".to_owned());
        }
        let words = optional_string_list(args, "args")?.unwrap_or_default();
        if words.len() > MAX_ARGS {
            return Err(format!("at most {MAX_ARGS} arguments per call"));
        }
        if let Some(bad) = words.iter().find(|word| !plain_word(word)) {
            return Err(format!(
                "argument `{bad}` uses characters this tool does not pass"
            ));
        }
        let cwd = match args.get("cwd") {
            None | Some(Value::Null) => None,
            Some(Value::String(dir)) if !dir.is_empty() => {
                check_read_path(ctx, dir)?;
                Some(dir.clone())
            }
            Some(_) => return Err("`cwd` must be a non-empty string".to_owned()),
        };
        let timeout_ms =
            u64_arg(args, "timeout_ms", DEFAULT_TIMEOUT_MS)?.clamp(1_000, MAX_TIMEOUT_MS);

        let mut line = program.to_owned();
        for word in &words {
            line.push(' ');
            line.push_str(word);
        }
        if let Err((rule, text)) = authorize(ctx, &line, cwd.as_deref()) {
            return Ok(blocked(ctx, &line, &rule, &text));
        }
        let running = jobs_of(&ctx.workspace)
            .iter()
            .filter(|job| job.running())
            .count();
        if running >= MAX_RUNNING {
            return Err(format!(
                "{running} background commands are already running (at most {MAX_RUNNING}); wait for one to end or stop one with command_stop"
            ));
        }

        let dir = cwd
            .as_ref()
            .map_or_else(|| ctx.workspace.clone(), |c| ctx.workspace.join(c));
        let mut command = Command::new(program);
        command
            .args(&words)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::hide_console(&mut command);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let settings = crate::sandbox::Settings {
            enabled: ctx.config.sandbox.enabled,
            network: ctx.config.sandbox.network,
        };
        let scratch = match crate::sandbox::confine(&mut command, &ctx.workspace, &settings) {
            Ok(scratch) => scratch,
            Err(reason) => {
                return Ok(blocked(
                    ctx,
                    &line,
                    "sandbox",
                    &format!("refused by the sandbox: {reason}"),
                ));
            }
        };
        let mut child = command.spawn().map_err(|e| {
            ctx.redactor
                .redact(&format!("failed to start `{program}`: {e}"))
                .into_owned()
        })?;

        let id = format!("bg-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let job = Arc::new(Job {
            id: id.clone(),
            line: line.clone(),
            started: Instant::now(),
            pid: child.id(),
            state: Mutex::new(State::Running),
            tail: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        with_jobs(|all| {
            let list = all.entry(ctx.workspace.clone()).or_default();
            // Remember a bounded number of finished commands: the oldest finished one goes first.
            while list.len() >= MAX_KEPT {
                match list.iter().position(|j| !j.running()) {
                    Some(i) => {
                        list.remove(i);
                    }
                    None => break,
                }
            }
            list.push(Arc::clone(&job));
        });

        if let Some(pipe) = child.stdout.take() {
            let job = Arc::clone(&job);
            thread::spawn(move || pump(pipe, &job));
        }
        if let Some(pipe) = child.stderr.take() {
            let job = Arc::clone(&job);
            thread::spawn(move || pump(pipe, &job));
        }
        write_record(
            &ctx.workspace,
            &Record {
                id: id.clone(),
                pid: job.pid,
                owner_pid: std::process::id(),
                program: program.to_owned(),
                started_unix_s: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
            },
        );
        let watcher = Arc::clone(&job);
        let workspace = ctx.workspace.clone();
        let timeout = Duration::from_millis(timeout_ms);
        thread::spawn(move || {
            // The sandbox's temporary directory lives until the command has ended.
            let _scratch = scratch;
            let deadline = Instant::now().checked_add(timeout);
            let (exit_code, status) = loop {
                match child.try_wait() {
                    Ok(Some(done)) => {
                        break (
                            done.code(),
                            if done.success() { "passed" } else { "failed" },
                        );
                    }
                    Ok(None) => {
                        let timed_out = deadline.is_some_and(|d| Instant::now() >= d);
                        if watcher.stop.load(Ordering::SeqCst) || timed_out {
                            let _ = kill_tree(watcher.pid);
                            let _ = child.kill();
                            let _ = child.wait();
                            break (None, if timed_out { "timeout" } else { "stopped" });
                        }
                        thread::sleep(POLL);
                    }
                    Err(_) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break (None, "failed");
                    }
                }
            };
            // Let the readers take the last of the output before the state says "finished".
            thread::sleep(Duration::from_millis(150));
            let duration_ms =
                u64::try_from(watcher.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            remove_record(&workspace, &watcher.id);
            *watcher.state.lock().unwrap_or_else(PoisonError::into_inner) = State::Finished {
                status,
                exit_code,
                duration_ms,
            };
            let how = match (status, exit_code) {
                ("passed", _) => "passed".to_owned(),
                ("failed", Some(code)) => format!("failed with exit {code}"),
                ("timeout", _) => "was stopped at its time limit".to_owned(),
                ("stopped", _) => "was stopped".to_owned(),
                _ => "failed".to_owned(),
            };
            crate::notices::post(
                &workspace,
                "background_finished",
                &format!(
                    "{} `{}` {how} after {} s. Read its output with command_status {}.",
                    watcher.id,
                    watcher.line,
                    duration_ms / 1000,
                    watcher.id
                ),
            );
        });

        Ok(redacted_output(
            ctx,
            &format!(
                "`{line}` started in the background as {id} (time limit {} s). You can go on; when it ends a notice follows a later result. Read its output with command_status {id}.",
                timeout_ms / 1000
            ),
            Some(json!({"status": "running", "id": id, "command": line, "timeout_ms": timeout_ms})),
        ))
    }
}

fn describe(ctx: &ToolContext, job: &Job) -> ToolOutput {
    let output = ctx.redactor.redact(&job.output()).into_owned();
    let tail = tail_text(&output, SHOW_TAIL_BYTES);
    let (summary, structured) = match job.state() {
        State::Running => {
            let elapsed_ms = u64::try_from(job.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            (
                format!(
                    "{} `{}`: running for {} s",
                    job.id,
                    job.line,
                    elapsed_ms / 1000
                ),
                json!({"status": "running", "id": job.id, "elapsed_ms": elapsed_ms, "output_tail": tail}),
            )
        }
        State::Finished {
            status,
            exit_code,
            duration_ms,
        } => {
            let code = exit_code.map_or(String::new(), |c| format!(", exit {c}"));
            (
                format!(
                    "{} `{}`: {status}{code} ({duration_ms} ms)",
                    job.id, job.line
                ),
                json!({"status": status, "id": job.id, "exit_code": exit_code, "duration_ms": duration_ms, "output_tail": tail}),
            )
        }
    };
    let mut text = summary;
    if job.running() {
        text.push_str("\nStill running. Do not poll: call command_status again with wait_ms (up to 10000) to wait for it, or carry on with other work; a notice follows a later result when it ends.");
    }
    for out_line in tail.trim_end().lines() {
        text.push_str(&format!("\n    {out_line}"));
    }
    redacted_output(ctx, &text, Some(structured))
}

/// `command_status`: the state and the latest output of a background command, or the list of them.
#[derive(Debug)]
pub struct CommandStatusTool;

impl McpTool for CommandStatusTool {
    fn name(&self) -> &'static str {
        "command_status"
    }
    fn description(&self) -> &'static str {
        "State, exit code and the last output of a background command (by id), or the list of them. Do not call it in a loop: pass \
         wait_ms (up to 10000) to wait for it to end, or carry on with other work, because a notice follows a later result when it ends."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "An id from command_start, e.g. bg-1. Omit to list the commands."},
                "wait_ms": {"type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS, "description": "Wait up to this long for it to end."}
            },
            "additionalProperties": false
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let Some(id) = args.get("id").and_then(Value::as_str) else {
            let jobs = jobs_of(&ctx.workspace);
            if jobs.is_empty() {
                return Ok(redacted_output(
                    ctx,
                    "No background commands in this session.",
                    Some(json!({"commands": []})),
                ));
            }
            let mut text = String::new();
            let mut list = Vec::new();
            for job in &jobs {
                let state = match job.state() {
                    State::Running => "running".to_owned(),
                    State::Finished {
                        status, exit_code, ..
                    } => format!(
                        "{status}{}",
                        exit_code.map_or(String::new(), |c| format!(", exit {c}"))
                    ),
                };
                text.push_str(&format!("{} `{}`: {state}\n", job.id, job.line));
                list.push(json!({"id": job.id, "command": job.line, "state": state}));
            }
            return Ok(redacted_output(
                ctx,
                text.trim_end(),
                Some(json!({"commands": list})),
            ));
        };
        let wait_ms = u64_arg(args, "wait_ms", 0)?.min(MAX_WAIT_MS);
        let job = find(&ctx.workspace, id).ok_or_else(|| format!("no background command `{id}` in this session; command_status without an id lists them"))?;
        if wait_ms > 0 {
            wait_finished(&job, Duration::from_millis(wait_ms));
        }
        Ok(describe(ctx, &job))
    }
}

/// `command_stop`: end a background command that Bridle started.
#[derive(Debug)]
pub struct CommandStopTool;

impl McpTool for CommandStopTool {
    fn name(&self) -> &'static str {
        "command_stop"
    }
    fn description(&self) -> &'static str {
        "Stop a background command started with command_start (the whole process tree). Only commands Bridle started can be stopped."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"id": {"type": "string", "description": "An id from command_start, e.g. bg-1."}},
            "required": ["id"],
            "additionalProperties": false
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or("`id` must be a string")?;
        let job = find(&ctx.workspace, id)
            .ok_or_else(|| format!("no background command `{id}` in this session"))?;
        job.stop.store(true, Ordering::SeqCst);
        wait_finished(&job, Duration::from_secs(3));
        Ok(describe(ctx, &job))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::mcp::tools::test_context;
    use harness_core::config::Config;

    fn ctx(dir: &Path) -> ToolContext {
        let mut config = Config::default();
        config.security.safe_commands = ["git", "ping", "sleep"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let mut c = test_context(dir);
        c.policy =
            harness_core::security::tool_policy::ToolPolicy::new(dir, &config.security).unwrap();
        c.config = config;
        c
    }

    /// A program that runs for about `secs` seconds on this system.
    fn long(secs: u64) -> Value {
        if cfg!(windows) {
            json!({"program": "ping", "args": ["-n", (secs + 1).to_string(), "127.0.0.1"], "timeout_ms": 60_000})
        } else {
            json!({"program": "sleep", "args": [secs.to_string()], "timeout_ms": 60_000})
        }
    }

    fn structured(out: &ToolOutput) -> &Value {
        out.structured.as_ref().unwrap()
    }

    #[test]
    fn the_tools_are_offered_only_when_turned_on() {
        let mut config = Config::default();
        assert!(tools(&config).is_empty());
        config.workflow.background_commands = true;
        let names: Vec<&str> = tools(&config).iter().map(|t| t.name()).collect();
        assert_eq!(names, ["command_start", "command_status", "command_stop"]);
    }

    #[test]
    fn a_command_runs_in_the_background_ends_and_leaves_a_notice_and_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let started = CommandStartTool
            .call(&json!({"program": "git", "args": ["--version"]}), &c)
            .unwrap();
        assert_eq!(structured(&started)["status"], "running", "{started:?}");
        let id = structured(&started)["id"].as_str().unwrap().to_owned();
        let done = CommandStatusTool
            .call(&json!({"id": id, "wait_ms": 8000}), &c)
            .unwrap();
        assert_eq!(structured(&done)["status"], "passed", "{done:?}");
        assert_eq!(structured(&done)["exit_code"], 0);
        assert!(
            structured(&done)["output_tail"]
                .as_str()
                .unwrap()
                .contains("git version"),
            "{done:?}"
        );
        // The end of the command was announced (the notice is queued just after the state changes, so give it a moment).
        let mut told = Vec::new();
        for _ in 0..100 {
            told.extend(crate::notices::take(dir.path()).shown);
            if !told.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            told.iter().any(|n| n.kind == "background_finished"
                && n.text.contains(&id)
                && n.text.contains("passed")),
            "{told:?}"
        );
        // The list shows it too.
        let list = CommandStatusTool.call(&json!({}), &c).unwrap();
        assert!(
            list.text.contains(&id) && list.text.contains("passed"),
            "{}",
            list.text
        );
    }

    #[test]
    fn a_failing_command_is_failed_with_its_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let started = CommandStartTool
            .call(
                &json!({"program": "git", "args": ["rev-parse", "--verify", "no-such-ref-xyz"]}),
                &c,
            )
            .unwrap();
        let id = structured(&started)["id"].as_str().unwrap().to_owned();
        let done = CommandStatusTool
            .call(&json!({"id": id, "wait_ms": 8000}), &c)
            .unwrap();
        assert_eq!(structured(&done)["status"], "failed", "{done:?}");
        assert_ne!(structured(&done)["exit_code"], 0);
    }

    #[test]
    fn a_running_command_can_be_stopped_and_one_over_its_time_limit_is_stopped_by_bridle() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let started = CommandStartTool.call(&long(30), &c).unwrap();
        let id = structured(&started)["id"].as_str().unwrap().to_owned();
        let running = CommandStatusTool.call(&json!({"id": id}), &c).unwrap();
        assert_eq!(structured(&running)["status"], "running");
        let stopped = CommandStopTool.call(&json!({"id": id}), &c).unwrap();
        assert_eq!(structured(&stopped)["status"], "stopped", "{stopped:?}");

        // The time limit is at least one second; a thirty-second command is ended at it.
        let mut limited = long(30);
        limited["timeout_ms"] = json!(1_000);
        let started = CommandStartTool.call(&limited, &c).unwrap();
        let id = structured(&started)["id"].as_str().unwrap().to_owned();
        let done = CommandStatusTool
            .call(&json!({"id": id, "wait_ms": 8000}), &c)
            .unwrap();
        assert_eq!(structured(&done)["status"], "timeout", "{done:?}");
    }

    #[test]
    fn a_command_the_policy_would_not_run_is_not_started_and_bad_arguments_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        // The same refusals as run_command: no path as the program, no shell characters.
        assert!(
            CommandStartTool
                .call(&json!({"program": "./evil"}), &c)
                .is_err()
        );
        assert!(
            CommandStartTool
                .call(&json!({"program": "git", "args": ["a;b"]}), &c)
                .is_err()
        );
        assert!(
            CommandStartTool
                .call(
                    &json!({"program": "git", "args": ["status"], "cwd": ".."}),
                    &c
                )
                .is_err()
        );
        // A program that is not allowed to run is blocked by the policy, not started.
        let blocked = CommandStartTool
            .call(
                &json!({"program": "curl", "args": ["http://example.com"]}),
                &c,
            )
            .unwrap();
        assert_eq!(structured(&blocked)["status"], "blocked", "{blocked:?}");
        assert!(jobs_of(dir.path()).is_empty(), "nothing was started");
        assert!(
            CommandStatusTool
                .call(&json!({"id": "bg-999999"}), &c)
                .is_err()
        );
        assert!(
            CommandStopTool
                .call(&json!({"id": "bg-999999"}), &c)
                .is_err()
        );
    }

    #[test]
    fn at_most_three_run_at_once_and_stop_all_ends_them() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        for _ in 0..MAX_RUNNING {
            let started = CommandStartTool.call(&long(30), &c).unwrap();
            assert_eq!(structured(&started)["status"], "running", "{started:?}");
        }
        let error = CommandStartTool.call(&long(30), &c).unwrap_err();
        assert!(error.contains("at most 3"), "{error}");
        stop_all(dir.path());
        assert!(jobs_of(dir.path()).iter().all(|job| !job.running()));
        // With the three ended, a new one may start.
        let again = CommandStartTool
            .call(&json!({"program": "git", "args": ["--version"]}), &c)
            .unwrap();
        assert_eq!(structured(&again)["status"], "running");
        stop_all(dir.path());
    }

    #[test]
    fn a_running_command_has_a_record_that_is_gone_when_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let started = CommandStartTool.call(&long(30), &c).unwrap();
        let id = structured(&started)["id"].as_str().unwrap().to_owned();
        let file = record_path(dir.path(), &id, std::process::id());
        assert!(file.is_file(), "a record while it runs");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.contains("\"program\":\"") && !text.contains("127.0.0.1"),
            "the program only, never the arguments: {text}"
        );
        CommandStopTool.call(&json!({"id": id}), &c).unwrap();
        assert!(!file.exists(), "no record once it has ended");
    }

    #[test]
    fn commands_left_by_a_dead_bridle_are_stopped_after_checking_the_process_id_still_belongs_to_them()
     {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let me = std::process::id();
        let record = |id: &str, pid: u32, owner: u32, program: &str| {
            write_record(
                ws,
                &Record {
                    id: id.to_owned(),
                    pid,
                    owner_pid: owner,
                    program: program.to_owned(),
                    started_unix_s: 0,
                },
            );
        };
        record("bg-1", 400, 100, "ping"); // owner is a running Bridle
        record("bg-2", 401, 101, "ping"); // owner gone, the process is still that program
        record("bg-3", 402, 102, "ping"); // owner gone, the pid now belongs to another program
        record("bg-4", 403, 103, "ping"); // owner gone, the process is gone too
        record("bg-5", 404, me, "ping"); // ours: never touched
        let image = |pid: u32| -> Option<String> {
            match pid {
                100 => Some("bridle-cli.exe".to_owned()),
                401 => Some("PING.EXE".to_owned()),
                402 => Some("chrome.exe".to_owned()),
                404 => Some("ping.exe".to_owned()),
                _ => None,
            }
        };
        let killed = std::cell::RefCell::new(Vec::new());
        let lines = sweep_with(ws, &image, &|pid| killed.borrow_mut().push(pid));
        assert_eq!(*killed.borrow(), vec![401], "{lines:?}");
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("bg-2") && l.contains("stopped")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("bg-3")
                && l.contains("chrome.exe")
                && l.contains("without signalling")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("bg-4") && l.contains("already gone")),
            "{lines:?}"
        );
        // The live owner's record and our own stay; the others are removed.
        assert!(record_path(ws, "bg-1", 100).exists() && record_path(ws, "bg-5", me).exists());
        for (id, owner) in [("bg-2", 101), ("bg-3", 102), ("bg-4", 103)] {
            assert!(!record_path(ws, id, owner).exists(), "{id}");
        }
        assert!(
            sweep_with(ws, &image, &|_| ()).is_empty(),
            "nothing more to do"
        );
    }
}
