//! Connects the window to the harness. It owns no policy and no agent logic: a chat turn, a model
//! start and a refresh are the jobs the terminal UI already runs (`tui::jobs`), and every message they
//! produce is forwarded to the window as an event named `bridle`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use harness_cli::chat::Agent;
use harness_cli::commands::init;
use harness_cli::commands::models;
use harness_cli::tui::chat::{ChatMsg, Turn};
use harness_cli::tui::data::{LoadArgs, Snapshot};
use harness_cli::tui::jobs::{self, Job, JobMsg};
use harness_core::security::redact::Redactor;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::view;

/// The name of the one event the window listens to.
const EVENT: &str = "bridle";
/// Folders kept in the list of recent projects.
const RECENT_LIMIT: usize = 10;
/// Bytes of `git diff` the window receives at most.
const DIFF_LIMIT: usize = 400_000;

/// The conversation and what is running now.
#[derive(Debug, Default)]
pub(crate) struct Inner {
    /// The conversation. It is `None` before the first message, while a turn runs (the turn owns it),
    /// and after a crash.
    pub(crate) agent: Option<Box<Agent>>,
    /// The conversation of each member of the team who has worked in this window, by member handle.
    pub(crate) members: BTreeMap<String, Box<Agent>>,
    /// The member and the issue of the turn that runs now, when it is a member's turn.
    pub(crate) active: Option<Active>,
    /// The project lock and the checkpoint of the member's turn that runs now.
    pub(crate) guard: Option<harness_cli::gitlayer::RunGuard>,
    pub(crate) running: bool,
    pub(crate) cancel: Arc<AtomicBool>,
}

/// A member's turn in progress.
#[derive(Debug, Clone)]
pub(crate) struct Active {
    pub(crate) member: String,
    pub(crate) issue: String,
    /// The lane the turn works in (ADR-0023), when it does not work in the project's own folder.
    pub(crate) lane: Option<String>,
}

#[derive(Debug)]
pub struct Bridge {
    /// Which configuration and folder the window works on. `switch_workspace` changes the folder.
    args: Mutex<LoadArgs>,
    pub(crate) tx: Sender<JobMsg>,
    pub(crate) inner: Mutex<Inner>,
    /// Every server this window started or is starting, by folder and slot, with the arguments that reach that folder.
    /// Recorded when the start is requested, not when it finishes, so a server that is still loading can be stopped too.
    /// A record is removed only by an explicit stop: a start that fails or is cut short leaves the record, and stopping a
    /// server that is not running does nothing.
    started: Mutex<BTreeMap<(PathBuf, String), LoadArgs>>,
    terminal: Terminal,
}

pub(crate) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Bridge {
    /// A copy of the current load arguments.
    pub fn load_args(&self) -> LoadArgs {
        self.args()
    }

    pub(crate) fn args(&self) -> LoadArgs {
        locked(&self.args).clone()
    }

    pub fn new(args: LoadArgs, tx: Sender<JobMsg>) -> Self {
        Self {
            args: Mutex::new(args),
            tx,
            inner: Mutex::new(Inner::default()),
            started: Mutex::new(BTreeMap::new()),
            terminal: Terminal::default(),
        }
    }

    /// Stops the model servers this window started, so closing Bridle gives the GPU memory back.
    pub fn shutdown(&self) {
        stop_terminal(self);
        let started: Vec<((PathBuf, String), LoadArgs)> = locked(&self.started)
            .iter()
            .map(|(key, args)| (key.clone(), args.clone()))
            .collect();
        for ((_, slot), args) in started {
            stop_server_in(&args, &slot);
        }
    }

    /// Stops the servers recorded for folders other than the current one. A start that was still loading when the window
    /// moved on finishes after the switch; this is what stops it.
    fn stop_other_folders(&self) {
        let Ok(current) = workspace_of(&self.args()) else {
            return;
        };
        let others: Vec<((PathBuf, String), LoadArgs)> = locked(&self.started)
            .iter()
            .filter(|((folder, _), _)| *folder != current)
            .map(|(key, args)| (key.clone(), args.clone()))
            .collect();
        for ((_, slot), args) in others {
            stop_server_in(&args, &slot);
        }
    }
}

/// Forwards what the jobs report to the window. Runs on its own thread until the app exits.
pub fn pump(rx: Receiver<JobMsg>, handle: &AppHandle) {
    for message in rx {
        let bridge = handle.state::<Bridge>();
        match message {
            JobMsg::Chat(ChatMsg::Event(event)) => {
                let mut payload = view::chat_event(&event);
                if let Some(active) = locked(&bridge.inner).active.clone() {
                    payload["member"] = json!(active.member);
                }
                emit(handle, &payload);
            }
            JobMsg::Chat(ChatMsg::Finished { agent, end }) => {
                let answer = agent.as_ref().and_then(|a| a.last_answer());
                let (active, guard) = {
                    let mut inner = locked(&bridge.inner);
                    let active = inner.active.take();
                    let guard = inner.guard.take();
                    match (&active, agent) {
                        // A conversation that worked in a lane belongs to that folder, so it is not kept for the next issue.
                        (Some(a), Some(agent)) if a.lane.is_none() => {
                            inner.members.insert(a.member.clone(), agent);
                        }
                        (Some(_), _) => {}
                        (None, agent) => inner.agent = agent,
                    }
                    inner.running = false;
                    (active, guard)
                };
                let mut payload = view::turn_end(&end);
                if let Some(active) = &active {
                    crate::crew_cmd::finish_member_turn(
                        &bridge,
                        active,
                        &end,
                        answer.as_deref(),
                        guard,
                    );
                    payload["member"] = json!(active.member);
                    payload["issue"] = json!(active.issue);
                }
                emit(handle, &payload);
                refresh(&bridge);
            }
            JobMsg::Chat(ChatMsg::Failed(reason)) => {
                let (active, guard) = {
                    let mut inner = locked(&bridge.inner);
                    let active = inner.active.take();
                    let guard = inner.guard.take();
                    match &active {
                        Some(a) => {
                            inner.members.remove(&a.member);
                        }
                        None => inner.agent = None,
                    }
                    inner.running = false;
                    (active, guard)
                };
                if let Some(active) = &active {
                    crate::crew_cmd::finish_member_turn(
                        &bridge,
                        active,
                        &harness_cli::chat::TurnEnd::Failed(reason.clone()),
                        None,
                        guard,
                    );
                }
                let mut payload = json!({"type": "turn_end", "end": "failed", "message": reason});
                if let Some(active) = &active {
                    payload["member"] = json!(active.member);
                    payload["issue"] = json!(active.issue);
                }
                emit(handle, &payload);
            }
            JobMsg::Snapshot(snapshot) => {
                emit(
                    handle,
                    &json!({"type": "snapshot", "snapshot": view::snapshot(&snapshot)}),
                );
            }
            JobMsg::Doctor {
                items,
                blocking,
                at,
            } => emit(handle, &view::doctor(&items, blocking, &at)),
            JobMsg::Health { slot, health } => emit(
                handle,
                &view::health(&slot, health.status, &health.checks, &health.at),
            ),
            JobMsg::Finished { job, outcome } => {
                if matches!(job, Job::Start { .. }) {
                    bridge.stop_other_folders();
                }
                if job.is_refresh() {
                    continue;
                }
                let (ok, text) = match outcome {
                    Ok(text) => (true, text),
                    Err(text) => (false, text),
                };
                emit(
                    handle,
                    &json!({"type": "job", "label": job.label(), "ok": ok, "message": text}),
                );
                refresh(&bridge);
            }
        }
    }
}

