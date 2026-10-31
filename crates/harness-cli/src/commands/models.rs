//! `agent models list|serve|stop|manifest`.
//!
//! `serve` starts a slot's inference server from its `[models.<slot>.launch]` section. The PID is
//! recorded under `<state>/servers/<slot>.pid` so `stop` can find the process later, and the output
//! goes to `<state>/logs/<slot>.log`. `manifest` records what the model file and runtime are, under
//! `<state>/manifests/`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::Subcommand;
use harness_core::config::{Config, LaunchConfig, ModelConfig};
use harness_core::identity;
use harness_runtime::launcher;
use harness_runtime::manifest::{self, ManifestInput, RuntimeVersion};
use harness_runtime::sysprobe;

use crate::app::App;
use crate::commands::health::{configured_slots, wire_name};
use crate::output::{Mark, line};

/// How often `serve` polls the health endpoint while the model loads.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Subcommand)]
pub enum ModelsAction {
    /// List configured model slots.
    List,
    /// Show each slot's server state: unloaded, warming, ready or error.
    Status {
        /// Slot to show (default: every configured slot).
        slot: Option<String>,
    },
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
    match action {
        ModelsAction::List => Ok(list(app)),
        ModelsAction::Status { slot } => status(app, slot.as_deref()),
        ModelsAction::Serve {
            slot,
            detach,
            wait_secs,
        } => serve(app, slot, *detach, *wait_secs),
        ModelsAction::Stop { slot } => stop(app, slot),
        ModelsAction::Manifest { slot, no_hash } => {
            manifest_command(app, slot.as_deref(), *no_hash)
        }
    }
}

fn list(app: &App) -> ExitCode {
    if app.config.models.is_empty() {
        println!("· no model slots configured (add [models.<slot>] to the config)");
        return ExitCode::SUCCESS;
    }
    for (slot, model) in &app.config.models {
        println!(
            "{slot:<10} id={} role={} endpoint={} hardware={}",
            model.id,
            wire_name(&model.role),
            model.endpoint,
            wire_name(&model.hardware)
        );
    }
    ExitCode::SUCCESS
}

/// Where a slot's server stands, as seen from outside the process. `loaded`, `idle` and
/// `unloading` need signals this command does not have (weights resident, time since the last
/// request, a stop in progress), so they are not reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    /// No server was started for the slot, or it was stopped.
    Unloaded,
    /// The server process runs but its health endpoint does not answer 200 yet.
    Warming,
    /// The server process runs and its health endpoint answers 200.
    Ready,
    /// A record exists but its process is gone, or the PID belongs to another program.
    Error,
}

/// Maps the PID check and the health answer to a lifecycle state. `healthy` is only read for a
/// running process.
fn lifecycle(pid_state: Option<&RecordState>, healthy: bool) -> Lifecycle {
    match pid_state {
        None => Lifecycle::Unloaded,
        Some(RecordState::Running) if healthy => Lifecycle::Ready,
        Some(RecordState::Running) => Lifecycle::Warming,
        Some(RecordState::Gone | RecordState::Reused(_)) => Lifecycle::Error,
    }
}

/// `agent models status`: one line per slot. Only a running process is probed over HTTP.
fn status(app: &App, slot: Option<&str>) -> anyhow::Result<ExitCode> {
    let state = app.state_dir();
    let slots: Vec<&str> = match slot {
        Some(name) => {
            check_slot_name(name)?;
            slot_model(&app.config, name)?;
            vec![name]
        }
        None => app.config.models.keys().map(String::as_str).collect(),
    };
    if slots.is_empty() {
        println!("· no model slots configured (add [models.<slot>] to the config)");
    }
    for name in slots {
        let model = slot_model(&app.config, name)?;
        let record = read_record(&pid_path(&state, name))?;
        let pid_state = record
            .as_ref()
            .map(|record| record_state(record, &expected_server_bin(&app.config, name)));
        let healthy = pid_state == Some(RecordState::Running)
            && launcher::is_healthy(&health_url(&model.endpoint));
        let (mark, text) = match lifecycle(pid_state.as_ref(), healthy) {
            Lifecycle::Unloaded => (Mark::Info, "unloaded".to_owned()),
            Lifecycle::Warming => (
                Mark::Warn,
                "warming (health check does not answer 200 yet)".to_owned(),
            ),
            Lifecycle::Ready => (Mark::Ok, "ready".to_owned()),
            Lifecycle::Error => {
                let pid = record.as_ref().map_or(0, |record| record.pid);
                let detail = match &pid_state {
                    Some(RecordState::Reused(image)) => {
                        format!("pid {pid} now belongs to `{image}`")
                    }
                    _ => format!("pid {pid} is gone"),
                };
                (
                    Mark::Fail,
                    format!(
                        "error: {detail}; `{} models serve {name}` starts it again",
                        identity::CLI_NAME
                    ),
                )
            }
        };
        line(mark, name, text);
    }
    Ok(ExitCode::SUCCESS)
}

