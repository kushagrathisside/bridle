//! Starts local inference servers from config (`agent models serve <slot>`).
//!
//! IMPLEMENTATION SPEC (delegated):
//! - `server_args(model, launch, model_file) -> Result<Vec<String>, LaunchError>` (pure, unit
//!   tested): parse host and port from `model.endpoint` (`http://HOST:PORT/v1`; missing port →
//!   `LaunchError::Endpoint`). Emit, in this order:
//!   `-m <model_file> --host <host> --port <port> --alias <model.id> -c <model.context> --jinja
//!   --metrics`, then `-ngl <n>` if `gpu_layers` set, `-t <n>` if `threads` set,
//!   `-fa on|off` if `flash_attn` set, `-ctk <kv> -ctv <kv>` if `model.kv_cache` set,
//!   `--load-mode <mode>` if `load_mode` set, `-np <parallel>`, `--cache-ram <n>` if
//!   `cache_ram_mib` set, `--reasoning-budget <n>` if `model.reasoning.budget_tokens` set,
//!   then `launch.extra_args` verbatim. Never `--no-mmap` (absent from b11756); never
//!   `--spec-type` unless the user passes it in `extra_args`.
//!   Host must be loopback unless the caller passes `allow_remote_bind = true` (→
//!   `LaunchError::NonLoopbackBind`) — servers never listen on the LAN by accident.
//! - `spawn_server(bin, args, log_file) -> Result<std::process::Child, LaunchError>`: stdout and
//!   stderr appended to `log_file` (create parent dirs); on Windows set `CREATE_NO_WINDOW`
//!   (0x0800_0000) via `std::os::windows::process::CommandExt::creation_flags` under
//!   `#[cfg(windows)]`. Missing binary → `LaunchError::MissingBinary(path)`.
//! - `wait_until_healthy(health_url, timeout, poll) -> Result<Duration, LaunchError>`: poll GET
//!   `health_url` with ureq (2 s per-request timeout) until HTTP 200, returning elapsed time
//!   (this is the "model loading" time of R-41); `LaunchError::NotReady(timeout)` on timeout.
//!   Accept an optional `&mut Child` and fail fast with `LaunchError::Exited(status)` if the
//!   process dies while waiting.
//! - Unit tests: argument assembly for GPU (Qwen-like) and CPU (LFM-like, `gpu_layers = 0`)
//!   configs, non-loopback rejection, endpoint without port; `wait_until_healthy` against a
//!   `TcpListener` fake that answers 503 twice then 200 (integration test file
//!   `crates/harness-runtime/tests/launcher_wait.rs`).

use std::fmt::Display;
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use harness_core::config::{LaunchConfig, LoadMode, ModelConfig, reserved_launch_flag};
use thiserror::Error;

/// `CREATE_NO_WINDOW`: stops a console-subsystem server from opening its own console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Per-request timeout for health polling.
const HEALTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Failures while assembling, starting or waiting for a local inference server.
#[derive(Debug, Error)]
pub enum LaunchError {
    /// The endpoint is not `http://HOST:PORT[/...]` with a usable host and non-zero port.
    #[error("cannot parse host:port from endpoint {0}")]
    Endpoint(String),
    /// The endpoint names a non-loopback host and the caller did not allow remote binding.
    #[error("refusing to bind a model server to non-loopback host {0}")]
    NonLoopbackBind(String),
    /// `launch.extra_args` holds a flag the launcher owns or one that rebinds or redirects the
    /// server (config rule V22). Checked here too, so a bypassed validation cannot rebind it.
    #[error("launch.extra_args may not contain {0}")]
    ReservedFlag(String),
    /// The inference server executable does not exist.
    #[error("inference server binary not found: {}", .0.display())]
    MissingBinary(PathBuf),
    /// Spawning the server, or opening its log file, failed for another reason.
    #[error("failed to start server: {0}")]
    Spawn(#[from] std::io::Error),
    /// The health endpoint did not return HTTP 200 within the timeout.
    #[error("server did not become healthy within {0:?}")]
    NotReady(Duration),
    /// The server process exited before it became healthy.
    #[error("server exited during start-up: {0}")]
    Exited(ExitStatus),
}

/// Builds the `llama-server` argument vector for one model slot.
///
/// `model_file` must already be resolved against `paths.models_dir`. `allow_remote_bind` must be
/// `true` before a non-loopback endpoint host is accepted.
pub fn server_args(
    model: &ModelConfig,
    launch: &LaunchConfig,
    model_file: &Path,
    allow_remote_bind: bool,
) -> Result<Vec<String>, LaunchError> {
    let (host, port) = parse_endpoint(&model.endpoint)?;
    if !allow_remote_bind && !is_loopback(&host) {
        return Err(LaunchError::NonLoopbackBind(host));
    }
    if let Some(flag) = reserved_launch_flag(&launch.extra_args) {
        return Err(LaunchError::ReservedFlag(flag.to_owned()));
    }

    let mut args = Vec::new();
    push_pair(&mut args, "-m", model_file.to_string_lossy());
    push_pair(&mut args, "--host", &host);
    push_pair(&mut args, "--port", port);
    push_pair(&mut args, "--alias", &model.id);
    push_pair(&mut args, "-c", model.context);
    args.extend(["--jinja".to_owned(), "--metrics".to_owned()]);
    if let Some(layers) = launch.gpu_layers {
        push_pair(&mut args, "-ngl", layers);
    }
    if let Some(threads) = launch.threads {
        push_pair(&mut args, "-t", threads);
    }
    if let Some(on) = launch.flash_attn {
        push_pair(&mut args, "-fa", if on { "on" } else { "off" });
    }
    if let Some(kv) = &model.kv_cache {
        push_pair(&mut args, "-ctk", kv);
        push_pair(&mut args, "-ctv", kv);
    }
    if let Some(mode) = launch.load_mode {
        push_pair(&mut args, "--load-mode", load_mode_arg(mode));
    }
    push_pair(&mut args, "-np", launch.parallel);
    if let Some(mib) = launch.cache_ram_mib {
        push_pair(&mut args, "--cache-ram", mib);
    }
    if let Some(budget) = model.reasoning.budget_tokens {
        push_pair(&mut args, "--reasoning-budget", budget);
    }
    args.extend(launch.extra_args.iter().cloned());
    Ok(args)
}

/// The `--load-mode` value llama-server b11756 accepts for each mode.
fn load_mode_arg(mode: LoadMode) -> &'static str {
    match mode {
        LoadMode::Auto => "auto",
        LoadMode::Mmap => "mmap",
        LoadMode::None => "none",
    }
}