/// Stops a slot's server in the folder `args` names. A server that is not running is not an error.
fn stop_server_in(args: &LoadArgs, slot: &str) {
    if let Ok(app) = args.load_app() {
        let _ = models::stop_server(&app, slot);
    }
}

pub(crate) fn emit(handle: &AppHandle, payload: &Value) {
    let _ = handle.emit(EVENT, payload);
}

fn refresh(bridge: &Bridge) {
    jobs::spawn(Job::Refresh, bridge.args(), bridge.tx.clone());
}

/// The current state of the workspace, the model slots, the tasks and the event log.
#[tauri::command(async)]
pub fn get_snapshot(bridge: State<'_, Bridge>) -> Value {
    view::snapshot(&Snapshot::collect(&bridge.args()))
}

/// Starts a turn. The answer streams back as `bridle` events; the last one is `turn_end`.
#[tauri::command]
pub fn send_message(bridge: State<'_, Bridge>, text: String, slot: String) -> Result<(), String> {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err("the message is empty".to_owned());
    }
    let turn = {
        let mut inner = locked(&bridge.inner);
        if inner.running {
            return Err("a turn is already running; stop it or wait for it to finish".to_owned());
        }
        inner.running = true;
        inner.cancel = Arc::new(AtomicBool::new(false));
        Turn {
            text,
            slot,
            agent: inner.agent.take(),
            cancel: Arc::clone(&inner.cancel),
        }
    };
    jobs::spawn_chat(turn, bridge.args(), bridge.tx.clone());
    Ok(())
}

/// Asks the running turn to stop after the step in progress.
#[tauri::command]
pub fn stop_turn(bridge: State<'_, Bridge>) {
    locked(&bridge.inner).cancel.store(true, Ordering::Relaxed);
}

/// Forgets the conversation. The next message starts a new one.
#[tauri::command]
pub fn new_chat(bridge: State<'_, Bridge>) -> Result<(), String> {
    let mut inner = locked(&bridge.inner);
    if inner.running {
        return Err("stop the running turn first".to_owned());
    }
    inner.agent = None;
    inner.members.clear();
    Ok(())
}

#[tauri::command]
pub fn refresh_snapshot(bridge: State<'_, Bridge>) {
    refresh(&bridge);
}

#[tauri::command]
pub fn start_model(bridge: State<'_, Bridge>, slot: String) {
    let args = bridge.args();
    if let Ok(folder) = workspace_of(&args) {
        locked(&bridge.started).insert((folder, slot.clone()), args.clone());
    }
    jobs::spawn(Job::Start { slot }, args, bridge.tx.clone());
}

#[tauri::command]
pub fn stop_model(bridge: State<'_, Bridge>, slot: String) {
    if let Ok(folder) = workspace_of(&bridge.args()) {
        locked(&bridge.started).remove(&(folder, slot.clone()));
    }
    jobs::spawn(Job::Stop { slot }, bridge.args(), bridge.tx.clone());
}

#[tauri::command]
pub fn run_doctor(bridge: State<'_, Bridge>) {
    jobs::spawn(Job::Doctor, bridge.args(), bridge.tx.clone());
}

#[tauri::command]
pub fn run_health(bridge: State<'_, Bridge>, slot: Option<String>) {
    jobs::spawn(
        Job::Health { slot, quick: true },
        bridge.args(),
        bridge.tx.clone(),
    );
}

/// The tools the model is offered, for the `/` menu. The commands of the menu are the window's own.
#[tauri::command(async)]
pub fn list_commands(bridge: State<'_, Bridge>) -> Value {
    let tools = match bridge.args().load_app() {
        Ok(app) => harness_cli::chat::tool_catalog(&app),
        Err(_) => Vec::new(),
    };
    json!({
        "tools": tools.iter().map(|(name, description)| json!({
            "name": name,
            "description": description.lines().next().unwrap_or("").trim(),
        })).collect::<Vec<_>>(),
    })
}

/// Where the list of recent folders is written: next to the machine config, under the Bridle name.
fn recent_file() -> Option<PathBuf> {
    harness_core::config::machine_config_write_path()
        .and_then(|config| config.parent().map(|dir| dir.join("recent-projects.json")))
}

/// The list as saved: from the Bridle folder, or from the old folder until the first save moves it.
fn read_recent() -> Vec<String> {
    let old = harness_core::config::machine_config_path()
        .and_then(|config| config.parent().map(|dir| dir.join("recent-projects.json")));
    [recent_file(), old]
        .into_iter()
        .flatten()
        .find_map(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
}

/// Puts `workspace` first in the list of recent folders. The list is a convenience: a failure to save it is ignored.
pub fn remember_project(workspace: &Path) {
    let Some(file) = recent_file() else { return };
    let this = workspace.display().to_string();
    let mut list = read_recent();
    list.retain(|entry| *entry != this);
    list.insert(0, this);
    list.truncate(RECENT_LIMIT);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(file, text);
    }
}

/// Folders opened lately that still exist, newest first.
#[tauri::command(async)]
pub fn recent_projects() -> Vec<String> {
    read_recent()
        .into_iter()
        .filter(|path| Path::new(path).is_dir())
        .collect()
}

