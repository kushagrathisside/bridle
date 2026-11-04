//! The browser manager: starts, supervises and stops the browser sessions (ADR-0020).
//!
//! One session is one worker process (Node.js running the Playwright worker) and, below it, one Chromium process with its own
//! temporary profile and downloads folder. Sessions share nothing: not cookies, not storage, not a process. They are named by an
//! unguessable id and belong to the manager that made them, so another MCP client (another process, another manager) cannot name
//! them, and a guessed id fails the same way an expired one does.
//!
//! Everything a worker does is a request on the line protocol with a deadline. A worker that stays silent past its deadline, writes
//! a malformed line, exits, or reports a crash is killed and its session is over; the next call says so, and the model can open a
//! new one. Nothing the worker returns is trusted for a path: Bridle names the folders and checks the file names it is told.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use harness_core::config::BrowserSection;
use serde_json::Value;
use thiserror::Error;

use super::protocol::{
    self, Action, ActionResult, LaunchResult, MAX_LINE, NavigateResult, Op, ShotResult,
    SnapshotResult, WaitUntil,
};
use super::proxy::{EgressProxy, ProxyConfig, Traffic, Upstream};
use super::runtime::{self, Layout, Manifest};
use crate::http::RemoteRoute;

/// How long the worker and Chromium may take to start.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(45);
const READY_TIMEOUT: Duration = Duration::from_secs(20);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
/// What a snapshot or a screenshot may take.
const QUICK_TIMEOUT: Duration = Duration::from_secs(30);
/// How much beyond the download limit one call may move through the proxy (pages, scripts, images).
const CALL_BYTES_ALLOWANCE: u64 = 64 * 1024 * 1024;
/// Session folders older than this are leftovers of a crash.
const STALE_SESSION_DIR: Duration = Duration::from_secs(24 * 60 * 60);
const STDERR_KEPT: usize = 4 * 1024;

/// What the worker said when it refused or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerFailure {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for WorkerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("the browser is not installed or not usable: {0}")]
    NotInstalled(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("the browser could not start: {0}")]
    Start(String),
    #[error("at most {0} browser sessions can be open at once; close one with browser_close first")]
    TooMany(usize),
    #[error(
        "there is no open browser session with that id (it was closed, it timed out, or it never existed)"
    )]
    NoSession,
    #[error("{0}")]
    Worker(WorkerFailure),
    #[error("the browser session ended: {0}")]
    Dead(String),
    #[error("the browser did not answer within {0} seconds; the session was closed")]
    Timeout(u64),
    #[error("the browser worker did not follow the protocol ({0}); the session was closed")]
    Protocol(String),
}

impl BrowserError {
    /// A short stable name for logs.
    pub fn code(&self) -> &str {
        match self {
            Self::NotInstalled(_) => "not_installed",
            Self::Unsupported(_) => "unsupported",
            Self::Start(_) => "start_failed",
            Self::TooMany(_) => "too_many_sessions",
            Self::NoSession => "no_session",
            Self::Worker(w) => &w.code,
            Self::Dead(_) => "session_ended",
            Self::Timeout(_) => "timeout",
            Self::Protocol(_) => "protocol",
        }
    }
}

// ---------------------------------------------------------------------------------------------------------- transport

/// Why a read from the worker gave nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvError {
    Timeout,
    Closed,
    TooLong,
}

/// The pipe to one worker.
pub trait Transport: Send {
    fn send(&mut self, line: &str) -> io::Result<()>;
    fn recv(&mut self, timeout: Duration) -> Result<String, RecvError>;
    /// Ends the worker (and, through it, Chromium). Safe to call twice.
    fn kill(&mut self);
    /// What the worker wrote to its error output lately, for a message when it dies.
    fn diagnostics(&self) -> String;
}

/// Starts workers.
pub trait Spawner: Send + Sync {
    fn spawn(&self, session_dir: &Path) -> io::Result<Box<dyn Transport>>;
}

/// The real thing: Node.js running the worker, with a nearly empty environment.
#[derive(Debug, Clone)]
pub struct ProcessSpawner {
    pub layout: Layout,
}

struct ProcessTransport {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Result<String, RecvError>>,
    stderr: Arc<Mutex<String>>,
}

