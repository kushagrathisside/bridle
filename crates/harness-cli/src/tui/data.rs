//! Passive state collection for the TUI: configuration, model slots, event logs, tasks, policy and
//! network summaries.
//!
//! Collecting a [`Snapshot`] reads local files and process records only. It never contacts a model
//! endpoint, starts a process or writes anything. Everything shown comes from the same loaders the
//! CLI uses ([`App::load`], the validator, the redactor, the PID records of `agent models`).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use harness_core::config::{Origin, Severity, Verdict, endpoint_is_loopback, machine_config_path};
use harness_core::identity;
use harness_core::security::redact::Redactor;
use harness_runtime::state::StateDir;

use crate::app::{self, App};
use crate::commands::config::redacted_toml;
use crate::commands::egress::destinations;
use crate::commands::health::wire_name;
use crate::commands::models::{self, ServerState};
use crate::tui::events::{self, LogRead, sanitize};

/// How to find the same configuration the CLI would load.
#[derive(Debug, Clone, Default)]
pub struct LoadArgs {
    pub config: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
}

impl LoadArgs {
    pub fn load_app(&self) -> anyhow::Result<App> {
        App::load(self.config.as_deref(), self.workspace.as_deref())
    }
}

/// Severity of a status line. Always shown with a word, never by colour alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sev {
    Ok,
    Info,
    Unknown,
    Warn,
    Fail,
}