/// Asks for a folder with the system's folder dialog. `None` when the person cancels.
#[tauri::command(async)]
pub fn pick_folder(initial: Option<String>) -> Result<Option<String>, String> {
    if !cfg!(windows) {
        return Err("the folder dialog is only available on Windows".to_owned());
    }
    const SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms; \
        $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
        $d.Description = 'Choose the folder Bridle should work in'; \
        $d.ShowNewFolderButton = $true; \
        if ($env:BRIDLE_START) { $d.SelectedPath = $env:BRIDLE_START }; \
        if ($d.ShowDialog() -eq 'OK') { [Console]::Out.Write($d.SelectedPath) }";
    let mut command = Command::new("powershell");
    command
        .args(["-NoProfile", "-NonInteractive", "-STA", "-Command", SCRIPT])
        .env("BRIDLE_START", initial.unwrap_or_default());
    harness_runtime::process::hide_console(&mut command);
    let output = command
        .output()
        .map_err(|e| format!("cannot open the folder dialog: {e}"))?;
    let chosen = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!chosen.is_empty()).then_some(chosen))
}

/// Asks for an image with the system's file dialog. `None` when the person cancels.
#[tauri::command(async)]
pub fn pick_image() -> Result<Option<String>, String> {
    if !cfg!(windows) {
        return Err("the file dialog is only available on Windows".to_owned());
    }
    const SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms; \
        $d = New-Object System.Windows.Forms.OpenFileDialog; \
        $d.Title = 'Choose an image'; \
        $d.Filter = 'Images (png, jpg, gif, webp)|*.png;*.jpg;*.jpeg;*.gif;*.webp|All files|*.*'; \
        if ($d.ShowDialog() -eq 'OK') { [Console]::Out.Write($d.FileName) }";
    let mut command = Command::new("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-STA", "-Command", SCRIPT]);
    harness_runtime::process::hide_console(&mut command);
    let output = command
        .output()
        .map_err(|e| format!("cannot open the file dialog: {e}"))?;
    let chosen = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!chosen.is_empty()).then_some(chosen))
}

/// Copies an image the person chose into the folder's attachments (`.bridle/attachments`, which the harness keeps
/// out of git), where the chat reads it through the same policy as any file. Only PNG, JPEG, GIF and WebP up to the
/// size limit are taken, checked by their first bytes. Returns the relative path, for `/image`, and a preview.
#[tauri::command(async)]
pub fn attach_image(bridge: State<'_, Bridge>, source: String) -> Result<Value, String> {
    let bytes = std::fs::read(&source).map_err(|e| format!("cannot read {source}: {e}"))?;
    stage_image(&bridge.args(), &bytes)
}

/// The same for an image that was pasted: the bytes arrive base64-encoded.
#[tauri::command(async)]
pub fn attach_image_bytes(bridge: State<'_, Bridge>, data: String) -> Result<Value, String> {
    let bytes = harness_cli::image::decode(&data).ok_or("the pasted image could not be decoded")?;
    stage_image(&bridge.args(), &bytes)
}

fn stage_image(args: &LoadArgs, bytes: &[u8]) -> Result<Value, String> {
    let max = harness_cli::image::MAX_IMAGE_BYTES;
    if bytes.len() as u64 > max {
        return Err(format!(
            "the image is {} bytes; the limit is {max}",
            bytes.len()
        ));
    }
    let media =
        harness_cli::image::sniff(bytes).ok_or("that is not a PNG, JPEG, GIF or WebP image")?;
    let extension = match media {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        _ => "webp",
    };
    let ws = workspace_of(args).map_err(|e| format!("{e:#}"))?;
    let state = harness_core::identity::state_dir(&ws);
    let dir = state.join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let name = format!("img-{stamp}.{extension}");
    std::fs::write(dir.join(&name), bytes).map_err(|e| format!("cannot save the image: {e}"))?;
    Ok(json!({
        "path": format!(
            "{}/attachments/{name}",
            state.file_name().map_or_else(|| harness_core::identity::STATE_DIR.to_owned(), |n| n.to_string_lossy().into_owned())
        ),
        "name": name,
        "bytes": bytes.len(),
        "preview": format!("data:{media};base64,{}", harness_cli::image::encode(bytes)),
    }))
}

/// The person's own terminal: a real pseudo-terminal (ConPTY on Windows), so a shell behaves as it does in any terminal
/// program: a prompt, line editing, colours, `vim`, `less`, `ssh`. It is the person's, not the model's, so the tool policy
/// does not apply to what is typed here. Its output streams to the window as base64 bytes, because a terminal's output is
/// bytes and not always whole characters.
#[derive(Default)]
pub struct Terminal {
    session: Mutex<Option<PtySession>>,
    /// Counts the sessions started. A reader thread reports only while its own session is the current one, so a shell that
    /// was replaced cannot print its exit into the new one.
    generation: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal").finish_non_exhaustive()
    }
}

struct PtySession {
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

/// A shell the terminal can run.
struct ShellProfile {
    id: &'static str,
    name: &'static str,
    program: PathBuf,
    args: Vec<String>,
    /// Whether this profile runs inside WSL, which takes the folder as `--cd`.
    wsl: bool,
}

/// The shells found on this computer, the preferred one first. Git Bash comes first: it is a native program with the usual
/// Linux tools and starts no virtual machine, so it costs almost no memory. WSL is listed last and only runs when it is
/// picked.
fn shell_profiles() -> Vec<ShellProfile> {
    let mut list = Vec::new();
    let git_bash = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ]
    .iter()
    .map(PathBuf::from)
    .chain(
        std::env::var_os("LOCALAPPDATA")
            .map(|base| PathBuf::from(base).join(r"Programs\Git\bin\bash.exe")),
    )
    .find(|path| path.is_file());
    if let Some(program) = git_bash {
        list.push(ShellProfile {
            id: "gitbash",
            name: "Git Bash",
            program,
            args: vec!["--login".to_owned(), "-i".to_owned()],
            wsl: false,
        });
    }
    let system =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let powershell = system.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if powershell.is_file() {
        list.push(ShellProfile {
            id: "powershell",
            name: "PowerShell",
            program: powershell,
            args: vec!["-NoLogo".to_owned()],
            wsl: false,
        });
    }
    let cmd = system.join(r"System32\cmd.exe");
    if cmd.is_file() {
        list.push(ShellProfile {
            id: "cmd",
            name: "Command Prompt",
            program: cmd,
            args: Vec::new(),
            wsl: false,
        });
    }
    let wsl = system.join(r"System32\wsl.exe");
    if wsl.is_file() {
        list.push(ShellProfile {
            id: "wsl",
            name: "WSL (Ubuntu)",
            program: wsl,
            args: Vec::new(),
            wsl: true,
        });
    }
    list
}

