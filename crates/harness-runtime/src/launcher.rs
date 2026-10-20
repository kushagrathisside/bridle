//! Starts local inference servers from config (`agent models serve <slot>`).
//!
//! IMPLEMENTATION SPEC (delegated):
//! - `server_args(model, launch, model_file) -> Result<Vec<String>, LaunchError>` (pure, unit
//!   tested): parse host and port from `model.endpoint` (`http://HOST:PORT/v1`; missing port →
//!   `LaunchError::Endpoint`). Emit, in this order:
//!   `-m <model_file> --host <host> --port <port> --alias <model.id> -c <model.context> --jinja
//!   --metrics`, then `-ngl <n>` if `gpu_layers` set, `-t <n>` if `threads` set,
//!   `--flash-attn on|off` if `flash_attn` set, `--no-mmap` if `no_mmap`,
//!   `-ctk <kv> -ctv <kv>` if `model.kv_cache` set, `--reasoning-budget <n>` if
//!   `model.reasoning.budget_tokens` set, then `launch.extra_args` verbatim.
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

use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::time::Duration;

use harness_core::config::{LaunchConfig, ModelConfig};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("cannot parse host:port from endpoint {0}")]
    Endpoint(String),
    #[error("refusing to bind a model server to non-loopback host {0}")]
    NonLoopbackBind(String),
    #[error("inference server binary not found: {0}")]
    MissingBinary(PathBuf),
    #[error("failed to start server: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("server did not become healthy within {0:?}")]
    NotReady(Duration),
    #[error("server exited during start-up: {0}")]
    Exited(ExitStatus),
}

pub fn server_args(
    model: &ModelConfig,
    launch: &LaunchConfig,
    model_file: &Path,
    allow_remote_bind: bool,
) -> Result<Vec<String>, LaunchError> {
    let _ = (model, launch, model_file, allow_remote_bind);
    todo!()
}

pub fn spawn_server(bin: &Path, args: &[String], log_file: &Path) -> Result<Child, LaunchError> {
    let _ = (bin, args, log_file);
    todo!()
}

pub fn wait_until_healthy(
    health_url: &str,
    timeout: Duration,
    poll: Duration,
    child: Option<&mut Child>,
) -> Result<Duration, LaunchError> {
    let _ = (health_url, timeout, poll, child);
    todo!()
}