fn serve(app: &App, slot: &str, detach: bool, wait_secs: u64) -> anyhow::Result<ExitCode> {
    app.require_valid_config()?;
    check_slot_name(slot)?;
    let plan = plan_serve(&app.config, slot)?;
    let state = app.state_dir();
    let pid_file = pid_path(&state, slot);
    check_one_gpu_model(app, slot)?;
    if let Some(record) = read_record(&pid_file)? {
        if record_state(&record, &plan.bin) == RecordState::Running {
            bail!(
                "`{slot}` already has a running server (pid {}); stop it with `{} models stop {slot}` first",
                record.pid,
                identity::CLI_NAME
            );
        }
        // The recorded process is gone or the PID was reused: the record is stale.
        let _ = fs::remove_file(&pid_file);
        println!("· removed a stale record for `{slot}` (pid {})", record.pid);
    }

    let log = log_path(&state, slot);
    let mut child = launcher::spawn_server(&plan.bin, &plan.args, &log)
        .with_context(|| format!("starting {}", plan.bin.display()))?;
    let pid = child.id();
    if let Err(err) = write_record(&pid_file, pid, &plan.bin) {
        let _ = child.kill();
        bail!(
            "cannot record the server pid in {}: {err}",
            pid_file.display()
        );
    }
    println!("· starting {slot} (pid {pid}); log: {}", log.display());

    let timeout = Duration::from_secs(wait_secs);
    match launcher::wait_until_healthy(&plan.health_url, timeout, POLL_INTERVAL, Some(&mut child)) {
        Ok(elapsed) => line(
            Mark::Ok,
            slot,
            format!(
                "healthy after {:.1} s at {}",
                elapsed.as_secs_f64(),
                plan.health_url
            ),
        ),
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&pid_file);
            bail!(
                "`{slot}` did not become healthy: {err} (see {})",
                log.display()
            );
        }
    }

    if detach {
        println!(
            "· left running in the background; stop it with `{} models stop {slot}`",
            identity::CLI_NAME
        );
        return Ok(ExitCode::SUCCESS);
    }
    let status = child.wait().context("waiting for the server process")?;
    let _ = fs::remove_file(&pid_file);
    println!("· {slot} server exited ({status})");
    Ok(exit_code_for(status))
}

/// One GPU model at a time: a GPU slot may start only when no other GPU slot's server
/// is running. The person stops the other one first; nothing is stopped automatically.
fn check_one_gpu_model(app: &App, slot: &str) -> anyhow::Result<()> {
    let is_gpu = |name: &str| {
        app.config
            .models
            .get(name)
            .is_some_and(|model| model.hardware == harness_core::config::Hardware::Gpu)
    };
    if !is_gpu(slot) {
        return Ok(());
    }
    for other in app
        .config
        .models
        .keys()
        .filter(|name| name.as_str() != slot && is_gpu(name))
    {
        let pid_file = pid_path(&app.state_dir(), other);
        if let Some(record) = read_record(&pid_file)?
            && record_state(&record, &expected_server_bin(&app.config, other))
                == RecordState::Running
        {
            bail!(
                "`{other}` is a GPU server that is running (pid {}); only one GPU model runs at a time. \
                 Stop it first with `{} models stop {other}`",
                record.pid,
                identity::CLI_NAME
            );
        }
    }
    Ok(())
}