fn start_terminal(
    app: &AppHandle,
    bridge: &Bridge,
    profile: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::Read;
    stop_terminal(bridge);
    let generation = bridge.terminal.generation.load(Ordering::SeqCst);
    let ws = workspace_of(&bridge.args()).map_err(|e| format!("{e:#}"))?;
    let profiles = shell_profiles();
    let chosen = profile
        .as_deref()
        .and_then(|id| profiles.iter().find(|p| p.id == id))
        .or_else(|| profiles.first())
        .ok_or("no shell was found on this computer")?;
    let mut command = CommandBuilder::new(&chosen.program);
    for arg in &chosen.args {
        command.arg(arg);
    }
    if chosen.wsl {
        command.arg("--cd");
        command.arg(&ws);
    } else {
        command.cwd(&ws);
    }
    command.env("TERM", "xterm-256color");
    // Git's login profile would otherwise start in the home folder.
    command.env("CHERE_INVOKING", "1");
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: rows.max(2),
            cols: cols.max(10),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("cannot open a terminal: {e}"))?;
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| format!("cannot start {}: {e}", chosen.name))?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("cannot read the terminal: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("cannot write to the terminal: {e}"))?;
    *locked(&bridge.terminal.session) = Some(PtySession {
        master: pair.master,
        writer,
        child,
    });
    let handle = app.clone();
    std::thread::Builder::new()
        .name("bridle-terminal".to_owned())
        .spawn(move || {
            let mut buffer = [0u8; 8192];
            let current = || {
                handle
                    .state::<Bridge>()
                    .terminal
                    .generation
                    .load(Ordering::SeqCst)
                    == generation
            };
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 || !current() {
                    break;
                }
                let data = harness_cli::image::encode(&buffer[..n]);
                let _ = handle.emit(EVENT, json!({"type": "term", "data": data}));
            }
            if current() {
                let _ = handle.emit(EVENT, json!({"type": "term_exit"}));
            }
        })
        .map_err(|e| format!("cannot start the terminal reader: {e}"))?;
    Ok(())
}

fn stop_terminal(bridge: &Bridge) {
    bridge.terminal.generation.fetch_add(1, Ordering::SeqCst);
    if let Some(mut session) = locked(&bridge.terminal.session).take() {
        let _ = session.child.kill();
        let _ = session.child.wait();
    }
}

/// The shells this computer has, for the terminal's picker.
#[tauri::command]
pub fn term_profiles() -> Value {
    let profiles = shell_profiles();
    json!({
        "default": profiles.first().map(|p| p.id),
        "profiles": profiles.iter().map(|p| json!({"id": p.id, "name": p.name})).collect::<Vec<_>>(),
    })
}

/// Starts the terminal, or restarts it, in the current folder with the chosen shell.
#[tauri::command(async)]
pub fn term_start(
    app: AppHandle,
    bridge: State<'_, Bridge>,
    profile: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    start_terminal(&app, &bridge, profile, cols, rows)
}

/// Sends what was typed to the terminal.
#[tauri::command(async)]
pub fn term_write(bridge: State<'_, Bridge>, data: String) -> Result<(), String> {
    let mut session = locked(&bridge.terminal.session);
    let Some(session) = session.as_mut() else {
        return Err("the terminal is not running; press Restart".to_owned());
    };
    session
        .writer
        .write_all(data.as_bytes())
        .and_then(|()| session.writer.flush())
        .map_err(|e| format!("cannot write to the terminal: {e}"))
}