fn push_pair(args: &mut Vec<String>, flag: &str, value: impl Display) {
    args.push(flag.to_owned());
    args.push(value.to_string());
}

/// Splits `http://HOST:PORT[/path]` into the host (without IPv6 brackets) and the port: the
/// address the server is started on.
pub fn parse_endpoint(endpoint: &str) -> Result<(String, u16), LaunchError> {
    let bad = || LaunchError::Endpoint(endpoint.to_owned());
    let rest = endpoint.strip_prefix("http://").ok_or_else(bad)?;
    let authority = rest.split_once('/').map_or(rest, |(auth, _)| auth);
    let (host, port) = match authority.strip_prefix('[') {
        // Bracketed IPv6 literal, e.g. `[::1]:8001`.
        Some(inner) => {
            let (host, after) = inner.split_once(']').ok_or_else(bad)?;
            (host, after.strip_prefix(':').ok_or_else(bad)?)
        }
        None => {
            let (host, port) = authority.rsplit_once(':').ok_or_else(bad)?;
            if host.contains(':') {
                return Err(bad());
            }
            (host, port)
        }
    };
    if host.is_empty() || host.contains('@') {
        return Err(bad());
    }
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(bad)?;
    Ok((host.to_owned(), port))
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback())
}

/// Starts `bin` with `args`, appending its stdout and stderr to `log_file`.
///
/// The parent directory of `log_file` is created when missing. A `NotFound` spawn error is
/// reported as [`LaunchError::MissingBinary`].
pub fn spawn_server(bin: &Path, args: &[String], log_file: &Path) -> Result<Child, LaunchError> {
    if let Some(dir) = log_file.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)?;
    let log_err = log.try_clone()?;

    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().map_err(|err| match err.kind() {
        ErrorKind::NotFound => LaunchError::MissingBinary(bin.to_path_buf()),
        _ => LaunchError::Spawn(err),
    })
}

/// Polls `health_url` until it answers HTTP 200 and returns how long that took.
///
/// Connection errors and non-200 answers are retried every `poll`. If `child` is given and
/// exits first, returns [`LaunchError::Exited`] at once. Gives up with
/// [`LaunchError::NotReady`] after `timeout`. A single request may run up to 2 s past the
/// deadline.
pub fn wait_until_healthy(
    health_url: &str,
    timeout: Duration,
    poll: Duration,
    mut child: Option<&mut Child>,
) -> Result<Duration, LaunchError> {
    let started = Instant::now();
    let agent = crate::http::local_agent(HEALTH_REQUEST_TIMEOUT);
    loop {
        if let Some(server) = child.as_deref_mut()
            && let Some(status) = server.try_wait()?
        {
            return Err(LaunchError::Exited(status));
        }
        if let Ok(response) = agent.get(health_url).call()
            && response.status().as_u16() == 200
        {
            return Ok(started.elapsed());
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(LaunchError::NotReady(timeout));
        }
        thread::sleep(poll.min(timeout.saturating_sub(elapsed)));
    }
}