/// What `stop_server` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    NoRecord,
    Stopped(u32),
    /// The recorded process was not running; the stale record was removed.
    Gone(u32),
    /// The PID belongs to another program now; it was not signalled.
    Reused {
        pid: u32,
        image: String,
    },
}

/// Stops a server started by `serve`, with the PID-reuse safeguard, and reports what happened.
/// Prints nothing. Shared by `stop` and the TUI.
pub(crate) fn stop_server(app: &App, slot: &str) -> anyhow::Result<StopOutcome> {
    check_slot_name(slot)?;
    let pid_file = pid_path(&app.state_dir(), slot);
    let Some(record) = read_record(&pid_file)? else {
        return Ok(StopOutcome::NoRecord);
    };
    let pid = record.pid;
    let outcome = match record_state(&record, &expected_server_bin(&app.config, slot)) {
        RecordState::Running => {
            terminate(pid)?;
            StopOutcome::Stopped(pid)
        }
        RecordState::Gone => StopOutcome::Gone(pid),
        // After a crash or reboot the PID can belong to an unrelated program: never signal it.
        RecordState::Reused(image) => StopOutcome::Reused { pid, image },
    };
    let _ = fs::remove_file(&pid_file);
    Ok(outcome)
}

fn stop(app: &App, slot: &str) -> anyhow::Result<ExitCode> {
    match stop_server(app, slot)? {
        StopOutcome::NoRecord => println!("· no server recorded for `{slot}`; nothing to stop"),
        StopOutcome::Stopped(pid) => line(Mark::Ok, slot, format!("stopped (pid {pid})")),
        StopOutcome::Gone(pid) => line(
            Mark::Info,
            slot,
            format!("pid {pid} was not running; removed the stale record"),
        ),
        StopOutcome::Reused { pid, image } => line(
            Mark::Warn,
            slot,
            format!(
                "pid {pid} now belongs to `{image}`, not the server; removed the stale record \
                 without signalling it"
            ),
        ),
    }
    Ok(ExitCode::SUCCESS)
}

/// A slot's managed-server state from its PID record only. Read-only: it never signals, never
/// contacts the endpoint, so a responding endpoint is not mistaken for a managed server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServerState {
    /// No record: Bridle has not started a server for this slot.
    NotManaged,
    Running {
        pid: u32,
    },
    /// A record exists but its process is gone.
    Stale {
        pid: u32,
    },
    /// A record exists but the PID now belongs to another program.
    Reused {
        pid: u32,
        image: String,
    },
    /// The record file could not be read or parsed.
    Unreadable(String),
}

pub(crate) fn server_state(app: &App, slot: &str) -> ServerState {
    if check_slot_name(slot).is_err() {
        return ServerState::Unreadable("invalid slot name".to_owned());
    }
    match read_record(&pid_path(&app.state_dir(), slot)) {
        Ok(None) => ServerState::NotManaged,
        Ok(Some(record)) => match record_state(&record, &expected_server_bin(&app.config, slot)) {
            RecordState::Running => ServerState::Running { pid: record.pid },
            RecordState::Gone => ServerState::Stale { pid: record.pid },
            RecordState::Reused(image) => ServerState::Reused {
                pid: record.pid,
                image,
            },
        },
        Err(err) => ServerState::Unreadable(format!("{err:#}")),
    }
}

/// Where the slot's server log is (it may not exist).
pub(crate) fn server_log_path(app: &App, slot: &str) -> PathBuf {
    log_path(&app.state_dir(), slot)
}

/// `serve --detach` without printing: the checks `serve` makes (valid config, one GPU model at a
/// time, no second server for the slot), then the quiet start, which waits for health.
pub(crate) fn start_checked(app: &App, slot: &str, wait: Duration) -> anyhow::Result<()> {
    app.require_valid_config()?;
    check_slot_name(slot)?;
    check_one_gpu_model(app, slot)?;
    if is_running(app, slot)? {
        bail!(
            "`{slot}` already has a running server; stop it with `{} models stop {slot}` first",
            identity::CLI_NAME
        );
    }
    start_quiet(app, slot, wait)
}