/// Tells the terminal its new size in characters.
#[tauri::command(async)]
pub fn term_resize(bridge: State<'_, Bridge>, cols: u16, rows: u16) {
    if let Some(session) = locked(&bridge.terminal.session).as_mut() {
        let _ = session.master.resize(portable_pty::PtySize {
            rows: rows.max(2),
            cols: cols.max(10),
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

/// Stops the terminal and everything it started.
#[tauri::command(async)]
pub fn term_stop(bridge: State<'_, Bridge>) {
    stop_terminal(&bridge);
}

/// Moves the window to another folder. The servers this window started belong to the old folder's state, so they
/// are stopped first, and the conversation starts again. Refused while a turn is running.
#[tauri::command(async)]
pub fn switch_workspace(
    app: AppHandle,
    bridge: State<'_, Bridge>,
    path: String,
) -> Result<String, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    if locked(&bridge.inner).running {
        return Err("stop the running turn before changing folder".to_owned());
    }
    let workspace =
        harness_cli::app::resolve_workspace(Some(&dir)).map_err(|e| format!("{e:#}"))?;
    // Stops what this window started for the old folder, including a server that is still loading. The records stay, so
    // a start that finishes after this point is recognised as the old folder's and stopped as well.
    bridge.shutdown();
    {
        let mut inner = locked(&bridge.inner);
        inner.agent = None;
        inner.members.clear();
    }
    locked(&bridge.args).workspace = Some(workspace.clone());
    crate::crew_cmd::recover_board(&workspace);
    remember_project(&workspace);
    set_window_title(&app, &workspace);
    refresh(&bridge);
    Ok(workspace.display().to_string())
}

/// The window's title names the folder it works on.
pub fn set_window_title(app: &AppHandle, workspace: &Path) {
    let name = workspace.file_name().map_or_else(
        || workspace.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(&format!("Bridle - {name}"));
    }
}

/// The proxy setting as saved, and the route requests take now. The environment variables are reported by name only.
#[tauri::command(async)]
pub fn get_network(bridge: State<'_, Bridge>) -> Value {
    use harness_core::config::ProxyMode;
    let app = match bridge.args().load_app() {
        Ok(app) => app,
        Err(error) => return json!({"ok": false, "error": format!("{error:#}")}),
    };
    let network = &app.config.network;
    let mode = match network.proxy_mode {
        ProxyMode::Environment => "environment",
        ProxyMode::Custom => "custom",
        ProxyMode::None => "none",
    };
    let named: Vec<&str> = ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "NO_PROXY"]
        .into_iter()
        .filter(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
        .collect();
    json!({
        "ok": true,
        "mode": mode,
        "proxy": network.proxy,
        "env_names": named,
        "env_override": harness_core::identity::lookup_env(&|n| std::env::var(n).ok(), "PROXY").is_some(),
        "override_name": harness_core::identity::env_var("PROXY"),
        "effective": harness_runtime::http::remote_route().describe(),
    })
}

/// Saves the proxy setting in the machine config. The URL is checked first, and a bad one is refused.
#[tauri::command(async)]
pub fn set_network(
    bridge: State<'_, Bridge>,
    mode: String,
    proxy: Option<String>,
) -> Result<String, String> {
    let path =
        init::write_machine_network(&mode, proxy.as_deref()).map_err(|e| format!("{e:#}"))?;
    // Loading the configuration again applies the new route to this process.
    let _ = bridge.args().load_app();
    Ok(format!("saved in {}", path.display()))
}

/// Sends one HEAD request to `url` with the proxy settings as they are in the form (not as saved), so a setting can be
/// tried before it is kept. Runs only when the person presses Test.
#[tauri::command(async)]
pub fn test_network(mode: String, proxy: Option<String>, url: String) -> Value {
    use harness_core::config::{Config, ProxyMode};
    let mut config = Config::default();
    config.network.proxy_mode = match mode.as_str() {
        "custom" => ProxyMode::Custom,
        "none" => ProxyMode::None,
        _ => ProxyMode::Environment,
    };
    config.network.proxy = proxy.filter(|p| !p.trim().is_empty());
    if config.network.proxy_mode == ProxyMode::Custom {
        match config.network.proxy.as_deref() {
            None => return json!({"ok": false, "error": "enter a proxy address first"}),
            Some(value) => {
                if let Err(reason) = harness_core::config::check_proxy_url(value) {
                    return json!({"ok": false, "error": reason});
                }
            }
        }
    }
    let route = harness_cli::app::remote_route_of(&config, &|name| {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    });
    match harness_runtime::http::probe(&route, url.trim(), std::time::Duration::from_secs(15)) {
        Ok(probe) => {
            json!({"ok": true, "status": probe.status, "elapsed_ms": probe.elapsed_ms, "route": probe.route})
        }
        Err(error) => json!({"ok": false, "error": error, "route": route.describe()}),
    }
}

/// First run on a folder: writes `bridle.toml` (from the local-only profile) and the `.gitignore`
/// entries. It writes none of the OpenCode files.
#[tauri::command(async)]
pub fn setup_repository(bridge: State<'_, Bridge>) -> Result<String, String> {
    let ws = workspace_of(&bridge.args()).map_err(|e| format!("{e:#}"))?;
    let wrote = init::write_repository_config(&ws, "local-only").map_err(|e| format!("{e:#}"))?;
    Ok(if wrote {
        format!("wrote bridle.toml in {}", ws.display())
    } else {
        "bridle.toml already exists".to_owned()
    })
}

/// First run on a machine: records where the model weights and runtimes are.
#[tauri::command(async)]
pub fn setup_machine(
    models_dir: Option<String>,
    runtimes_dir: Option<String>,
) -> Result<String, String> {
    let models = models_dir.as_deref().map(Path::new);
    let runtimes = runtimes_dir.as_deref().map(Path::new);
    init::write_machine_config(models, runtimes, None)
        .map(|path| format!("wrote {}", path.display()))
        .map_err(|e| format!("{e:#}"))
}

/// Shows the machine configuration file in Windows Explorer (selected), so the person can open it in an editor. Runs `explorer.exe`
/// with a path that Bridle computed; nothing the model says reaches it.
#[tauri::command]
pub fn reveal_machine_config() -> Result<String, String> {
    let path = harness_core::config::machine_config_path()
        .ok_or("this machine has no machine configuration folder")?;
    let target = if path.exists() {
        path.clone()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.clone())
    };
    if !target.exists() {
        return Err(format!(
            "{} does not exist yet: run the first-run setup, or `bridle init --machine`",
            target.display()
        ));
    }
    let mut command = std::process::Command::new("explorer.exe");
    if path.exists() {
        command.arg(format!("/select,{}", target.display()));
    } else {
        command.arg(&target);
    }
    command
        .spawn()
        .map_err(|e| format!("cannot open Explorer: {e}"))?;
    Ok(path.display().to_string())
}

/// The folders a first run would propose, for the setup form: those that exist on this machine.
#[tauri::command]
pub fn suggest_machine_paths() -> Value {
    let found = |path: &str| Path::new(path).is_dir().then(|| path.to_owned());
    json!({
        "models_dir": found(r"D:\models").or_else(|| found(r"C:\models")),
        "runtimes_dir": found(r"D:\runtimes").or_else(|| found(r"C:\runtimes")),
    })
}

/// The changed files of the workspace (`git status`), for the Changes tab. The person asked for it, so it
/// does not go through the model's policy; the text is still redacted.
#[tauri::command(async)]
pub fn git_changes(bridge: State<'_, Bridge>) -> Value {
    let Ok(ws) = workspace_of(&bridge.args()) else {
        return json!({"ok": false, "error": "no workspace"});
    };
    match git(
        &ws,
        &[
            "status",
            "--porcelain=v1",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    ) {
        Ok(text) => {
            let mut parts = text.split('\0').filter(|p| !p.is_empty());
            let branch = parts
                .next()
                .and_then(|line| line.strip_prefix("## "))
                .unwrap_or("")
                .to_owned();
            let mut files = Vec::new();
            while let Some(entry) = parts.next() {
                let (code, path) = entry.split_at(entry.len().min(3));
                let code = code.trim().to_owned();
                // A rename or copy is followed by the old name as its own entry.
                if code.starts_with('R') || code.starts_with('C') {
                    parts.next();
                }
                files.push(json!({"status": code, "path": path}));
            }
            json!({"ok": true, "branch": branch, "files": files})
        }
        Err(error) => json!({"ok": false, "error": error}),
    }
}

/// The diff of one file against `HEAD`, or the whole new file when it is not tracked yet.
#[tauri::command(async)]
pub fn git_file_diff(bridge: State<'_, Bridge>, path: String) -> Value {
    let (ws, rel) = match resolve_in_workspace(&bridge.args(), &path) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    let spec = rel.to_string_lossy().replace('\\', "/");
    let tracked = git(&ws, &["ls-files", "--error-unmatch", "--", &spec]).is_ok();
    let diff = if tracked {
        git(
            &ws,
            &["diff", "--no-color", "--no-ext-diff", "HEAD", "--", &spec],
        )
        .or_else(|_| git(&ws, &["diff", "--no-color", "--no-ext-diff", "--", &spec]))
    } else {
        // `git diff --no-index` exits with 1 when the files differ, which `git` reports as an error.
        git_allow_status(
            &ws,
            &["diff", "--no-color", "--no-index", "--", NULL_DEVICE, &spec],
            1,
        )
    };
    match diff {
        Ok(text) => {
            let redactor = Redactor::new();
            let truncated = text.len() > DIFF_LIMIT;
            json!({"ok": true, "diff": redactor.redact(&cut_at(&text, DIFF_LIMIT)).into_owned(), "truncated": truncated})
        }
        Err(error) => json!({"ok": false, "error": error}),
    }
}

/// The entries of one folder of the workspace, folders first. Version-control data, build output and the
/// harness's own state are left out.
#[tauri::command(async)]
pub fn list_dir(bridge: State<'_, Bridge>, path: String) -> Value {
    let (ws, rel) = match resolve_in_workspace(&bridge.args(), &path) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    let dir = ws.join(&rel);
    let read = match std::fs::read_dir(&dir) {
        Ok(read) => read,
        Err(error) => {
            return json!({"ok": false, "error": format!("cannot read the folder: {error}")});
        }
    };
    let mut entries: Vec<(bool, String, u64)> = read
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if HIDDEN_NAMES.contains(&name.as_str()) {
                return None;
            }
            let meta = entry.metadata().ok()?;
            Some((meta.is_dir(), name, meta.len()))
        })
        .collect();
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let truncated = entries.len() > LIST_LIMIT;
    entries.truncate(LIST_LIMIT);
    json!({
        "ok": true,
        "truncated": truncated,
        "entries": entries.iter().map(|(is_dir, name, size)| json!({"name": name, "dir": is_dir, "size": size})).collect::<Vec<_>>(),
    })
}

/// A text file of the workspace for the viewer. Read only; the text is redacted, and `redacted` says when
/// that changed anything, so the viewer never passes off altered text as the file's content.
#[tauri::command(async)]
pub fn read_workspace_file(bridge: State<'_, Bridge>, path: String) -> Value {
    let (ws, rel) = match resolve_in_workspace(&bridge.args(), &path) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    let full = ws.join(&rel);
    let size = match std::fs::metadata(&full) {
        Ok(meta) if meta.is_file() => meta.len(),
        Ok(_) => return json!({"ok": false, "error": "that is a folder"}),
        Err(error) => {
            return json!({"ok": false, "error": format!("cannot read the file: {error}")});
        }
    };
    if size > FILE_LIMIT {
        return json!({"ok": false, "error": format!("the file is {size} bytes; the viewer opens files up to {FILE_LIMIT} bytes")});
    }
    let bytes = match std::fs::read(&full) {
        Ok(bytes) => bytes,
        Err(error) => {
            return json!({"ok": false, "error": format!("cannot read the file: {error}")});
        }
    };
    if bytes.contains(&0) {
        return json!({"ok": false, "error": "this looks like a binary file", "binary": true});
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let redactor = Redactor::new();
    let clean = redactor.redact(&text).into_owned();
    json!({
        "ok": true,
        "path": rel.to_string_lossy().replace('\\', "/"),
        "size": size,
        "redacted": clean != text,
        "modified": modified_ms(&full),
        "content": clean,
    })
}

/// A file's last-modified time in milliseconds since the epoch; 0 when it cannot be read.
fn modified_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Saves the person's edit of a file. The write is atomic (a temporary file, then a rename), refuses a file that changed
/// on disk since it was read unless `force` is set, and never touches version-control data or the harness's own state.
#[tauri::command(async)]
pub fn write_workspace_file(
    bridge: State<'_, Bridge>,
    path: String,
    content: String,
    expected_modified: Option<u64>,
    force: bool,
) -> Value {
    let (ws, rel) = match resolve_in_workspace(&bridge.args(), &path) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    if let Err(error) = check_user_writable(&rel) {
        return json!({"ok": false, "error": error});
    }
    let full = ws.join(&rel);
    if let (Some(expected), false) = (expected_modified, force) {
        let now = modified_ms(&full);
        if full.exists() && now != expected {
            return json!({"ok": false, "conflict": true, "error": "the file changed on disk after it was opened (the agent or another program wrote it)"});
        }
    }
    let temporary = full.with_extension(format!("bridle-{}.tmp", std::process::id()));
    let written = std::fs::write(&temporary, content.as_bytes()).and_then(|()| {
        // Keep the file's permissions: the temporary file starts with the defaults.
        if let Ok(meta) = std::fs::metadata(&full) {
            let _ = std::fs::set_permissions(&temporary, meta.permissions());
        }
        std::fs::rename(&temporary, &full)
    });
    match written {
        Ok(()) => json!({"ok": true, "modified": modified_ms(&full)}),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            json!({"ok": false, "error": format!("cannot save the file: {error}")})
        }
    }
}

/// The content of a file at the last commit, for the diff view. Empty when the file is not tracked yet.
#[tauri::command(async)]
pub fn git_show_head(bridge: State<'_, Bridge>, path: String) -> Value {
    let (ws, rel) = match resolve_in_workspace(&bridge.args(), &path) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    let spec = format!("HEAD:{}", rel.to_string_lossy().replace('\\', "/"));
    match git(&ws, &["show", &spec]) {
        Ok(text) => {
            let redactor = Redactor::new();
            json!({"ok": true, "tracked": true, "content": redactor.redact(&text).into_owned()})
        }
        Err(_) => json!({"ok": true, "tracked": false, "content": ""}),
    }
}

/// Creates an empty file or a folder inside the workspace. Refuses a name that exists.
#[tauri::command(async)]
pub fn create_entry(bridge: State<'_, Bridge>, path: String, folder: bool) -> Value {
    let (ws, rel) = match resolve_new_in_workspace(&bridge.args(), &path) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    if let Err(error) = check_user_writable(&rel) {
        return json!({"ok": false, "error": error});
    }
    let full = ws.join(&rel);
    if full.exists() {
        return json!({"ok": false, "error": "that name already exists"});
    }
    let made = if folder {
        std::fs::create_dir_all(&full)
    } else {
        full.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&full, b""))
    };
    match made {
        Ok(()) => json!({"ok": true}),
        Err(error) => json!({"ok": false, "error": format!("cannot create it: {error}")}),
    }
}