impl Spawner for ProcessSpawner {
    fn spawn(&self, session_dir: &Path) -> io::Result<Box<dyn Transport>> {
        let temp = session_dir.join("tmp");
        std::fs::create_dir_all(&temp)?;
        let mut command = Command::new(self.layout.node_exe());
        command
            .arg(self.layout.worker_js())
            .current_dir(session_dir);
        command.env_clear();
        for name in [
            "SystemRoot",
            "SystemDrive",
            "windir",
            "ComSpec",
            "PATHEXT",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("TEMP", &temp)
            .env("TMP", &temp)
            .env("BRIDLE_BROWSER_DIR", session_dir)
            .env("BRIDLE_PLAYWRIGHT_DIR", self.layout.playwright_dir())
            .env("PLAYWRIGHT_BROWSERS_PATH", self.layout.browsers_dir());
        crate::process::hide_console(&mut command);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no stdout"))?;
        let stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("no stderr"))?;
        let (sender, replies) = mpsc::channel();
        thread::Builder::new()
            .name("bridle-browser-stdout".to_owned())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let mut line = Vec::new();
                    match (&mut reader)
                        .take(MAX_LINE as u64 + 1)
                        .read_until(b'\n', &mut line)
                    {
                        Ok(0) | Err(_) => {
                            let _ = sender.send(Err(RecvError::Closed));
                            return;
                        }
                        Ok(_) if line.len() > MAX_LINE => {
                            let _ = sender.send(Err(RecvError::TooLong));
                            return;
                        }
                        Ok(_) => {
                            let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                            if sender.send(Ok(text)).is_err() {
                                return;
                            }
                        }
                    }
                }
            })?;
        let kept = Arc::new(Mutex::new(String::new()));
        let kept_writer = Arc::clone(&kept);
        thread::Builder::new()
            .name("bridle-browser-stderr".to_owned())
            .spawn(move || {
                let mut pipe = stderr_pipe;
                let mut buffer = [0u8; 1024];
                while let Ok(n) = pipe.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    let mut text = kept_writer.lock().unwrap_or_else(PoisonError::into_inner);
                    text.push_str(&String::from_utf8_lossy(&buffer[..n]));
                    if text.len() > 2 * STDERR_KEPT {
                        let cut = (text.len() - STDERR_KEPT..text.len())
                            .find(|i| text.is_char_boundary(*i))
                            .unwrap_or(0);
                        *text = text[cut..].to_owned();
                    }
                }
            })?;
        Ok(Box::new(ProcessTransport {
            child,
            stdin,
            replies,
            stderr: kept,
        }))
    }
}

impl Transport for ProcessTransport {
    fn send(&mut self, line: &str) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("the worker is gone"))?;
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    fn recv(&mut self, timeout: Duration) -> Result<String, RecvError> {
        match self.replies.recv_timeout(timeout) {
            Ok(Ok(line)) => Ok(line),
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => Err(RecvError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(RecvError::Closed),
        }
    }

    fn kill(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn diagnostics(&self) -> String {
        let text = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        let tail = text.trim();
        let from = tail.len().saturating_sub(600);
        let from = (from..tail.len())
            .find(|i| tail.is_char_boundary(*i))
            .unwrap_or(0);
        tail[from..].to_owned()
    }
}

impl Drop for ProcessTransport {
    fn drop(&mut self) {
        self.kill();
    }
}

// ----------------------------------------------------------------------------------------------------------- session

struct Worker {
    transport: Box<dyn Transport>,
    next_id: u64,
    ended: Option<String>,
}

/// One open browser.
pub struct Session {
    id: String,
    dir: PathBuf,
    worker: Mutex<Worker>,
    traffic: Arc<Traffic>,
    launched: LaunchResult,
    timeouts: Timeouts,
    call_bytes: u64,
    created: Instant,
    last_used: Mutex<Instant>,
    /// The address the last `navigate` was asked for: the "original" address of what is read next.
    requested_url: Mutex<String>,
}

#[derive(Debug, Clone, Copy)]
struct Timeouts {
    navigation: Duration,
    action: Duration,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("browser", &self.launched.browser_version)
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The folder Bridle made for this session: `downloads/` and `shots/` hold what the worker produced.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn traffic(&self) -> &Arc<Traffic> {
        &self.traffic
    }

    pub fn browser_version(&self) -> &str {
        &self.launched.browser_version
    }

    pub fn playwright_version(&self) -> &str {
        &self.launched.playwright_version
    }

    pub fn age(&self) -> Duration {
        self.created.elapsed()
    }

    pub fn requested_url(&self) -> String {
        self.requested_url
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_requested_url(&self, url: &str) {
        *self
            .requested_url
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = url.to_owned();
    }

    fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
    }

    /// Sends one request and waits for its answer, at most `limit`. Whatever goes wrong ends the session.
    fn call(&self, op: &Op, limit: Duration) -> Result<Value, BrowserError> {
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(why) = &worker.ended {
            return Err(BrowserError::Dead(why.clone()));
        }
        *self
            .last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
        self.traffic.begin_call(self.call_bytes);
        let id = worker.next_id;
        worker.next_id += 1;
        if let Err(e) = worker.transport.send(&op.encode(id)) {
            let why = format!(
                "the worker is gone ({e}){}",
                tail(&worker.transport.diagnostics())
            );
            worker.transport.kill();
            worker.ended = Some(why.clone());
            return Err(BrowserError::Dead(why));
        }
        let deadline = Instant::now() + limit;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match worker.transport.recv(left) {
                Ok(line) => line,
                Err(RecvError::Timeout) => {
                    worker.transport.kill();
                    worker.ended = Some(format!(
                        "no answer to `{}` within {} seconds",
                        op.name(),
                        limit.as_secs()
                    ));
                    return Err(BrowserError::Timeout(limit.as_secs()));
                }
                Err(RecvError::Closed) => {
                    let why = format!(
                        "the worker exited during `{}`{}",
                        op.name(),
                        tail(&worker.transport.diagnostics())
                    );
                    worker.transport.kill();
                    worker.ended = Some(why.clone());
                    return Err(BrowserError::Dead(why));
                }
                Err(RecvError::TooLong) => {
                    return Err(self.protocol_break(&mut worker, "a line was too long"));
                }
            };
            let reply = match protocol::decode(&line) {
                Ok(r) => r,
                Err(e) => return Err(self.protocol_break(&mut worker, &e)),
            };
            if reply.event.is_some() {
                continue;
            }
            if reply.id != Some(id) {
                return Err(self.protocol_break(&mut worker, "an answer to a different request"));
            }
            if reply.ok {
                return Ok(reply.result.unwrap_or(Value::Null));
            }
            let error = reply.error.unwrap_or(protocol::WorkerError {
                code: "internal".to_owned(),
                message: "the worker failed without saying why".to_owned(),
            });
            if error.code == "crashed" {
                worker.transport.kill();
                worker.ended = Some(error.message.clone());
            }
            return Err(BrowserError::Worker(WorkerFailure {
                code: error.code,
                message: error.message,
            }));
        }
    }

    fn protocol_break(&self, worker: &mut Worker, what: &str) -> BrowserError {
        worker.transport.kill();
        worker.ended = Some(format!("the worker broke the protocol: {what}"));
        BrowserError::Protocol(what.to_owned())
    }

    fn typed<T: serde::de::DeserializeOwned>(&self, value: Value) -> Result<T, BrowserError> {
        serde_json::from_value(value).map_err(|_| {
            let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
            self.protocol_break(&mut worker, "an answer of the wrong shape")
        })
    }

    pub fn navigate(
        &self,
        url: &str,
        wait_until: WaitUntil,
    ) -> Result<NavigateResult, BrowserError> {
        let limit = self.timeouts.navigation + Duration::from_secs(10);
        let value = self.call(
            &Op::Navigate {
                url: url.to_owned(),
                wait_until,
            },
            limit,
        )?;
        self.set_requested_url(url);
        self.typed(value)
    }

    pub fn snapshot(
        &self,
        max_html_bytes: u64,
        aria_chars: u64,
    ) -> Result<SnapshotResult, BrowserError> {
        let value = self.call(
            &Op::Snapshot {
                max_html_bytes,
                aria_chars,
            },
            QUICK_TIMEOUT,
        )?;
        self.typed(value)
    }

    pub fn action(&self, action: &Action) -> Result<ActionResult, BrowserError> {
        let timeout_ms = u64::try_from(self.timeouts.action.as_millis()).unwrap_or(10_000);
        let limit = self.timeouts.navigation + self.timeouts.action * 4 + Duration::from_secs(10);
        let value = self.call(
            &Op::Action {
                action: action.clone(),
                timeout_ms,
            },
            limit,
        )?;
        self.typed(value)
    }

    pub fn screenshot(&self) -> Result<ShotResult, BrowserError> {
        let value = self.call(&Op::Screenshot, QUICK_TIMEOUT)?;
        self.typed(value)
    }

    /// Asks the worker to close Chromium, then makes sure it is gone.
    fn terminate(&self) {
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if worker.ended.is_none() {
            let id = worker.next_id;
            worker.next_id += 1;
            if worker.transport.send(&Op::Close.encode(id)).is_ok() {
                let _ = worker.transport.recv(CLOSE_TIMEOUT);
            }
            worker.ended = Some("closed".to_owned());
        }
        worker.transport.kill();
    }
}