/// Whether a slot's server is running: its PID record names a live process of the server's program.
pub(crate) fn is_running(app: &App, slot: &str) -> anyhow::Result<bool> {
    check_slot_name(slot)?;
    let Some(record) = read_record(&pid_path(&app.state_dir(), slot))? else {
        return Ok(false);
    };
    Ok(record_state(&record, &expected_server_bin(&app.config, slot)) == RecordState::Running)
}

/// Starts a slot's server and returns once it answers health. Prints nothing: the escalation swap runs inside a hook,
/// whose stdout carries the answer. A server that is already running is left alone.
pub(crate) fn start_quiet(app: &App, slot: &str, wait: Duration) -> anyhow::Result<()> {
    check_slot_name(slot)?;
    if is_running(app, slot)? {
        return Ok(());
    }
    let plan = plan_serve(&app.config, slot)?;
    let state = app.state_dir();
    let pid_file = pid_path(&state, slot);
    let _ = fs::remove_file(&pid_file);
    let log = log_path(&state, slot);
    let mut child = launcher::spawn_server(&plan.bin, &plan.args, &log)
        .with_context(|| format!("starting {}", plan.bin.display()))?;
    if let Err(err) = write_record(&pid_file, child.id(), &plan.bin) {
        let _ = child.kill();
        bail!(
            "cannot record the server pid in {}: {err}",
            pid_file.display()
        );
    }
    match launcher::wait_until_healthy(&plan.health_url, wait, POLL_INTERVAL, Some(&mut child)) {
        Ok(_) => Ok(()),
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&pid_file);
            bail!(
                "`{slot}` did not become healthy: {err} (see {})",
                log.display()
            );
        }
    }
}

/// Stops a slot's server and prints nothing. A record whose process is gone is removed.
pub(crate) fn stop_quiet(app: &App, slot: &str) -> anyhow::Result<()> {
    check_slot_name(slot)?;
    let pid_file = pid_path(&app.state_dir(), slot);
    if let Some(record) = read_record(&pid_file)? {
        if record_state(&record, &expected_server_bin(&app.config, slot)) == RecordState::Running {
            terminate(record.pid)?;
        }
        let _ = fs::remove_file(&pid_file);
    }
    Ok(())
}