impl Sev {
    pub fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Info => "info",
            Self::Unknown => "unchecked",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssueRow {
    pub error: bool,
    pub rule: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileState {
    /// The slot has no `launch` section: Bridle does not manage its server.
    NotManaged,
    /// The path cannot be resolved (relative, and no `paths.*_dir` set).
    Unresolved,
    Present(Option<u64>),
    Missing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlotRow {
    pub name: String,
    pub model_id: String,
    pub role: String,
    pub provider: String,
    pub hardware: String,
    pub endpoint: String,
    pub loopback: bool,
    pub runtime: Option<String>,
    pub runtime_bin: Option<String>,
    pub runtime_state: FileState,
    pub model_file: Option<String>,
    pub model_state: FileState,
    pub server: ServerState,
    pub log_path: String,
    /// Whether `agent models serve` could start this slot (it has a launch section).
    pub launchable: bool,
}

impl SlotRow {
    /// Static (no network) readiness of the slot's files.
    pub fn static_sev(&self) -> Sev {
        let bad = |s: &FileState| matches!(s, FileState::Missing | FileState::Unresolved);
        if self.launchable && (bad(&self.model_state) || bad(&self.runtime_state)) {
            Sev::Fail
        } else if matches!(
            self.server,
            ServerState::Unreadable(_) | ServerState::Reused { .. }
        ) {
            Sev::Warn
        } else {
            Sev::Ok
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolicyInfo {
    pub rows: Vec<(String, String)>,
    pub allowed_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetRow {
    pub name: String,
    pub target: String,
    pub scope: String,
    pub note: String,
    pub sev: Sev,
}

/// One task, or one session that never became a task. The task fields come from the orchestrator's journal
/// and are empty for a session; the counts come from the event logs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    /// True for a task of the journal or an event that names a task, false for a bare session id.
    pub is_task: bool,
    pub objective: String,
    /// `active`, `blocked`, `completed` or `aborted`; empty for a session.
    pub status: String,
    pub phase: String,
    pub iteration: u32,
    pub last_action: String,
    pub last_test: String,
    pub failures: u32,
    pub refusals: u32,
    pub worktree: Option<String>,
    /// Sessions of the logs that belong to this task.
    pub sessions: Vec<String>,
    pub first: String,
    pub last: String,
    pub events: usize,
    pub allowed: usize,
    pub blocked: usize,
    pub failed: usize,
    pub last_kind: String,
    pub last_status: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub workspace: Option<PathBuf>,
    /// Set when the configuration could not be loaded at all.
    pub config_error: Option<String>,
    pub config_files: Vec<(String, String)>,
    pub machine_config: Option<(String, bool)>,
    pub issues: Vec<IssueRow>,
    pub slots: Vec<SlotRow>,
    /// The slot named by `agent.default_model`.
    pub default_slot: Option<String>,
    pub config_lines: Vec<ConfigLine>,
    pub policy: Option<PolicyInfo>,
    pub network: Vec<NetRow>,
    pub log: LogRead,
    pub tasks: Vec<TaskRow>,
    pub collected_at: String,
}

impl Snapshot {
    pub fn workspace_text(&self) -> String {
        self.workspace
            .as_ref()
            .map_or_else(|| "(unresolved)".to_owned(), |p| p.display().to_string())
    }

    pub fn error_count(&self) -> usize {
        self.issues.iter().filter(|i| i.error).count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues.iter().filter(|i| !i.error).count()
    }

    /// Reads local state. Always returns a snapshot: failures become `config_error` or empty data.
    pub fn collect(args: &LoadArgs) -> Self {
        let mut snap = Self {
            collected_at: events::short_time(
                &humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string(),
            ),
            ..Self::default()
        };
        let redactor = Redactor::new();
        match args.load_app() {
            Ok(app) => snap.fill_from_app(&app, &redactor),
            Err(err) => {
                snap.config_error = Some(sanitize(&redactor.redact(&format!("{err:#}"))));
                snap.workspace = app::resolve_workspace(args.workspace.as_deref()).ok();
            }
        }
        if let Some(ws) = &snap.workspace {
            snap.log = events::read_logs(&ws.join(identity::STATE_DIR));
            snap.tasks = collect_tasks(ws, &snap.log);
        }
        snap
    }

    fn fill_from_app(&mut self, app: &App, redactor: &Redactor) {
        self.workspace = Some(app.workspace.clone());
        let machine = machine_config_path();
        self.machine_config = machine
            .as_ref()
            .map(|p| (p.display().to_string(), p.exists()));
        self.config_files = app
            .config_files
            .iter()
            .map(|f| {
                let layer = if Some(f) == machine.as_ref() {
                    "machine (trusted)"
                } else if f.file_name().is_some_and(|n| n == identity::CONFIG_FILE) {
                    "repository (untrusted: restricted by the trust rules)"
                } else {
                    "explicit --config / env (trusted)"
                };
                (f.display().to_string(), layer.to_owned())
            })
            .collect();
        self.issues = app
            .issues
            .iter()
            .map(|i| IssueRow {
                error: i.severity == Severity::Error,
                rule: i.rule.to_owned(),
                path: sanitize(&i.path),
                message: sanitize(&redactor.redact(&i.message)),
            })
            .collect();
        self.default_slot = Some(app.config.agent.default_model.clone()).filter(|s| !s.is_empty());
        self.slots = app
            .config
            .models
            .iter()
            .map(|(name, model)| slot_row(app, name, model))
            .collect();
        self.config_lines = match redacted_toml(app) {
            Ok(text) => annotate(&text, &app.origins),
            Err(e) => vec![ConfigLine {
                text: format!("cannot render the configuration: {e}"),
                origin: String::new(),
            }],
        };
        self.policy = Some(policy_info(app));
        self.network = network_rows(app);
    }
}

/// One line of the rendered configuration with the layer that supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLine {
    pub text: String,
    /// `machine`, `repo`, `--config`, `env`, `default`; empty for lines that name no key.
    pub origin: String,
}

/// Tags each line of the redacted TOML with the layer that set its key. A key with no recorded
/// origin comes from the built-in defaults; a deeper key inherits the nearest recorded ancestor.
pub fn annotate(toml_text: &str, origins: &BTreeMap<String, Origin>) -> Vec<ConfigLine> {
    // Origins are recorded for key paths up to three parts. A deeper key belongs to its
    // three-part ancestor; a shallower key that was never recorded was not set by any file.
    let lookup = |path: &str| -> String {
        let key = path.splitn(4, '.').take(3).collect::<Vec<_>>().join(".");
        origins
            .get(&key)
            .map_or_else(|| "default".to_owned(), |o| o.label().to_owned())
    };
    let mut prefix = String::new();
    let mut previous = String::new();
    let mut out = Vec::new();
    for raw in toml_text.lines() {
        let text = sanitize(raw);
        let origin = if raw.is_empty() {
            String::new()
        } else if raw.starts_with(' ') || raw.starts_with(']') {
            // Continuation of a multi-line value.
            previous.clone()
        } else if let Some(header) = raw
            .strip_prefix("[[")
            .and_then(|h| h.strip_suffix("]]"))
            .or_else(|| raw.strip_prefix('[').and_then(|h| h.strip_suffix(']')))
        {
            prefix = header.replace('"', "");
            // A header names a section, not a value: no tag.
            String::new()
        } else if let Some((key, _)) = raw.split_once(" = ") {
            let key = key.trim().replace('"', "");
            let path = if prefix.is_empty() {
                key
            } else {
                format!("{prefix}.{key}")
            };
            lookup(&path)
        } else {
            previous.clone()
        };
        if !origin.is_empty() {
            previous.clone_from(&origin);
        }
        out.push(ConfigLine { text, origin });
    }
    out
}

/// Redacts secrets and strips control characters from a configured value before it is shown.
fn shown(text: &str) -> String {
    static REDACTOR: std::sync::OnceLock<Redactor> = std::sync::OnceLock::new();
    sanitize(&REDACTOR.get_or_init(Redactor::new).redact(text))
}

fn file_state(path: Option<&Path>, launchable: bool) -> FileState {
    if !launchable {
        return FileState::NotManaged;
    }
    match path {
        None => FileState::Unresolved,
        Some(p) => match std::fs::metadata(p) {
            Ok(m) if m.is_file() => FileState::Present(Some(m.len())),
            _ => FileState::Missing,
        },
    }
}

fn slot_row(app: &App, name: &str, model: &harness_core::config::ModelConfig) -> SlotRow {
    let launchable = model.launch.is_some();
    let runtime = model.launch.as_ref().map(|l| l.runtime.clone());
    let runtime_bin = runtime
        .as_deref()
        .and_then(|r| app.config.resolve_runtime_bin(r));
    let model_file = app.config.resolve_model_file(name);
    SlotRow {
        name: sanitize(name),
        model_id: sanitize(&model.id),
        role: wire_name(&model.role),
        provider: wire_name(&model.provider),
        hardware: wire_name(&model.hardware),
        endpoint: shown(&model.endpoint),
        loopback: endpoint_is_loopback(&model.endpoint),
        runtime,
        runtime_bin: runtime_bin.as_ref().map(|p| p.display().to_string()),
        runtime_state: file_state(runtime_bin.as_deref(), launchable),
        model_file: model_file.as_ref().map(|p| p.display().to_string()),
        model_state: file_state(model_file.as_deref(), launchable),
        server: models::server_state(app, name),
        log_path: models::server_log_path(app, name).display().to_string(),
        launchable,
    }
}

fn verdict_word(v: Verdict) -> &'static str {
    match v {
        Verdict::Allow => "allow",
        Verdict::Ask => "ask (blocked)",
        Verdict::Deny => "deny",
    }
}

fn policy_info(app: &App) -> PolicyInfo {
    let s = &app.config.security;
    let yes = |b: bool| if b { "yes" } else { "no" }.to_owned();
    let rows = vec![
        (
            "network commands".to_owned(),
            verdict_word(s.network).to_owned(),
        ),
        (
            "destructive commands".to_owned(),
            verdict_word(s.destructive_commands).to_owned(),
        ),
        (
            "privileged commands".to_owned(),
            verdict_word(s.privileged_commands).to_owned(),
        ),
        (
            "risky commands".to_owned(),
            verdict_word(s.risky_commands).to_owned(),
        ),
        ("require worktree".to_owned(), yes(s.require_worktree)),
        ("secrets redaction".to_owned(), yes(s.secrets_redaction)),
        (
            "extra safe commands".to_owned(),
            s.safe_commands.len().to_string(),
        ),
        (
            "event log enabled".to_owned(),
            yes(app.config.telemetry.enabled),
        ),
    ];
    PolicyInfo {
        rows,
        allowed_paths: s
            .allowed_paths
            .iter()
            .map(|p| sanitize(&p.display().to_string()))
            .collect(),
    }
}

/// The outbound destinations, from the same list `agent egress` prints.
fn network_rows(app: &App) -> Vec<NetRow> {
    destinations(&app.config)
        .into_iter()
        .map(|d| NetRow {
            name: shown(&d.what),
            target: shown(&d.endpoint),
            scope: if d.loopback { "this machine" } else { "REMOTE" }.to_owned(),
            note: shown(&d.when),
            sev: if d.loopback { Sev::Ok } else { Sev::Warn },
        })
        .collect()
}

/// Proxy environment variables that are set (names only; values may hold credentials).
pub fn proxy_vars() -> Vec<String> {
    ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"]
        .iter()
        .flat_map(|v| [v.to_string(), v.to_lowercase()])
        .filter(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()))
        .collect()
}

/// Groups events by task id, falling back to the session id.
pub fn group_tasks(log: &LogRead) -> Vec<TaskRow> {
    let mut index: HashMap<(String, bool), usize> = HashMap::new();
    let mut rows: Vec<TaskRow> = Vec::new();
    for e in &log.events {
        let Some((id, is_task)) = e.group_key() else {
            continue;
        };
        let i = *index.entry((id.to_owned(), is_task)).or_insert_with(|| {
            rows.push(TaskRow {
                id: id.to_owned(),
                is_task,
                first: e.time.clone(),
                last: e.time.clone(),
                ..TaskRow::default()
            });
            rows.len() - 1
        });
        let row = &mut rows[i];
        row.events += 1;
        row.last = e.time.clone();
        row.last_kind = e.kind.clone();
        row.last_status = e.status.clone();
        match e.decision {
            Some(d) if d.is_blocked() => row.blocked += 1,
            Some(_) => row.allowed += 1,
            None => {}
        }
        if e.is_failure() {
            row.failed += 1;
        }
    }
    rows.sort_by(|a, b| b.last.cmp(&a.last));
    rows
}

/// Tasks of the orchestrator's journal that are shown at most.
const MAX_TASKS: usize = 100;
/// Sessions of the logs looked up in the session index at most.
const MAX_SESSIONS: usize = 300;

/// The tasks of the workspace: the journal's tasks with the events of their sessions folded in, then the
/// sessions and task ids of the logs that match no task. Read-only: nothing is created, locked or
/// cleaned, so opening the screen never changes the state.
pub fn collect_tasks(workspace: &Path, log: &LogRead) -> Vec<TaskRow> {
    let rows = group_tasks(log);
    let Some(state) = StateDir::open_existing(workspace) else {
        return rows;
    };
    let mut by_id: HashMap<String, TaskRow> = HashMap::new();
    let ids = state.task_ids().unwrap_or_default();
    for id in ids.into_iter().rev().take(MAX_TASKS) {
        let Ok(task) = state.task(&id) else { continue };
        let row = match task.peek() {
            Ok(journal) => {
                let s = &journal.state;
                TaskRow {
                    id: id.clone(),
                    is_task: true,
                    objective: shown(&s.original_prompt),
                    status: format!("{:?}", s.status).to_lowercase(),
                    phase: s.phase.name().to_lowercase(),
                    iteration: s.iteration,
                    last_action: shown(s.last_action.as_deref().unwrap_or("")),
                    last_test: shown(s.last_test.as_deref().unwrap_or("")),
                    failures: s.failure_count,
                    refusals: s.consecutive_refusals,
                    worktree: s.worktree.as_deref().map(shown),
                    ..TaskRow::default()
                }
            }
            // A journal that cannot be read is shown as such, not hidden.
            Err(_) => TaskRow {
                id: id.clone(),
                is_task: true,
                status: "unreadable".to_owned(),
                ..TaskRow::default()
            },
        };
        by_id.insert(id, row);
    }

    let mut leftover = Vec::new();
    let mut looked_up = 0;
    for row in rows {
        let target = if row.is_task {
            Some(row.id.clone())
        } else if looked_up < MAX_SESSIONS {
            looked_up += 1;
            state.session_task(&row.id).ok().flatten()
        } else {
            None
        };
        match target.and_then(|id| by_id.get_mut(&id)) {
            Some(task) => fold_events(task, row),
            None => leftover.push(row),
        }
    }
    let mut out: Vec<TaskRow> = by_id.into_values().chain(leftover).collect();
    out.sort_by(|a, b| (&b.last, &b.id).cmp(&(&a.last, &a.id)));
    out
}

/// Adds the event counts of `from` (a session or task id of the logs) to `task`.
fn fold_events(task: &mut TaskRow, from: TaskRow) {
    if !from.is_task {
        task.sessions.push(from.id);
    }
    task.events += from.events;
    task.allowed += from.allowed;
    task.blocked += from.blocked;
    task.failed += from.failed;
    if task.first.is_empty() || from.first < task.first {
        task.first = from.first;
    }
    if from.last >= task.last {
        task.last = from.last;
        task.last_kind = from.last_kind;
        task.last_status = from.last_status;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::tui::events::{Decision, EventView};

    fn ev(time: &str, task: Option<&str>, session: Option<&str>, d: Option<Decision>) -> EventView {
        EventView {
            time: time.to_owned(),
            kind: "permission".to_owned(),
            task_id: task.map(str::to_owned),
            session_id: session.map(str::to_owned),
            tool: None,
            tool_class: None,
            decision: d,
            status: None,
            actor: None,
            model: None,
            duration_ms: None,
            rule: None,
            reasons: vec![],
            command: None,
            paths: vec![],
            error: None,
            file: "x",
        }
    }

    #[test]
    fn events_group_by_task_then_session_and_count_decisions() {
        let log = LogRead {
            events: vec![
                ev("1", Some("t1"), Some("s1"), Some(Decision::Allow)),
                ev("2", Some("t1"), None, Some(Decision::Deny)),
                ev("3", None, Some("s1"), Some(Decision::Ask)),
                ev("4", None, None, Some(Decision::Deny)),
            ],
            ..LogRead::default()
        };
        let rows = group_tasks(&log);
        assert_eq!(rows.len(), 2, "events with no id are not grouped");
        let t1 = rows.iter().find(|r| r.id == "t1").expect("t1");
        assert!(t1.is_task && t1.events == 2 && t1.allowed == 1 && t1.blocked == 1);
        let s1 = rows.iter().find(|r| r.id == "s1").expect("s1");
        assert!(!s1.is_task && s1.blocked == 1, "ask counts as blocked");
        assert_eq!(rows[0].id, "s1", "newest first");
    }

    /// Every file under `dir` with its size and modification time, to compare before and after.
    fn listing(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap().flatten() {
                let meta = entry.metadata().unwrap();
                if meta.is_dir() {
                    stack.push(entry.path());
                } else {
                    out.push((entry.path(), meta.len(), meta.modified().unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn real_tasks_are_read_without_writing_and_fold_in_their_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path();
        let state = StateDir::open(ws).unwrap();
        let task_id = state.bind_session("ses_a", "repo").unwrap();
        drop(state);
        let log = LogRead {
            events: vec![
                ev("1", None, Some("ses_a"), Some(Decision::Deny)),
                ev("2", None, Some("ses_a"), Some(Decision::Allow)),
                ev("3", None, Some("ses_other"), Some(Decision::Allow)),
            ],
            ..LogRead::default()
        };
        let before = listing(ws);
        let rows = collect_tasks(ws, &log);
        assert_eq!(
            before,
            listing(ws),
            "reading tasks must not change any file"
        );

        assert_eq!(rows.len(), 2, "{rows:?}");
        let task = rows.iter().find(|r| r.id == task_id).expect("the task");
        assert!(task.is_task && task.status == "active", "{task:?}");
        assert_eq!(task.sessions, vec!["ses_a".to_owned()]);
        assert_eq!((task.events, task.allowed, task.blocked), (2, 1, 1));
        let session = rows
            .iter()
            .find(|r| r.id == "ses_other")
            .expect("the session");
        assert!(!session.is_task && session.events == 1);
    }

    #[test]
    fn no_state_directory_means_no_tasks_and_nothing_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(collect_tasks(dir.path(), &LogRead::default()).is_empty());
        assert!(!dir.path().join(identity::STATE_DIR).exists());
    }

    #[test]
    fn credentials_in_an_endpoint_are_not_displayed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = dir.path().join("cfg.toml");
        std::fs::write(
            &cfg,
            "[models.main]\nprovider = \"mock\"\nid = \"m\"\nrole = \"engineer\"\nendpoint = \"http://user:hunter2secretvalue@127.0.0.1:9/v1?api_key=sk-live-0123456789abcdefghij\"\ncontext = 4096\nhardware = \"cpu\"\n",
        )
        .expect("write");
        let snap = Snapshot::collect(&LoadArgs {
            config: Some(cfg),
            workspace: Some(dir.path().to_path_buf()),
        });
        let shown_text = format!("{:?}{:?}{:?}", snap.slots, snap.network, snap.config_lines);
        assert!(
            !shown_text.contains("sk-live-0123456789abcdefghij"),
            "api key shown"
        );
    }

    #[test]
    fn config_lines_are_tagged_with_their_origin() {
        let origins = BTreeMap::from([
            ("profile".to_owned(), Origin::Machine),
            ("agent".to_owned(), Origin::Repository),
            ("agent.max_iterations".to_owned(), Origin::Repository),
            ("models".to_owned(), Origin::Machine),
            ("models.main".to_owned(), Origin::Machine),
            ("models.main.id".to_owned(), Origin::Machine),
            ("models.main.endpoint".to_owned(), Origin::Explicit),
            ("models.main.launch".to_owned(), Origin::Machine),
            ("paths.models_dir".to_owned(), Origin::Environment),
        ]);
        let text = "profile = \"x\"\n\n[agent]\nmax_iterations = 3\nmax_same_failure = 2\n\n[models.main]\nendpoint = \"e\"\nid = \"m\"\n\n[models.main.launch]\nextra_args = [\n    \"--a = b\",\n]\n\n[paths]\nmodels_dir = \"/d\"\n";
        let tag = |needle: &str| {
            annotate(text, &origins)
                .into_iter()
                .find(|l| l.text.contains(needle))
                .map(|l| l.origin)
                .unwrap_or_default()
        };
        assert_eq!(tag("profile"), "machine");
        assert_eq!(tag("max_iterations"), "repo");
        assert_eq!(tag("max_same_failure"), "default", "never set by a file");
        assert_eq!(tag("endpoint"), "--config");
        assert_eq!(tag("id = "), "machine");
        assert_eq!(tag("[agent]"), "", "headers carry no tag");
        assert_eq!(
            tag("\"--a = b\""),
            "machine",
            "array items follow their key"
        );
        assert_eq!(tag("models_dir"), "env");
        assert_eq!(
            annotate("[tools]\nx = 1\n", &BTreeMap::new())[1].origin,
            "default"
        );
    }

    #[test]
    fn collect_reports_a_broken_config_instead_of_failing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = dir.path().join("bad.toml");
        std::fs::write(&cfg, "this is = = not toml").expect("write");
        let snap = Snapshot::collect(&LoadArgs {
            config: Some(cfg),
            workspace: Some(dir.path().to_path_buf()),
        });
        assert!(snap.config_error.is_some());
        assert!(snap.workspace.is_some(), "events can still be read");
        assert!(snap.slots.is_empty());
    }

    #[test]
    fn collect_reads_the_real_config_events_and_slots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = dir.path().join("cfg.toml");
        std::fs::write(
            &cfg,
            "[models.main]\nprovider = \"mock\"\nid = \"m\"\nrole = \"engineer\"\nendpoint = \"http://127.0.0.1:9/v1\"\ncontext = 4096\nhardware = \"cpu\"\n",
        )
        .expect("write");
        let state = dir.path().join(identity::STATE_DIR);
        std::fs::create_dir_all(&state).expect("mkdir");
        std::fs::write(
            state.join("tool-events.jsonl"),
            "{\"timestamp\":\"2026-01-01T00:00:00.000Z\",\"kind\":\"permission\",\"verdict\":\"deny\",\"session_id\":\"s\"}\n",
        )
        .expect("write");
        let snap = Snapshot::collect(&LoadArgs {
            config: Some(cfg),
            workspace: Some(dir.path().to_path_buf()),
        });
        assert!(snap.config_error.is_none(), "{:?}", snap.config_error);
        assert_eq!(snap.slots.len(), 1);
        assert!(snap.slots[0].loopback);
        assert_eq!(snap.slots[0].server, ServerState::NotManaged);
        assert_eq!(snap.tasks.len(), 1);
        assert!(
            snap.config_lines
                .iter()
                .any(|l| l.text.starts_with("endpoint") && l.origin == "--config")
        );
    }
}