/// Renames or moves a file or folder inside the workspace.
#[tauri::command(async)]
pub fn rename_entry(bridge: State<'_, Bridge>, from: String, to: String) -> Value {
    let args = bridge.args();
    let (ws, source) = match resolve_in_workspace(&args, &from) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    let (_, target) = match resolve_new_in_workspace(&args, &to) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    if let Err(error) = check_user_writable(&source).and_then(|()| check_user_writable(&target)) {
        return json!({"ok": false, "error": error});
    }
    if ws.join(&target).exists() {
        return json!({"ok": false, "error": "that name already exists"});
    }
    match std::fs::rename(ws.join(&source), ws.join(&target)) {
        Ok(()) => json!({"ok": true}),
        Err(error) => json!({"ok": false, "error": format!("cannot rename it: {error}")}),
    }
}

/// "Deletes" a file or folder by moving it into `.bridle/trash`, so the person can get it back.
#[tauri::command(async)]
pub fn delete_entry(bridge: State<'_, Bridge>, path: String) -> Value {
    let (ws, rel) = match resolve_in_workspace(&bridge.args(), &path) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    if let Err(error) = check_user_writable(&rel) {
        return json!({"ok": false, "error": error});
    }
    match move_to_trash(&ws, &rel) {
        Ok(place) => json!({"ok": true, "trash": place}),
        Err(error) => json!({"ok": false, "error": error}),
    }
}