fn manifest_command(app: &App, slot: Option<&str>, no_hash: bool) -> anyhow::Result<ExitCode> {
    app.require_valid_config()?;
    let slots: Vec<&str> = match slot {
        Some(name) => {
            check_slot_name(name)?;
            launch_of(slot_model(&app.config, name)?, name)?;
            vec![name]
        }
        None => app
            .config
            .models
            .iter()
            .filter(|(_, model)| model.launch.is_some())
            .map(|(name, _)| name.as_str())
            .collect(),
    };
    if slots.is_empty() {
        println!("· no slot has a launch section; nothing to record");
        return Ok(ExitCode::SUCCESS);
    }

    let state = app.state_dir();
    let dir = state.join("manifests");
    let host = host_facts();
    let mut failed = false;
    for name in slots {
        match record_manifest(&app.config, name, &state, &dir, &host, no_hash) {
            Ok(path) => line(
                Mark::Ok,
                name,
                format!("manifest written to {}", path.display()),
            ),
            Err(err) => {
                failed = true;
                line(Mark::Fail, name, format!("{err:#}"));
            }
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Everything `serve` needs, resolved from the config without starting anything.
#[derive(Debug, PartialEq, Eq)]
struct ServePlan {
    bin: PathBuf,
    args: Vec<String>,
    health_url: String,
}

fn plan_serve(config: &Config, slot: &str) -> anyhow::Result<ServePlan> {
    let model = slot_model(config, slot)?;
    let launch = launch_of(model, slot)?;
    if !config.runtimes.contains_key(&launch.runtime) {
        bail!(
            "slot `{slot}` uses runtime `{}`, which is not defined under [runtimes]",
            launch.runtime
        );
    }
    let bin = config
        .resolve_runtime_bin(&launch.runtime)
        .with_context(|| {
            format!(
                "runtime `{}` has a relative server_bin, but paths.runtimes_dir is not set",
                launch.runtime
            )
        })?;
    if !bin.is_file() {
        bail!(
            "inference server binary not found: {} (check runtimes.{}.server_bin and paths.runtimes_dir)",
            bin.display(),
            launch.runtime
        );
    }
    let model_file = config.resolve_model_file(slot).with_context(|| {
        format!(
            "model file `{}` is relative, but paths.models_dir is not set",
            launch.model_file.display()
        )
    })?;
    if !model_file.is_file() {
        bail!(
            "model file not found: {} (check models.{slot}.launch.model_file and paths.models_dir)",
            model_file.display()
        );
    }
    let args = launcher::server_args(model, launch, &model_file, false)?;
    Ok(ServePlan {
        bin,
        args,
        health_url: health_url(&model.endpoint),
    })
}

/// `GET <root>/health`, where the root is the endpoint without its `/v1` suffix.
pub(crate) fn health_url(endpoint: &str) -> String {
    let root = endpoint.trim_end_matches('/');
    let root = root.strip_suffix("/v1").unwrap_or(root);
    format!("{root}/health")
}

/// Hashes the model file and records its manifest. Returns the manifest path.
fn record_manifest(
    config: &Config,
    slot: &str,
    state: &Path,
    dir: &Path,
    host: &HostFacts,
    no_hash: bool,
) -> anyhow::Result<PathBuf> {
    let model = slot_model(config, slot)?;
    let launch = launch_of(model, slot)?;
    let model_file = config
        .resolve_model_file(slot)
        .with_context(|| format!("model file for `{slot}` cannot be resolved"))?;
    let size = fs::metadata(&model_file)
        .with_context(|| format!("model file {} is missing", model_file.display()))?
        .len();
    let sha256 = if no_hash {
        None
    } else {
        let cache = manifest::hash_cache_path(state);
        Some(
            manifest::cached_sha256(&cache, &model_file)
                .with_context(|| format!("hashing {}", model_file.display()))?,
        )
    };
    let (runtime_kind, runtime) = runtime_identity(config, &launch.runtime);
    if runtime.is_none() {
        line(
            Mark::Warn,
            slot,
            format!(
                "runtime build not recognised from `{} --version`",
                launch.runtime
            ),
        );
    }
    let written = manifest::build_manifest(ManifestInput {
        slot,
        model,
        model_file: Some(&model_file),
        size_bytes: Some(size),
        sha256,
        runtime_kind,
        runtime,
        gpu: host.gpu.clone(),
        driver: host.driver.clone(),
        cuda: host.cuda.clone(),
        os: host.os.clone(),
    });
    Ok(manifest::write_manifest(dir, &written)?)
}

/// The runtime's kind (its config name, such as `llama_cpp`) and the build its binary reports.
fn runtime_identity(config: &Config, runtime: &str) -> (Option<String>, Option<RuntimeVersion>) {
    let kind = config.runtimes.get(runtime).map(|rt| wire_name(&rt.kind));
    let version = config
        .resolve_runtime_bin(runtime)
        .and_then(|bin| runtime_version_text(&bin))
        .and_then(|text| manifest::parse_llama_version(&text));
    (kind, version)
}

/// Full `<bin> --version` output, stdout and stderr together: llama.cpp writes its version line to
/// either stream, after any backend-loading lines. `None` when the binary cannot be run or does
/// not answer within the probe timeout (a hung binary must not hang `doctor`).
pub(crate) fn runtime_version_text(bin: &Path) -> Option<String> {
    sysprobe::command_output(bin, &["--version"])
}

/// Host facts recorded in every manifest.
#[derive(Debug)]
struct HostFacts {
    os: String,
    gpu: Option<String>,
    driver: Option<String>,
    cuda: Option<String>,
}

fn host_facts() -> HostFacts {
    let report = sysprobe::probe_system();
    let os = match &report.os_version {
        Some(version) => format!("{} {version}", report.os),
        None => report.os.clone(),
    };
    let gpu = (!report.gpus.is_empty()).then(|| {
        report
            .gpus
            .iter()
            .map(|gpu| gpu.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    });
    HostFacts {
        os,
        gpu,
        driver: report.gpus.first().map(|gpu| gpu.driver.clone()),
        cuda: report.cuda_version,
    }
}

fn slot_model<'a>(config: &'a Config, slot: &str) -> anyhow::Result<&'a ModelConfig> {
    config.models.get(slot).with_context(|| {
        format!(
            "no model slot named `{slot}` (configured: {})",
            configured_slots(config)
        )
    })
}

fn launch_of<'a>(model: &'a ModelConfig, slot: &str) -> anyhow::Result<&'a LaunchConfig> {
    model
        .launch
        .as_ref()
        .with_context(|| format!("slot `{slot}` has no [models.{slot}.launch] section"))
}