fn tail(diagnostics: &str) -> String {
    if diagnostics.is_empty() {
        String::new()
    } else {
        format!(
            ": {}",
            diagnostics.split_whitespace().collect::<Vec<_>>().join(" ")
        )
    }
}

// ---------------------------------------------------------------------------------------------------------- manager

/// What a manager needs to know beyond the browser section.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The largest file a page may download, in MiB (`web.max_download_mb`).
    pub max_download_mb: u64,
}

pub struct BrowserManager {
    config: BrowserSection,
    limits: Limits,
    layout: Layout,
    proxy: EgressProxy,
    spawner: Box<dyn Spawner>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    manifest: Option<Manifest>,
}

impl std::fmt::Debug for BrowserManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserManager")
            .field("max_sessions", &self.config.max_sessions)
            .field("proxy", &self.proxy)
            .finish_non_exhaustive()
    }
}

impl BrowserManager {
    /// Checks the installation (its programs must still be the ones `setup` recorded), starts the egress proxy, and returns a
    /// manager that starts no browser until a session is opened.
    pub fn start(
        config: &BrowserSection,
        limits: Limits,
        route: &RemoteRoute,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, BrowserError> {
        runtime::supported_platform().map_err(BrowserError::Unsupported)?;
        let layout = Layout::default_location().ok_or_else(|| {
            BrowserError::NotInstalled(
                "the machine configuration folder is unknown (set BRIDLE_HOME)".to_owned(),
            )
        })?;
        Self::start_at(layout, config, limits, route, env, None)
    }

    /// [`Self::start`] with a given runtime folder and, for tests and the self-test, hosts that skip the address check.
    pub fn start_at(
        layout: Layout,
        config: &BrowserSection,
        limits: Limits,
        route: &RemoteRoute,
        env: &dyn Fn(&str) -> Option<String>,
        exempt: Option<Vec<(String, u16)>>,
    ) -> Result<Self, BrowserError> {
        let status = runtime::inspect(&layout);
        let manifest = match (&status.state, status.manifest.clone()) {
            (runtime::State::Ready, Some(m)) => m,
            _ => return Err(BrowserError::NotInstalled(status.describe())),
        };
        runtime::verify_identity(&layout, &manifest).map_err(BrowserError::NotInstalled)?;
        let upstream = Upstream::from_route(route, env).map_err(BrowserError::Start)?;
        let mut proxy_config = ProxyConfig::new(upstream);
        proxy_config.exempt = exempt.unwrap_or_default();
        proxy_config.allow_hosts = config.allowed_hosts.clone();
        proxy_config.deny_hosts = config.blocked_hosts.clone();
        proxy_config.allow_unpinned = config.allow_unpinned_destinations;
        let proxy = EgressProxy::start(proxy_config)
            .map_err(|e| BrowserError::Start(format!("the egress proxy: {e}")))?;
        let spawner = Box::new(ProcessSpawner {
            layout: layout.clone(),
        });
        let manager = Self::from_parts(config, limits, layout, proxy, spawner, Some(manifest));
        manager.sweep_stale_folders();
        Ok(manager)
    }

    /// The pieces, for tests that bring their own worker and proxy.
    pub fn from_parts(
        config: &BrowserSection,
        limits: Limits,
        layout: Layout,
        proxy: EgressProxy,
        spawner: Box<dyn Spawner>,
        manifest: Option<Manifest>,
    ) -> Self {
        Self {
            config: config.clone(),
            limits,
            layout,
            proxy,
            spawner,
            sessions: Mutex::new(HashMap::new()),
            manifest,
        }
    }

    pub fn config(&self) -> &BrowserSection {
        &self.config
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn proxy(&self) -> &EgressProxy {
        &self.proxy
    }

    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }

    /// Removes the session folders of Bridle processes that are gone (a crash, a kill): a folder is named `<pid>-<session id>`, and one
    /// whose process no longer runs is removed; so is any folder older than a day. A folder of a live process, this one included, is
    /// never touched.
    fn sweep_stale_folders(&self) {
        sweep_session_folders(&self.layout.sessions_dir());
    }

    /// Closes sessions that have not been used for `session_idle_secs`.
    pub fn reap_idle(&self) {
        let limit = Duration::from_secs(self.config.session_idle_secs);
        let idle: Vec<String> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|s| s.worker.try_lock().is_ok() && s.idle_for() > limit)
            .map(|s| s.id.clone())
            .collect();
        for id in idle {
            self.close(&id);
        }
    }