fn move_to_trash(ws: &Path, rel: &Path) -> Result<String, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let name = rel
        .file_name()
        .map_or_else(|| "entry".to_owned(), |n| n.to_string_lossy().into_owned());
    let dir = harness_core::identity::state_dir(ws)
        .join("trash")
        .join(stamp.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create the trash folder: {e}"))?;
    let target = dir.join(&name);
    std::fs::rename(ws.join(rel), &target)
        .map_err(|e| format!("cannot move it to the trash: {e}"))?;
    Ok(target.display().to_string())
}

/// Throws away the changes to one file: a tracked file goes back to its last commit, a new file is moved to the trash.
#[tauri::command(async)]
pub fn revert_file(bridge: State<'_, Bridge>, path: String) -> Value {
    let (ws, rel) = match resolve_in_workspace(&bridge.args(), &path) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    if let Err(error) = check_user_writable(&rel) {
        return json!({"ok": false, "error": error});
    }
    let spec = rel.to_string_lossy().replace('\\', "/");
    if git(&ws, &["ls-files", "--error-unmatch", "--", &spec]).is_ok() {
        match git(&ws, &["checkout", "HEAD", "--", &spec]) {
            Ok(_) => json!({"ok": true, "action": "restored"}),
            Err(error) => json!({"ok": false, "error": error}),
        }
    } else {
        match move_to_trash(&ws, &rel) {
            Ok(place) => json!({"ok": true, "action": "moved to the trash", "trash": place}),
            Err(error) => json!({"ok": false, "error": error}),
        }
    }
}

/// A preview of an image file of the workspace, as a data URL, for an image tab.
#[tauri::command(async)]
pub fn read_image_preview(bridge: State<'_, Bridge>, path: String) -> Value {
    let (ws, rel) = match resolve_in_workspace(&bridge.args(), &path) {
        Ok(pair) => pair,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    let Ok(bytes) = std::fs::read(ws.join(&rel)) else {
        return json!({"ok": false, "error": "cannot read the file"});
    };
    if bytes.len() as u64 > harness_cli::image::MAX_IMAGE_BYTES * 4 {
        return json!({"ok": false, "error": "the image is too large to preview"});
    }
    match harness_cli::image::sniff(&bytes) {
        Some(media) => {
            json!({"ok": true, "preview": format!("data:{media};base64,{}", harness_cli::image::encode(&bytes))})
        }
        None => json!({"ok": false, "error": "not an image the viewer can show"}),
    }
}

/// Every file of the workspace, for "Go to file". In a git repository it is the tracked and the untracked files that git
/// does not ignore; elsewhere it walks the folders the explorer lists.
#[tauri::command(async)]
pub fn list_workspace_files(bridge: State<'_, Bridge>) -> Value {
    let Ok(ws) = workspace_of(&bridge.args()) else {
        return json!({"ok": false, "error": "no workspace"});
    };
    let mut files: Vec<String> = Vec::new();
    match git(
        &ws,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    ) {
        Ok(out) => files.extend(out.split('\0').filter(|p| !p.is_empty()).map(str::to_owned)),
        Err(_) => collect_files(&ws, &ws, &mut files, 0),
    }
    files.truncate(FILE_LIST_LIMIT);
    json!({"ok": true, "files": files})
}

/// Files "Go to file" lists at most.
const FILE_LIST_LIMIT: usize = 8000;

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 12 || out.len() >= FILE_LIST_LIMIT {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if HIDDEN_NAMES.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        match entry.metadata() {
            Ok(meta) if meta.is_dir() => collect_files(root, &path, out, depth + 1),
            Ok(_) => out.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            ),
            Err(_) => {}
        }
    }
}