/// Slot names become file names under `servers/`, `logs/` and `manifests/`.
fn check_slot_name(slot: &str) -> anyhow::Result<()> {
    if slot.is_empty() || slot == "." || slot == ".." || slot.contains(['/', '\\']) {
        bail!("`{slot}` is not a valid slot name");
    }
    Ok(())
}

fn pid_path(state: &Path, slot: &str) -> PathBuf {
    state.join("servers").join(format!("{slot}.pid"))
}

fn log_path(state: &Path, slot: &str) -> PathBuf {
    state.join("logs").join(format!("{slot}.log"))
}

/// What `serve` records about a server it started.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerRecord {
    pid: u32,
    /// The binary that was started. `None` in records written before it was recorded.
    bin: Option<PathBuf>,
}

/// Where a recorded PID stands now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordState {
    /// The PID runs the recorded server binary.
    Running,
    /// No process has that PID.
    Gone,
    /// The PID now belongs to another program (reused after a crash or reboot).
    Reused(String),
}

fn write_record(path: &Path, pid: u32, bin: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, format!("pid={pid}\nbin={}\n", bin.display()))
}

/// `Ok(None)` when no record exists. A record without a valid process id is an error.
fn read_record(path: &Path) -> anyhow::Result<Option<ServerRecord>> {
    match fs::read_to_string(path) {
        Ok(text) => parse_record(&text)
            .map(Some)
            .with_context(|| format!("{} is not a valid server record", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

/// Parses `pid=<n>` and `bin=<path>` lines; a bare number is an older record holding only the PID.
fn parse_record(text: &str) -> anyhow::Result<ServerRecord> {
    let mut pid = None;
    let mut bin = None;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        match line.split_once('=') {
            Some(("pid", value)) => pid = Some(value.trim()),
            Some(("bin", value)) => bin = Some(PathBuf::from(value.trim())),
            Some(_) => {}
            None => pid = Some(line),
        }
    }
    let pid: u32 = pid
        .context("no process id")?
        .parse()
        .context("the process id is not a number")?;
    // `kill 0` signals the caller's own process group, and values above i32::MAX wrap to negative
    // ids (`-1` is every process the user owns), so only real process ids are accepted.
    if pid == 0 || i32::try_from(pid).is_err() {
        bail!("{pid} is not a usable process id");
    }
    Ok(ServerRecord { pid, bin })
}

/// Checks the recorded PID against the program that should be running there: the recorded binary,
/// or `expected` for an older record.
fn record_state(record: &ServerRecord, expected: &Path) -> RecordState {
    let bin = record.bin.as_deref().unwrap_or(expected);
    match sysprobe::process_image(record.pid) {
        None => RecordState::Gone,
        Some(image) if same_program(&image, bin) => RecordState::Running,
        Some(image) => RecordState::Reused(image),
    }
}

/// Whether a process image name is the program at `bin`: same file name, ignoring case and `.exe`.
fn same_program(image: &str, bin: &Path) -> bool {
    let stem = |name: &str| {
        let lower = name.to_ascii_lowercase();
        lower
            .strip_suffix(".exe")
            .map_or(lower.clone(), str::to_owned)
    };
    bin.file_name()
        .is_some_and(|name| stem(&name.to_string_lossy()) == stem(image))
}

/// The server binary configured for `slot`, used to check records that do not name their binary.
fn expected_server_bin(config: &Config, slot: &str) -> PathBuf {
    config
        .models
        .get(slot)
        .and_then(|model| model.launch.as_ref())
        .and_then(|launch| config.resolve_runtime_bin(&launch.runtime))
        .unwrap_or_else(|| PathBuf::from("llama-server"))
}

/// Asks process `pid` to exit. Fails only when the process is still running afterwards.
fn terminate(pid: u32) -> anyhow::Result<()> {
    let status = signal_command(pid)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running the stop command")?;
    if !status.success() && sysprobe::process_image(pid).is_some() {
        bail!("could not stop pid {pid}; it is still running");
    }
    Ok(())
}

#[cfg(windows)]
fn signal_command(pid: u32) -> Command {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
    cmd
}

#[cfg(not(windows))]
fn signal_command(pid: u32) -> Command {
    let mut cmd = Command::new("kill");
    cmd.arg(pid.to_string());
    cmd
}

/// The server's exit code, forwarded. A process killed by a signal has no code and reports failure.
fn exit_code_for(status: ExitStatus) -> ExitCode {
    match status.code().and_then(|code| u8::try_from(code).ok()) {
        Some(code) => ExitCode::from(code),
        None => ExitCode::FAILURE,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use harness_core::config::from_toml_str;

    const CONFIG: &str = r#"
[runtimes.llama]
kind = "llama_cpp"
server_bin = "llama-server"

[models.main]
provider = "llama_cpp"
id = "m"
role = "engineer"
endpoint = "http://127.0.0.1:8001/v1"
context = 4096
hardware = "gpu"

[models.main.launch]
runtime = "llama"
model_file = "model.gguf"
gpu_layers = 99

[models.bare]
provider = "llama_cpp"
id = "b"
role = "decision"
endpoint = "http://127.0.0.1:8002/v1"
context = 4096
hardware = "cpu"
"#;

    /// Config whose runtime and model directories are the given temp directory.
    fn config_in(dir: &Path) -> Config {
        let mut config = from_toml_str(CONFIG).expect("fixture is valid");
        config.paths.runtimes_dir = Some(dir.to_path_buf());
        config.paths.models_dir = Some(dir.to_path_buf());
        config
    }

    #[test]
    fn health_url_drops_the_v1_suffix() {
        assert_eq!(
            health_url("http://127.0.0.1:8001/v1"),
            "http://127.0.0.1:8001/health"
        );
        assert_eq!(
            health_url("http://127.0.0.1:8001/v1/"),
            "http://127.0.0.1:8001/health"
        );
        assert_eq!(
            health_url("http://127.0.0.1:8001"),
            "http://127.0.0.1:8001/health"
        );
    }

    #[test]
    fn serve_plan_builds_the_argv_for_a_gpu_slot() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("llama-server"), b"").expect("bin");
        fs::write(dir.path().join("model.gguf"), b"").expect("model");
        let config = config_in(dir.path());

        let plan = plan_serve(&config, "main").expect("plan");

        assert_eq!(plan.bin, dir.path().join("llama-server"));
        assert_eq!(plan.health_url, "http://127.0.0.1:8001/health");
        let model_file = dir.path().join("model.gguf").display().to_string();
        assert_eq!(
            plan.args,
            [
                "-m",
                model_file.as_str(),
                "--host",
                "127.0.0.1",
                "--port",
                "8001",
                "--alias",
                "m",
                "-c",
                "4096",
                "--jinja",
                "--metrics",
                "-ngl",
                "99",
                "-np",
                "1",
            ]
        );
    }

    #[test]
    fn serve_plan_reports_a_missing_model_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("llama-server"), b"").expect("bin");
        let config = config_in(dir.path());
        let err = plan_serve(&config, "main").expect_err("model file is absent");
        assert!(err.to_string().contains("model file not found"), "{err}");
    }

    #[test]
    fn serve_plan_reports_a_missing_binary_and_slots_without_launch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = config_in(dir.path());
        let err = plan_serve(&config, "main").expect_err("binary is absent");
        assert!(err.to_string().contains("binary not found"), "{err}");
        let err = plan_serve(&config, "bare").expect_err("no launch section");
        assert!(err.to_string().contains("no [models.bare.launch]"), "{err}");
        assert!(plan_serve(&config, "nope").is_err());
    }

    #[test]
    fn server_record_round_trips_and_reads_older_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = pid_path(dir.path(), "main");
        assert_eq!(read_record(&path).expect("absent is fine"), None);
        write_record(&path, 4242, Path::new("llama-server")).expect("write");
        assert_eq!(
            read_record(&path).expect("read"),
            Some(ServerRecord {
                pid: 4242,
                bin: Some(PathBuf::from("llama-server")),
            })
        );
        fs::write(&path, "4242\n").expect("older record");
        assert_eq!(
            read_record(&path).expect("read"),
            Some(ServerRecord {
                pid: 4242,
                bin: None
            })
        );
        fs::write(&path, "not a pid").expect("corrupt");
        assert!(read_record(&path).is_err());
    }

    #[test]
    fn records_never_hold_a_process_group_or_wrapped_pid() {
        for text in ["0", "pid=0", "4294967295", "pid=2147483648", "-1", "pid="] {
            assert!(parse_record(text).is_err(), "{text}");
        }
        assert!(parse_record("pid=2147483647").is_ok());
    }

    #[test]
    fn program_names_match_ignoring_case_and_exe() {
        assert!(same_program(
            "LLAMA-SERVER.EXE",
            Path::new("runtimes/b11756/llama-server.exe")
        ));
        assert!(same_program("llama-server", Path::new("/opt/llama-server")));
        assert!(!same_program("notepad.exe", Path::new("llama-server.exe")));
    }

    #[test]
    fn a_reused_pid_is_reported_not_signalled() {
        // This test process stands in for an unrelated program that got the recorded PID.
        let record = ServerRecord {
            pid: std::process::id(),
            bin: Some(PathBuf::from("llama-server.exe")),
        };
        assert!(matches!(
            record_state(&record, Path::new("llama-server.exe")),
            RecordState::Reused(_)
        ));
        let ours = ServerRecord {
            pid: std::process::id(),
            bin: Some(std::env::current_exe().expect("current exe")),
        };
        assert_eq!(record_state(&ours, Path::new("x")), RecordState::Running);
        let gone = ServerRecord {
            pid: u32::try_from(i32::MAX - 1).expect("fits"),
            bin: None,
        };
        assert_eq!(record_state(&gone, Path::new("x")), RecordState::Gone);
    }

    #[test]
    fn slot_names_cannot_escape_the_state_directory() {
        assert!(check_slot_name("main").is_ok());
        assert!(check_slot_name("").is_err());
        assert!(check_slot_name("..").is_err());
        assert!(check_slot_name("../x").is_err());
        assert!(check_slot_name(r"a\b").is_err());
    }

    #[test]
    fn state_layout_is_servers_and_logs() {
        let state = Path::new("/state");
        assert_eq!(
            pid_path(state, "main"),
            Path::new("/state/servers/main.pid")
        );
        assert_eq!(log_path(state, "main"), Path::new("/state/logs/main.log"));
    }

    #[test]
    fn a_slot_without_a_record_is_unloaded() {
        assert_eq!(lifecycle(None, false), Lifecycle::Unloaded);
    }

    #[test]
    fn a_running_server_is_warming_until_it_answers_health() {
        assert_eq!(
            lifecycle(Some(&RecordState::Running), false),
            Lifecycle::Warming
        );
        assert_eq!(
            lifecycle(Some(&RecordState::Running), true),
            Lifecycle::Ready
        );
    }

    #[test]
    fn a_record_whose_process_is_gone_or_reused_is_an_error() {
        assert_eq!(lifecycle(Some(&RecordState::Gone), true), Lifecycle::Error);
        assert_eq!(
            lifecycle(Some(&RecordState::Reused("notepad.exe".into())), false),
            Lifecycle::Error
        );
    }
}
