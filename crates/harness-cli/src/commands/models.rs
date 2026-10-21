//! `agent models list|serve|stop|manifest` (§38, §43).
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

fn serve(app: &App, slot: &str, detach: bool, wait_secs: u64) -> anyhow::Result<ExitCode> {
    app.require_valid_config()?;
    check_slot_name(slot)?;
    let plan = plan_serve(&app.config, slot)?;
    let state = app.state_dir();
    let pid_file = pid_path(&state, slot);
    if let Some(pid) = read_pid(&pid_file)? {
        bail!(
            "`{slot}` already has a recorded server (pid {pid}); stop it with `{} models stop {slot}` first",
            identity::CLI_NAME
        );
    }

    let log = log_path(&state, slot);
    let mut child = launcher::spawn_server(&plan.bin, &plan.args, &log)
        .with_context(|| format!("starting {}", plan.bin.display()))?;
    let pid = child.id();
    if let Err(err) = write_pid(&pid_file, pid) {
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

fn stop(app: &App, slot: &str) -> anyhow::Result<ExitCode> {
    check_slot_name(slot)?;
    let pid_file = pid_path(&app.state_dir(), slot);
    let Some(pid) = read_pid(&pid_file)? else {
        println!("· no server recorded for `{slot}`; nothing to stop");
        return Ok(ExitCode::SUCCESS);
    };
    if terminate(pid)? {
        line(Mark::Ok, slot, format!("stopped (pid {pid})"));
    } else {
        line(
            Mark::Info,
            slot,
            format!("pid {pid} was not running; removed the stale record"),
        );
    }
    let _ = fs::remove_file(&pid_file);
    Ok(ExitCode::SUCCESS)
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
fn health_url(endpoint: &str) -> String {
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
/// either stream, after any backend-loading lines. `None` when the binary cannot be run.
pub(crate) fn runtime_version_text(bin: &Path) -> Option<String> {
    let out = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
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

fn write_pid(path: &Path, pid: u32) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, format!("{pid}\n"))
}

/// `Ok(None)` when no PID file exists. A file that does not hold a number is an error.
fn read_pid(path: &Path) -> anyhow::Result<Option<u32>> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .trim()
            .parse::<u32>()
            .map(Some)
            .with_context(|| format!("{} does not contain a process id", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

/// Asks process `pid` to exit. `Ok(false)` means no such process was found.
fn terminate(pid: u32) -> anyhow::Result<bool> {
    let status = signal_command(pid)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running the stop command")?;
    if status.success() {
        return Ok(true);
    }
    if process_exists(pid) {
        bail!("could not stop pid {pid}; it is still running");
    }
    Ok(false)
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

#[cfg(not(windows))]
fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// `taskkill` reports a missing process with a failure code and gives no cheap way to tell it apart
/// from a denied request, so a failed Windows stop is treated as "not running".
#[cfg(windows)]
fn process_exists(_pid: u32) -> bool {
    false
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
    fn pid_file_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = pid_path(dir.path(), "main");
        assert_eq!(read_pid(&path).expect("absent is fine"), None);
        write_pid(&path, 4242).expect("write");
        assert_eq!(read_pid(&path).expect("read"), Some(4242));
        fs::write(&path, "not a pid").expect("corrupt");
        assert!(read_pid(&path).is_err());
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
}