/// One health request: `true` only when the server answers HTTP 200 now. Nothing is retried.
pub fn is_healthy(health_url: &str) -> bool {
    crate::http::local_agent(HEALTH_REQUEST_TIMEOUT)
        .get(health_url)
        .call()
        .is_ok_and(|response| response.status().as_u16() == 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(endpoint: &str) -> ModelConfig {
        serde_json::from_value(json!({
            "provider": "llama_cpp",
            "id": "test-model",
            "role": "engineer",
            "endpoint": endpoint,
            "context": 4096,
            "hardware": "gpu",
        }))
        .expect("fixture matches the ModelConfig schema")
    }

    fn launch() -> LaunchConfig {
        serde_json::from_value(json!({
            "runtime": "llama_cpp_main",
            "model_file": "model.gguf",
        }))
        .expect("fixture matches the LaunchConfig schema")
    }

    #[test]
    fn endpoint_without_port_is_rejected() {
        let result = server_args(
            &model("http://127.0.0.1/v1"),
            &launch(),
            Path::new("m.gguf"),
            false,
        );
        assert!(matches!(result, Err(LaunchError::Endpoint(_))));
    }

    #[test]
    fn endpoint_must_use_plain_http_with_a_host() {
        for bad in [
            "https://127.0.0.1:8001/v1",
            "127.0.0.1:8001/v1",
            "http://:8001/v1",
            "http://127.0.0.1:0/v1",
            "http://127.0.0.1:port/v1",
            "http://user@127.0.0.1:8001/v1",
        ] {
            let result = parse_endpoint(bad);
            assert!(
                matches!(result, Err(LaunchError::Endpoint(_))),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn ipv6_and_localhost_endpoints_parse() {
        assert_eq!(
            parse_endpoint("http://[::1]:8001/v1").ok(),
            Some(("::1".to_owned(), 8001))
        );
        assert_eq!(
            parse_endpoint("http://localhost:8080").ok(),
            Some(("localhost".to_owned(), 8080))
        );
    }

    #[test]
    fn non_loopback_host_needs_explicit_permission() {
        let endpoint = "http://0.0.0.0:8001/v1";
        let refused = server_args(&model(endpoint), &launch(), Path::new("m.gguf"), false);
        assert!(matches!(
            refused,
            Err(LaunchError::NonLoopbackBind(ref host)) if host == "0.0.0.0"
        ));
        let allowed = server_args(&model(endpoint), &launch(), Path::new("m.gguf"), true);
        assert!(allowed.is_ok());
    }

    #[test]
    fn extra_args_cannot_rebind_the_server_even_without_validation() {
        let endpoint = "http://127.0.0.1:8001/v1";
        for extra in [
            ["--host", "0.0.0.0"],
            ["--port=9", "x"],
            ["--log-file", "x"],
        ] {
            let mut launch = launch();
            launch.extra_args = extra.iter().map(|a| (*a).to_owned()).collect();
            let refused = server_args(&model(endpoint), &launch, Path::new("m.gguf"), false);
            assert!(
                matches!(refused, Err(LaunchError::ReservedFlag(_))),
                "{extra:?}: {refused:?}"
            );
        }
    }

    #[test]
    fn ipv6_loopback_is_passed_unbracketed_to_host_flag() {
        let args = server_args(
            &model("http://[::1]:8001/v1"),
            &launch(),
            Path::new("m.gguf"),
            false,
        )
        .expect("loopback endpoint is accepted");
        let host_at = args.iter().position(|a| a == "--host").expect("--host");
        assert_eq!(args[host_at + 1], "::1");
    }

    #[test]
    fn optional_flags_are_emitted_only_when_configured() {
        let args = server_args(
            &model("http://127.0.0.1:8001/v1"),
            &launch(),
            Path::new("m.gguf"),
            false,
        )
        .expect("loopback endpoint is accepted");
        assert_eq!(
            args,
            [
                "-m",
                "m.gguf",
                "--host",
                "127.0.0.1",
                "--port",
                "8001",
                "--alias",
                "test-model",
                "-c",
                "4096",
                "--jinja",
                "--metrics",
                "-np",
                "1",
            ]
        );
    }

    #[test]
    fn missing_binary_is_reported_with_its_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("no-such-llama-server");
        let result = spawn_server(&bin, &[], &dir.path().join("logs").join("server.log"));
        assert!(matches!(
            result,
            Err(LaunchError::MissingBinary(ref path)) if *path == bin
        ));
    }
}