    pub fn open_count(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Opens a new session: a worker, a Chromium behind the egress proxy, a folder of its own.
    pub fn open(&self) -> Result<Arc<Session>, BrowserError> {
        self.reap_idle();
        if self.open_count() >= self.config.max_sessions {
            return Err(BrowserError::TooMany(self.config.max_sessions));
        }
        let id = format!("br-{}", super::random_hex(8));
        let dir = self
            .layout
            .sessions_dir()
            .join(format!("{}-{id}", std::process::id()));
        std::fs::create_dir_all(dir.join("downloads"))
            .map_err(|e| BrowserError::Start(format!("cannot create the session folder: {e}")))?;
        std::fs::create_dir_all(dir.join("shots"))
            .map_err(|e| BrowserError::Start(format!("cannot create the session folder: {e}")))?;
        let started = (|| -> Result<Arc<Session>, BrowserError> {
            let mut transport = self
                .spawner
                .spawn(&dir)
                .map_err(|e| BrowserError::Start(format!("cannot start the worker: {e}")))?;
            let fail = |transport: &mut Box<dyn Transport>, what: String| {
                let more = tail(&transport.diagnostics());
                transport.kill();
                BrowserError::Start(format!("{what}{more}"))
            };
            // The worker says it is ready, and which Playwright it is: the one that was installed.
            match transport.recv(READY_TIMEOUT) {
                Ok(line) => match protocol::decode(&line) {
                    Ok(r) if r.event.as_deref() == Some("ready") => {
                        if r.playwright_version.as_deref() != Some(runtime::PLAYWRIGHT_VERSION)
                            && self.manifest.is_some()
                        {
                            return Err(fail(
                                &mut transport,
                                "the worker is not running the pinned Playwright".to_owned(),
                            ));
                        }
                    }
                    _ => {
                        return Err(fail(
                            &mut transport,
                            "the worker did not start as expected".to_owned(),
                        ));
                    }
                },
                Err(_) => {
                    return Err(fail(
                        &mut transport,
                        "the worker did not report ready".to_owned(),
                    ));
                }
            }
            let (secret, traffic) = self.proxy.register(&id);
            let launch = Op::Launch {
                proxy_server: self.proxy.server_url(),
                proxy_user: id.clone(),
                proxy_secret: secret,
                navigation_timeout_ms: self.config.navigation_timeout_ms,
                action_timeout_ms: self.config.action_timeout_ms,
                downloads: self.config.downloads_enabled,
            };
            traffic.begin_call(CALL_BYTES_ALLOWANCE);
            if let Err(e) = transport.send(&launch.encode(1)) {
                self.proxy.unregister(&id);
                return Err(fail(
                    &mut transport,
                    format!("cannot talk to the worker: {e}"),
                ));
            }
            let launched: LaunchResult = match transport.recv(LAUNCH_TIMEOUT) {
                Ok(line) => match protocol::decode(&line) {
                    Ok(r) if r.ok && r.id == Some(1) => {
                        match serde_json::from_value(r.result.unwrap_or(Value::Null)) {
                            Ok(l) => l,
                            Err(_) => {
                                self.proxy.unregister(&id);
                                return Err(fail(
                                    &mut transport,
                                    "the worker's launch answer was malformed".to_owned(),
                                ));
                            }
                        }
                    }
                    Ok(r) => {
                        self.proxy.unregister(&id);
                        let message = r
                            .error
                            .map_or_else(|| "it said no".to_owned(), |e| e.message);
                        return Err(fail(
                            &mut transport,
                            format!("Chromium did not start: {message}"),
                        ));
                    }
                    Err(e) => {
                        self.proxy.unregister(&id);
                        return Err(fail(&mut transport, e));
                    }
                },
                Err(RecvError::Timeout) => {
                    self.proxy.unregister(&id);
                    return Err(fail(
                        &mut transport,
                        format!(
                            "Chromium did not start within {} seconds",
                            LAUNCH_TIMEOUT.as_secs()
                        ),
                    ));
                }
                Err(_) => {
                    self.proxy.unregister(&id);
                    return Err(fail(
                        &mut transport,
                        "the worker exited while starting Chromium".to_owned(),
                    ));
                }
            };
            let call_bytes = (self.limits.max_download_mb.saturating_mul(1024 * 1024))
                .saturating_add(CALL_BYTES_ALLOWANCE);
            Ok(Arc::new(Session {
                id: id.clone(),
                dir: dir.clone(),
                worker: Mutex::new(Worker {
                    transport,
                    next_id: 2,
                    ended: None,
                }),
                traffic,
                launched,
                timeouts: Timeouts {
                    navigation: Duration::from_millis(self.config.navigation_timeout_ms),
                    action: Duration::from_millis(self.config.action_timeout_ms),
                },
                call_bytes,
                created: Instant::now(),
                last_used: Mutex::new(Instant::now()),
                requested_url: Mutex::new(String::new()),
            }))
        })();
        match started {
            Ok(session) => {
                self.sessions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(id, Arc::clone(&session));
                Ok(session)
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    /// The session with this id. An id that was never issued, was closed, or timed out gives the same answer.
    pub fn get(&self, id: &str) -> Result<Arc<Session>, BrowserError> {
        self.reap_idle();
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .ok_or(BrowserError::NoSession)
    }

    /// Ends a session: Chromium is closed, the proxy forgets it, its folder is removed. False when there was no such session.
    pub fn close(&self, id: &str) -> bool {
        let Some(session) = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
        else {
            return false;
        };
        self.proxy.unregister(id);
        session.terminate();
        remove_folder(&session.dir);
        true
    }

    pub fn close_all(&self) {
        let ids: Vec<String> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        for id in ids {
            self.close(&id);
        }
    }

    /// The ids of the open sessions.
    pub fn session_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        ids.sort();
        ids
    }
}

/// Removes the session folders in `sessions_dir` whose process is gone or that are over a day old, and returns how many it removed. A
/// folder of a live process, this one included, is never touched. (`bridle doctor --repair` calls this too.)
pub fn sweep_session_folders(sessions_dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return 0;
    };
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let owner = name.split('-').next().and_then(|p| p.parse::<u32>().ok());
        let owner_gone = owner.is_some_and(|pid| {
            pid != std::process::id() && system.process(sysinfo::Pid::from_u32(pid)).is_none()
        });
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_SESSION_DIR);
        if owner_gone || old {
            remove_folder(&entry.path());
            removed += 1;
        }
    }
    removed
}