/// Text search over the workspace. In a git repository it uses `git grep`, which skips what git ignores and binary
/// files; elsewhere it walks the folders the explorer lists. At most [`SEARCH_LIMIT`] matches come back.
#[tauri::command(async)]
pub fn search_workspace(
    bridge: State<'_, Bridge>,
    query: String,
    case_sensitive: bool,
    regex: bool,
) -> Value {
    let Ok(ws) = workspace_of(&bridge.args()) else {
        return json!({"ok": false, "error": "no workspace"});
    };
    if query.trim().is_empty() {
        return json!({"ok": true, "matches": [], "truncated": false});
    }
    let redactor = Redactor::new();
    let mut matches: Vec<Value> = Vec::new();
    let mut truncated = false;
    let mut push = |path: &str, line: u64, text: &str| {
        if matches.len() >= SEARCH_LIMIT {
            truncated = true;
            return;
        }
        let shown: String = text.chars().take(240).collect();
        matches.push(
            json!({"path": path, "line": line, "text": redactor.redact(&shown).into_owned()}),
        );
    };
    let inside_git = git(&ws, &["rev-parse", "--is-inside-work-tree"]).is_ok();
    if inside_git {
        let mut args = vec![
            "grep",
            "-n",
            "-I",
            "--no-color",
            "--untracked",
            "--exclude-standard",
            "-z",
        ];
        if !case_sensitive {
            args.push("-i");
        }
        args.push(if regex { "-E" } else { "-F" });
        args.extend(["-e", &query]);
        // `git grep` exits with 1 when nothing matches.
        match git_allow_status(&ws, &args, 1) {
            Ok(out) => {
                // With -z a match is `path NUL line NUL text`, matches separated by newlines.
                for record in out.split('\n') {
                    let mut parts = record.splitn(3, '\0');
                    if let (Some(path), Some(line), Some(text)) =
                        (parts.next(), parts.next(), parts.next())
                        && let Ok(line) = line.parse::<u64>()
                    {
                        push(path, line, text.trim_end_matches('\r'));
                    }
                }
            }
            Err(error) => return json!({"ok": false, "error": error}),
        }
    } else {
        let matcher = match build_matcher(&query, case_sensitive, regex) {
            Ok(matcher) => matcher,
            Err(error) => return json!({"ok": false, "error": error}),
        };
        walk_search(&ws, &ws, &matcher, &mut push, 0);
    }
    json!({"ok": true, "matches": matches, "truncated": truncated})
}

/// Matches at most this many lines are returned by one search.
const SEARCH_LIMIT: usize = 400;

/// Decides whether one line matches the search.
type LineMatcher = Box<dyn Fn(&str) -> bool>;

fn build_matcher(query: &str, case_sensitive: bool, regex: bool) -> Result<LineMatcher, String> {
    if regex {
        let pattern = compile_pattern(query, case_sensitive)?;
        Ok(Box::new(move |line| pattern.is_match(line)))
    } else if case_sensitive {
        let needle = query.to_owned();
        Ok(Box::new(move |line| line.contains(&needle)))
    } else {
        let needle = query.to_lowercase();
        Ok(Box::new(move |line| line.to_lowercase().contains(&needle)))
    }
}

fn compile_pattern(query: &str, case_sensitive: bool) -> Result<regex::Regex, String> {
    regex::RegexBuilder::new(query)
        .case_insensitive(!case_sensitive)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| format!("not a valid pattern: {e}"))
}

fn walk_search(
    root: &Path,
    dir: &Path,
    matcher: &dyn Fn(&str) -> bool,
    push: &mut dyn FnMut(&str, u64, &str),
    depth: usize,
) {
    if depth > 12 {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if HIDDEN_NAMES.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            walk_search(root, &path, matcher, push, depth + 1);
        } else if meta.len() <= FILE_LIMIT {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if bytes.contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            for (index, line) in text.lines().enumerate() {
                if matcher(line) {
                    push(&rel, index as u64 + 1, line);
                }
            }
        }
    }
}

/// A path the person may change from the window: not version-control data and not the harness's own state.
fn check_user_writable(rel: &Path) -> Result<(), String> {
    match rel.components().next() {
        Some(Component::Normal(first))
            if first == ".git"
                || first == harness_core::identity::STATE_DIR
                || first == harness_core::identity::LEGACY_STATE_DIR =>
        {
            Err(format!(
                "{} is managed by git or by Bridle and is not changed from here",
                first.to_string_lossy()
            ))
        }
        _ => Ok(()),
    }
}

/// Like [`resolve_in_workspace`] for a path that does not exist yet: its parent folder must be inside the workspace.
fn resolve_new_in_workspace(args: &LoadArgs, path: &str) -> Result<(PathBuf, PathBuf), String> {
    let ws = workspace_of(args).map_err(|e| format!("{e:#}"))?;
    let rel: PathBuf = Path::new(path)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    if rel.as_os_str().is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("the path must be inside the folder".to_owned());
    }
    let canonical_ws =
        std::fs::canonicalize(&ws).map_err(|e| format!("cannot read the folder: {e}"))?;
    // The nearest parent that exists must be inside the workspace, so a new entry cannot be placed outside through a link.
    let mut probe = ws.join(&rel);
    while !probe.exists() {
        if !probe.pop() {
            break;
        }
    }
    let canonical =
        std::fs::canonicalize(&probe).map_err(|e| format!("cannot read the folder: {e}"))?;
    if !canonical.starts_with(&canonical_ws) {
        return Err("the path leaves the folder".to_owned());
    }
    Ok((ws, rel))
}

/// Folders and files the explorer does not list.
const HIDDEN_NAMES: [&str; 8] = [
    ".git",
    ".bridle",
    ".local-agent",
    ".worktrees",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
];
/// Entries of one folder the explorer returns at most.
const LIST_LIMIT: usize = 2000;
/// Largest file the viewer opens.
const FILE_LIMIT: u64 = 1_000_000;
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// The workspace and `path` inside it, as a relative path. A path that is absolute, climbs out with `..`,
/// or leaves the workspace through a link is refused.
fn resolve_in_workspace(args: &LoadArgs, path: &str) -> Result<(PathBuf, PathBuf), String> {
    let ws = workspace_of(args).map_err(|e| format!("{e:#}"))?;
    let rel: PathBuf = Path::new(path)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("the path must be inside the folder".to_owned());
    }
    let canonical_ws =
        std::fs::canonicalize(&ws).map_err(|e| format!("cannot read the folder: {e}"))?;
    if let Ok(canonical) = std::fs::canonicalize(ws.join(&rel))
        && !canonical.starts_with(&canonical_ws)
    {
        return Err("the path leaves the folder".to_owned());
    }
    Ok((ws, rel))
}

/// The first `limit` bytes of `text`, cut on a character boundary.
fn cut_at(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

pub(crate) fn workspace_of(args: &LoadArgs) -> anyhow::Result<PathBuf> {
    harness_cli::app::resolve_workspace(args.workspace.as_deref())
}

fn git(workspace: &Path, args: &[&str]) -> Result<String, String> {
    git_allow_status(workspace, args, 0)
}

/// Runs git and treats exit status `allowed` (besides 0) as success. `git diff --no-index` exits with 1
/// when the two sides differ.
fn git_allow_status(workspace: &Path, args: &[&str], allowed: i32) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(workspace);
    harness_runtime::process::hide_console(&mut command);
    let output = command
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if output.status.success() || output.status.code() == Some(allowed) {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}