impl Drop for BrowserManager {
    fn drop(&mut self) {
        self.close_all();
        self.proxy.stop();
    }
}

/// Removes a session folder; Chromium may take a moment to let go of its files.
fn remove_folder(dir: &Path) {
    for _ in 0..10 {
        if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod fake {
    //! A worker that is a closure, for tests: no Node, no Chromium.
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) enum Say {
        Ok(Value),
        Fail(&'static str, &'static str),
        Silent,
        Exit,
        Raw(String),
    }

    pub(crate) type Script = dyn Fn(&Value, &Path) -> Say + Send + Sync;

    pub(crate) struct FakeSpawner {
        pub(crate) script: Arc<Script>,
        pub(crate) spawned: Arc<AtomicUsize>,
        pub(crate) killed: Arc<AtomicUsize>,
        pub(crate) ready: Arc<Mutex<Option<String>>>,
    }

    struct FakeTransport {
        script: Arc<Script>,
        dir: PathBuf,
        queue: std::collections::VecDeque<Result<String, RecvError>>,
        killed: Arc<AtomicUsize>,
        dead: bool,
    }

    impl FakeSpawner {
        pub(crate) fn new(script: impl Fn(&Value, &Path) -> Say + Send + Sync + 'static) -> Self {
            Self {
                script: Arc::new(script),
                spawned: Arc::new(AtomicUsize::new(0)),
                killed: Arc::new(AtomicUsize::new(0)),
                ready: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl Spawner for FakeSpawner {
        fn spawn(&self, session_dir: &Path) -> io::Result<Box<dyn Transport>> {
            self.spawned.fetch_add(1, Ordering::SeqCst);
            let mut queue = std::collections::VecDeque::new();
            let ready = self.ready.lock().unwrap().clone().unwrap_or_else(|| {
                format!(
                    r#"{{"v":1,"event":"ready","playwright_version":"{}"}}"#,
                    runtime::PLAYWRIGHT_VERSION
                )
            });
            queue.push_back(Ok(ready));
            Ok(Box::new(FakeTransport {
                script: Arc::clone(&self.script),
                dir: session_dir.to_path_buf(),
                queue,
                killed: Arc::clone(&self.killed),
                dead: false,
            }))
        }
    }

    impl Transport for FakeTransport {
        fn send(&mut self, line: &str) -> io::Result<()> {
            if self.dead {
                return Err(io::Error::other("closed"));
            }
            let request: Value = serde_json::from_str(line).expect("Bridle sends valid JSON");
            let id = request["id"].clone();
            match (self.script)(&request, &self.dir) {
                Say::Ok(result) => self
                    .queue
                    .push_back(Ok(serde_json::json!({"v": 1, "id": id, "ok": true, "result": result}).to_string())),
                Say::Fail(code, message) => self.queue.push_back(Ok(
                    serde_json::json!({"v": 1, "id": id, "ok": false, "error": {"code": code, "message": message}}).to_string(),
                )),
                Say::Silent => {}
                Say::Exit => self.queue.push_back(Err(RecvError::Closed)),
                Say::Raw(text) => self.queue.push_back(Ok(text)),
            }
            Ok(())
        }

        fn recv(&mut self, timeout: Duration) -> Result<String, RecvError> {
            match self.queue.pop_front() {
                Some(item) => item,
                None => {
                    // Nothing is coming: behave like a silent worker, without making the test wait the whole deadline.
                    thread::sleep(timeout.min(Duration::from_millis(30)));
                    Err(RecvError::Timeout)
                }
            }
        }

        fn kill(&mut self) {
            if !self.dead {
                self.dead = true;
                self.killed.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn diagnostics(&self) -> String {
            "fake worker log".to_owned()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::fake::{FakeSpawner, Say};
    use super::*;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    fn launched() -> Value {
        json!({"browser_version": "156.0.0.0", "playwright_version": runtime::PLAYWRIGHT_VERSION})
    }

    fn config() -> BrowserSection {
        BrowserSection {
            enabled: true,
            max_sessions: 2,
            ..BrowserSection::default()
        }
    }

    fn manager(dir: &Path, config: &BrowserSection, spawner: FakeSpawner) -> BrowserManager {
        let proxy = EgressProxy::start(ProxyConfig::new(Upstream::Direct)).unwrap();
        BrowserManager::from_parts(
            config,
            Limits {
                max_download_mb: 10,
            },
            Layout::at(dir.join("browser-runtime")),
            proxy,
            Box::new(spawner),
            None,
        )
    }

    fn ordinary(request: &Value, _: &Path) -> Say {
        match request["op"].as_str().unwrap() {
            "launch" => Say::Ok(launched()),
            "navigate" => Say::Ok(json!({
                "status": 200, "url": request["args"]["url"], "title": "A page", "redirects": 0, "ready_state": "interactive"
            })),
            "close" => Say::Ok(json!({})),
            other => Say::Fail(
                "invalid",
                Box::leak(format!("unexpected {other}").into_boxed_str()),
            ),
        }
    }

    #[test]
    fn a_session_is_opened_used_and_closed_and_its_folder_and_proxy_access_go_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(ordinary);
        let killed = Arc::clone(&spawner.killed);
        let m = manager(dir.path(), &config(), spawner);
        let session = m.open().unwrap();
        assert!(
            session.id().starts_with("br-") && session.id().len() == 19,
            "{}",
            session.id()
        );
        assert_eq!(session.browser_version(), "156.0.0.0");
        assert!(session.dir().join("downloads").is_dir() && session.dir().join("shots").is_dir());
        let nav = session
            .navigate("https://example.org/", WaitUntil::DomContentLoaded)
            .unwrap();
        assert_eq!(nav.title, "A page");
        assert_eq!(session.requested_url(), "https://example.org/");
        assert_eq!(m.open_count(), 1);
        let folder = session.dir().to_path_buf();
        assert!(m.close(session.id()));
        assert!(!folder.exists());
        assert_eq!(m.open_count(), 0);
        assert_eq!(killed.load(Ordering::SeqCst), 1);
        assert!(!m.close(session.id()), "a second close finds nothing");
        assert!(matches!(m.get(session.id()), Err(BrowserError::NoSession)));
    }

    #[test]
    fn ids_cannot_be_guessed_and_a_wrong_id_looks_like_an_expired_one() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path(), &config(), FakeSpawner::new(ordinary));
        let a = m.open().unwrap();
        let b = m.open().unwrap();
        assert_ne!(a.id(), b.id());
        for guess in [
            "br-0000000000000000",
            "br-",
            "",
            "../br-x",
            a.id().to_uppercase().as_str(),
            "s-1",
        ] {
            assert!(
                matches!(m.get(guess), Err(BrowserError::NoSession)),
                "{guess}"
            );
            assert!(!m.close(guess));
        }
        assert_eq!(m.open_count(), 2, "wrong ids closed nothing");
        // Another manager (another MCP client) does not know these ids at all.
        let other = manager(dir.path(), &config(), FakeSpawner::new(ordinary));
        assert!(matches!(other.get(a.id()), Err(BrowserError::NoSession)));
    }

    #[test]
    fn the_session_limit_holds_and_closing_frees_a_place() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path(), &config(), FakeSpawner::new(ordinary));
        let first = m.open().unwrap();
        let _second = m.open().unwrap();
        assert!(matches!(m.open(), Err(BrowserError::TooMany(2))));
        m.close(first.id());
        assert!(m.open().is_ok());
    }

    #[test]
    fn sessions_are_separate_folders_and_separate_workers() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(ordinary);
        let spawned = Arc::clone(&spawner.spawned);
        let m = manager(dir.path(), &config(), spawner);
        let a = m.open().unwrap();
        let b = m.open().unwrap();
        assert_eq!(spawned.load(Ordering::SeqCst), 2);
        assert_ne!(a.dir(), b.dir());
        assert!(!Arc::ptr_eq(a.traffic(), b.traffic()));
    }

    #[test]
    fn a_worker_that_stays_silent_is_killed_and_the_session_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(|request, _| match request["op"].as_str().unwrap() {
            "launch" => Say::Ok(launched()),
            _ => Say::Silent,
        });
        let killed = Arc::clone(&spawner.killed);
        let mut cfg = config();
        cfg.navigation_timeout_ms = 1_000;
        let m = manager(dir.path(), &cfg, spawner);
        let session = m.open().unwrap();
        let started = Instant::now();
        let error = session
            .navigate("https://example.org/", WaitUntil::Load)
            .unwrap_err();
        assert!(matches!(error, BrowserError::Timeout(_)), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(killed.load(Ordering::SeqCst), 1);
        let again = session
            .navigate("https://example.org/", WaitUntil::Load)
            .unwrap_err();
        assert!(matches!(again, BrowserError::Dead(_)), "{again}");
        assert!(again.to_string().contains("no answer"), "{again}");
    }

    #[test]
    fn a_worker_that_exits_or_breaks_the_protocol_ends_the_session() {
        let dir = tempfile::tempdir().unwrap();
        for (what, expect_dead) in [
            ("exit", true),
            ("garbage", false),
            ("wrong-id", false),
            ("wrong-shape", false),
        ] {
            let spawner =
                FakeSpawner::new(
                    move |request, _| match (request["op"].as_str().unwrap(), what) {
                        ("launch", _) => Say::Ok(launched()),
                        (_, "exit") => Say::Exit,
                        (_, "garbage") => Say::Raw("this is not json".to_owned()),
                        (_, "wrong-id") => {
                            Say::Raw(r#"{"v":1,"id":999,"ok":true,"result":{}}"#.to_owned())
                        }
                        _ => Say::Ok(json!({"unexpected": true})),
                    },
                );
            let m = manager(dir.path(), &config(), spawner);
            let session = m.open().unwrap();
            let error = session
                .navigate("https://example.org/", WaitUntil::Load)
                .unwrap_err();
            if expect_dead {
                assert!(matches!(error, BrowserError::Dead(_)), "{what}: {error}");
            } else {
                assert!(
                    matches!(error, BrowserError::Protocol(_)),
                    "{what}: {error}"
                );
            }
            // The session is over either way, and says so.
            assert!(
                session
                    .navigate("https://example.org/", WaitUntil::Load)
                    .is_err(),
                "{what}"
            );
        }
    }

    #[test]
    fn a_browser_crash_reported_by_the_worker_ends_the_session_but_an_ordinary_error_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(|request, _| match request["op"].as_str().unwrap() {
            "launch" => Say::Ok(launched()),
            "navigate" if request["args"]["url"] == "https://crash.test/" => {
                Say::Fail("crashed", "the page crashed")
            }
            "navigate" => Say::Fail("navigation", "net::ERR_NAME_NOT_RESOLVED"),
            _ => Say::Ok(json!({})),
        });
        let m = manager(dir.path(), &config(), spawner);
        let session = m.open().unwrap();
        let e = session
            .navigate("https://nowhere.test/", WaitUntil::Load)
            .unwrap_err();
        assert!(
            matches!(&e, BrowserError::Worker(w) if w.code == "navigation"),
            "{e}"
        );
        // Still usable.
        assert!(
            session
                .navigate("https://nowhere.test/", WaitUntil::Load)
                .is_err()
        );
        let crash = session
            .navigate("https://crash.test/", WaitUntil::Load)
            .unwrap_err();
        assert_eq!(crash.code(), "crashed");
        assert!(matches!(
            session.navigate("https://x.test/", WaitUntil::Load),
            Err(BrowserError::Dead(_))
        ));
    }

    #[test]
    fn a_worker_that_fails_to_launch_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(|request, _| match request["op"].as_str().unwrap() {
            "launch" => Say::Fail("internal", "Executable doesn't exist"),
            _ => Say::Ok(json!({})),
        });
        let killed = Arc::clone(&spawner.killed);
        let m = manager(dir.path(), &config(), spawner);
        let error = m.open().unwrap_err();
        assert!(
            matches!(&error, BrowserError::Start(why) if why.contains("Executable")),
            "{error}"
        );
        assert_eq!(m.open_count(), 0);
        assert_eq!(killed.load(Ordering::SeqCst), 1);
        let leftovers = std::fs::read_dir(dir.path().join("browser-runtime").join("sessions"))
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn a_worker_that_is_not_the_pinned_playwright_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(ordinary);
        *spawner.ready.lock().unwrap() =
            Some(r#"{"v":1,"event":"ready","playwright_version":"0.0.1"}"#.to_owned());
        // `from_parts` with a manifest present means "an installed runtime", so the pin is enforced.
        let proxy = EgressProxy::start(ProxyConfig::new(Upstream::Direct)).unwrap();
        let layout = Layout::at(dir.path().join("browser-runtime"));
        let manifest: Manifest = serde_json::from_value(json!({
            "schema": 1, "status": "complete", "installed_at": "t", "platform": "p", "node_version": "n",
            "node_archive_sha256": "a", "node_exe_sha256": "b", "playwright_version": runtime::PLAYWRIGHT_VERSION,
            "playwright_integrity": "i", "chromium_revision": "r", "chromium_version": "v", "chromium_executable": "e",
            "chromium_executable_sha256": "c", "chromium_source": "s", "worker_sha256": "w", "protocol": 1
        }))
        .unwrap();
        let m = BrowserManager::from_parts(
            &config(),
            Limits {
                max_download_mb: 10,
            },
            layout,
            proxy,
            Box::new(spawner),
            Some(manifest),
        );
        assert!(
            matches!(m.open().unwrap_err(), BrowserError::Start(why) if why.contains("pinned"))
        );
    }

    #[test]
    fn idle_sessions_are_closed_and_busy_ones_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config();
        cfg.session_idle_secs = 60;
        let m = manager(dir.path(), &cfg, FakeSpawner::new(ordinary));
        let session = m.open().unwrap();
        m.reap_idle();
        assert_eq!(m.open_count(), 1, "just used");
        *session.last_used.lock().unwrap() = Instant::now()
            .checked_sub(Duration::from_secs(120))
            .unwrap();
        {
            // A call in flight holds the worker: it is not reaped from under it.
            let _busy = session.worker.lock().unwrap();
            m.reap_idle();
            assert_eq!(m.open_count(), 1);
        }
        m.reap_idle();
        assert_eq!(m.open_count(), 0);
        assert!(matches!(m.get(session.id()), Err(BrowserError::NoSession)));
    }

    #[test]
    fn folders_of_processes_that_are_gone_are_swept_and_live_ones_are_left() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path(), &config(), FakeSpawner::new(ordinary));
        let sessions = dir.path().join("browser-runtime").join("sessions");
        let mine = m.open().unwrap();
        let dead = sessions.join("4294967-br-deadbeefdeadbeef");
        std::fs::create_dir_all(dead.join("downloads")).unwrap();
        std::fs::write(dead.join("downloads").join("dl-1.bin"), b"left behind").unwrap();
        let unrelated = sessions.join("not-a-session-folder");
        std::fs::create_dir_all(&unrelated).unwrap();
        m.sweep_stale_folders();
        assert!(
            !dead.exists(),
            "the folder of a process that is gone is removed"
        );
        assert!(
            mine.dir().exists(),
            "this process's own session is not touched"
        );
        assert!(
            unrelated.exists(),
            "a folder that is not ours and not a day old is left alone"
        );
    }

    #[test]
    fn dropping_the_manager_closes_every_session() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(ordinary);
        let killed = Arc::clone(&spawner.killed);
        let m = manager(dir.path(), &config(), spawner);
        let a = m.open().unwrap();
        let _b = m.open().unwrap();
        let folder = a.dir().to_path_buf();
        drop(m);
        assert_eq!(killed.load(Ordering::SeqCst), 2);
        assert!(!folder.exists());
    }

    #[test]
    fn concurrent_sessions_each_get_their_own_answers() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(|request, _| match request["op"].as_str().unwrap() {
            "launch" => Say::Ok(launched()),
            "navigate" => Say::Ok(
                json!({"status": 200, "url": request["args"]["url"], "title": request["args"]["url"], "redirects": 0, "ready_state": "complete"}),
            ),
            _ => Say::Ok(json!({})),
        });
        let m = Arc::new(manager(dir.path(), &config(), spawner));
        let sessions = [m.open().unwrap(), m.open().unwrap()];
        thread::scope(|scope| {
            for (n, session) in sessions.iter().enumerate() {
                scope.spawn(move || {
                    for i in 0..20 {
                        let url = format!("https://s{n}.test/{i}");
                        let nav = session.navigate(&url, WaitUntil::Commit).unwrap();
                        assert_eq!(nav.title, url);
                    }
                });
            }
        });
    }
}
