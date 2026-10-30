//! Application state, navigation and key handling. Pure: no terminal, no I/O. Keys produce
//! [`Effect`]s (quit, or run a background job); rendering lives in `ui` and reads this state.
//!
//! Mutating operations (start or stop a model server) never run directly from a key: they first
//! open a confirmation overlay, and only an explicit `y` there produces the job.

use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::commands::models::ServerState;
use crate::tui::chat::{ChatMsg, ChatState, Outcome, Turn};
use crate::tui::data::{Sev, Snapshot, TaskRow};
use crate::tui::diag::{self, DiagItem};
use crate::tui::jobs::{Job, JobMsg, SlotHealth};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Chat,
    Overview,
    Tasks,
    Activity,
    Policy,
    Models,
    Diagnostics,
    Config,
    Network,
}

impl View {
    pub const ALL: [View; 9] = [
        View::Chat,
        View::Overview,
        View::Tasks,
        View::Activity,
        View::Policy,
        View::Models,
        View::Diagnostics,
        View::Config,
        View::Network,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Overview => "Overview",
            Self::Tasks => "Tasks",
            Self::Activity => "Activity",
            Self::Policy => "Policy",
            Self::Models => "Models",
            Self::Diagnostics => "Diagnostics",
            Self::Config => "Configuration",
            Self::Network => "Network",
        }
    }

    /// A shorter name for narrow navigation rows.
    pub fn short(self) -> &'static str {
        match self {
            Self::Diagnostics => "Checks",
            Self::Config => "Config",
            other => other.title(),
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Nav,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Start(String),
    Stop(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help,
    Confirm(Confirm),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionFilter {
    Blocked,
    Allowed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub text: String,
    pub decision: Option<DecisionFilter>,
    /// Only events of this task or session id.
    pub group: Option<String>,
}

impl Filter {
    pub fn is_active(&self) -> bool {
        !self.text.is_empty() || self.decision.is_some() || self.group.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    View(View),
    Slot(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvRow {
    pub sev: Sev,
    pub text: String,
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Quit,
    Run(Job),
    /// A chat message is queued: take it with [`Ui::take_chat_turn`] and start the turn.
    ChatSend,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DoctorState {
    pub items: Vec<DiagItem>,
    pub blocking: bool,
    pub at: String,
}

#[derive(Debug)]
pub struct Ui {
    pub snap: Snapshot,
    pub loaded: bool,
    pub view: View,
    pub focus: Focus,
    pub overlay: Option<Overlay>,
    pub sel: [usize; 9],
    pub chat: ChatState,
    pub filter: Filter,
    pub editing_search: bool,
    pub policy_blocked_only: bool,
    pub health: HashMap<String, SlotHealth>,
    pub doctor: Option<DoctorState>,
    pub busy: Option<Job>,
    pub refreshing: bool,
    /// Re-read local state when the watched files change (toggled with `p`).
    pub auto: bool,
    pub notice: Option<(Sev, String)>,
}

/// Rows moved by PageUp/PageDown.
const PAGE: usize = 10;

impl Ui {
    pub fn new() -> Self {
        Self {
            snap: Snapshot::default(),
            loaded: false,
            view: View::Chat,
            focus: Focus::Content,
            overlay: None,
            sel: [0; 9],
            chat: ChatState::new(),
            filter: Filter::default(),
            editing_search: false,
            policy_blocked_only: false,
            health: HashMap::new(),
            doctor: None,
            busy: None,
            refreshing: false,
            auto: true,
            notice: None,
        }
    }

    // ---- derived lists -------------------------------------------------------------------

    /// Indices into `snap.log.events` that pass the filter, newest first.
    pub fn visible_events(&self) -> Vec<usize> {
        let needle = self.filter.text.to_lowercase();
        let group_ids = self.filter.group.as_deref().map(|g| self.group_ids(g));
        (0..self.snap.log.events.len())
            .rev()
            .filter(|&i| {
                let e = &self.snap.log.events[i];
                let by_decision = match self.filter.decision {
                    None => true,
                    Some(DecisionFilter::Blocked) => e.is_blocked(),
                    Some(DecisionFilter::Allowed) => e.decision.is_some() && !e.is_blocked(),
                };
                let by_group = group_ids.as_ref().is_none_or(|ids| {
                    ids.iter().any(|id| {
                        e.task_id.as_deref() == Some(id.as_str())
                            || e.session_id.as_deref() == Some(id.as_str())
                    })
                });
                let by_text = needle.is_empty()
                    || [
                        Some(e.kind.as_str()),
                        e.tool.as_deref(),
                        e.status.as_deref(),
                        e.rule.as_deref(),
                        e.command.as_deref(),
                        e.error.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .chain(e.reasons.iter().map(String::as_str))
                    .any(|s| s.to_lowercase().contains(&needle));
                by_decision && by_group && by_text
            })
            .collect()
    }

    /// The ids an event may carry to belong to `group`: the task id and the sessions bound to it.
    fn group_ids(&self, group: &str) -> Vec<String> {
        let mut ids = vec![group.to_owned()];
        if let Some(task) = self.snap.tasks.iter().find(|t| t.id == group) {
            ids.extend(task.sessions.iter().cloned());
        }
        ids
    }

    /// The task (or session) an event belongs to, as the Tasks view names it.
    fn canonical_group(&self, event: &crate::tui::events::EventView) -> Option<String> {
        let (id, _) = event.group_key()?;
        let task = self
            .snap
            .tasks
            .iter()
            .find(|t| t.id == id || t.sessions.iter().any(|s| s == id));
        Some(task.map_or_else(|| id.to_owned(), |t| t.id.clone()))
    }

    /// Slot names in configuration order.
    pub fn slot_names(&self) -> Vec<String> {
        self.snap.slots.iter().map(|s| s.name.clone()).collect()
    }

    /// Whether typed characters go to the chat input (so global letter shortcuts must not fire).
    pub fn in_chat_input(&self) -> bool {
        self.view == View::Chat
            && self.focus == Focus::Content
            && self.overlay.is_none()
            && !self.editing_search
    }

    /// Text pasted into the terminal.
    pub fn handle_paste(&mut self, text: &str) {
        if self.in_chat_input() {
            self.chat.paste(text);
        }
    }

    /// The turn queued by the last Enter, ready for a worker thread.
    pub fn take_chat_turn(&mut self) -> Option<Turn> {
        let slots = self.slot_names();
        let default = self.snap.default_slot.clone();
        self.chat.take_turn(&slots, default.as_deref())
    }

    /// Tasks (or sessions) matching the search text.
    pub fn visible_tasks(&self) -> Vec<&TaskRow> {
        let needle = self.filter.text.to_lowercase();
        self.snap
            .tasks
            .iter()
            .filter(|t| needle.is_empty() || t.id.to_lowercase().contains(&needle))
            .collect()
    }

    /// Decision events (allow, ask or deny), newest first.
    pub fn policy_events(&self) -> Vec<usize> {
        (0..self.snap.log.events.len())
            .rev()
            .filter(|&i| {
                let e = &self.snap.log.events[i];
                e.decision.is_some() && (!self.policy_blocked_only || e.is_blocked())
            })
            .collect()
    }

    /// The newest decisions, for the Overview.
    pub fn overview_decisions(&self) -> Vec<usize> {
        self.policy_events().into_iter().take(8).collect()
    }

    pub fn diag_items(&self) -> Vec<DiagItem> {
        let mut items = Vec::new();
        if let Some(c) = &self.snap.config_error {
            items.push(DiagItem {
                sev: Sev::Fail,
                category: "config",
                title: "configuration could not be loaded".to_owned(),
                detail: c.clone(),
                impact: "Nothing that needs the configuration can run.",
                next: "Fix the file named in the message, then refresh (r).",
            });
        }
        match &self.doctor {
            Some(d) => items.extend(d.items.iter().cloned()),
            None => {
                // Without a doctor run, still show the config findings we already have.
                for i in &self.snap.issues {
                    items.push(DiagItem {
                        sev: if i.error { Sev::Fail } else { Sev::Warn },
                        category: "config",
                        title: format!("[{}] {}", i.rule, i.path),
                        detail: i.message.clone(),
                        impact: "Errors block `agent models serve` and the hook; warnings are advisory.",
                        next: "Edit the config file (this interface is read-only) and refresh (r).",
                    });
                }
            }
        }
        let mut slots: Vec<_> = self.health.keys().collect();
        slots.sort();
        for slot in slots {
            items.extend(diag::from_health(slot, &self.health[slot].checks));
        }
        items.sort_by_key(|i| std::cmp::Reverse(i.sev));
        items
    }

    /// Overall readiness and the single most important reason.
    pub fn readiness(&self) -> (Sev, &'static str, String) {
        if !self.loaded {
            return (Sev::Unknown, "Loading", "reading local state…".to_owned());
        }
        if let Some(e) = &self.snap.config_error {
            return (
                Sev::Fail,
                "Not ready",
                format!("configuration cannot be loaded: {e}"),
            );
        }
        if self.snap.error_count() > 0 {
            let first = self.snap.issues.iter().find(|i| i.error);
            let detail = first.map_or(String::new(), |i| format!("[{}] {}", i.rule, i.message));
            return (
                Sev::Fail,
                "Not ready",
                format!(
                    "{} configuration error(s): {detail}",
                    self.snap.error_count()
                ),
            );
        }
        if self.snap.slots.is_empty() {
            return (
                Sev::Fail,
                "Not ready",
                "no model slots are configured".to_owned(),
            );
        }
        if let Some(s) = self.snap.slots.iter().find(|s| s.static_sev() == Sev::Fail) {
            return (
                Sev::Fail,
                "Not ready",
                format!("slot `{}`: model file or runtime is missing", s.name),
            );
        }
        if let Some((slot, _)) = self
            .health
            .iter()
            .find(|(_, h)| h.status == harness_runtime::health::Status::Fail)
        {
            return (
                Sev::Fail,
                "Not ready",
                format!("health check failed for `{slot}`"),
            );
        }
        let unverified = self
            .snap
            .slots
            .iter()
            .any(|s| !self.health.contains_key(&s.name));
        if unverified {
            return (
                Sev::Unknown,
                "Not checked",
                "the files look right, but the servers have not been checked. Press h".to_owned(),
            );
        }
        if self
            .health
            .values()
            .any(|h| h.status == harness_runtime::health::Status::Warn)
        {
            return (
                Sev::Warn,
                "Needs attention",
                "a health check reported warnings".to_owned(),
            );
        }
        if self.snap.warning_count() > 0 {
            return (
                Sev::Warn,
                "Needs attention",
                format!("{} configuration warning(s)", self.snap.warning_count()),
            );
        }
        (
            Sev::Ok,
            "Ready",
            "the configuration is valid, the files are present and the servers answered".to_owned(),
        )
    }

    /// What to do next, in plain words, for the Overview. Empty when there is nothing to suggest.
    pub fn next_steps(&self) -> Vec<String> {
        if !self.loaded {
            return Vec::new();
        }
        if self.snap.config_error.is_some() {
            return vec![
                "Open Diagnostics (key 7): it names the problem in the configuration and how to fix it."
                    .to_owned(),
            ];
        }
        if self.snap.slots.is_empty() {
            return vec![
                "No model is configured yet. Create your machine settings once: agent init --machine --models-dir <weights folder> --runtimes-dir <runtimes folder>"
                    .to_owned(),
                "Then, in your repository: agent init --profile local-only".to_owned(),
                "Details: docs/guide/installation.md".to_owned(),
            ];
        }
        let mut steps = Vec::new();
        if self.snap.error_count() > 0 {
            steps.push("Fix the configuration errors: open Diagnostics (key 7).".to_owned());
        }
        if let Some(slot) = self.snap.slots.iter().find(|s| {
            s.launchable
                && s.static_sev() != Sev::Fail
                && matches!(
                    s.server,
                    ServerState::NotManaged | ServerState::Stale { .. }
                )
        }) {
            steps.push(format!(
                "Start the `{}` server: open Models (key 6), select it and press s.",
                slot.name
            ));
        }
        if self.health.is_empty() {
            steps.push("Check that the servers answer: press h.".to_owned());
        }
        if steps.is_empty() && self.chat.items.is_empty() {
            steps.push("Everything looks ready. Open Chat (key 1) and ask something.".to_owned());
        }
        steps
    }

    /// Selectable status rows at the top of the Overview.
    pub fn overview_rows(&self) -> Vec<OvRow> {
        let mut rows = Vec::new();
        if let Some(e) = &self.snap.config_error {
            rows.push(OvRow {
                sev: Sev::Fail,
                text: format!("configuration: cannot load ({e})"),
                target: Target::View(View::Diagnostics),
            });
        } else {
            let (sev, text) = match (self.snap.error_count(), self.snap.warning_count()) {
                (0, 0) => (Sev::Ok, "configuration: valid".to_owned()),
                (e, w) => (
                    if e > 0 { Sev::Fail } else { Sev::Warn },
                    format!("configuration: {e} error(s), {w} warning(s)"),
                ),
            };
            rows.push(OvRow {
                sev,
                text,
                target: Target::View(if sev == Sev::Ok {
                    View::Config
                } else {
                    View::Diagnostics
                }),
            });
        }
        for (i, s) in self.snap.slots.iter().enumerate() {
            let (sev, text) = self.slot_summary(i);
            rows.push(OvRow {
                sev,
                text: format!("model {}: {text}", s.name),
                target: Target::Slot(i),
            });
        }
        let (sev, text) = match &self.doctor {
            None => (Sev::Unknown, "diagnostics: not run (press d)".to_owned()),
            Some(d) if d.blocking => (
                Sev::Fail,
                format!("diagnostics: blocking problems ({})", d.at),
            ),
            Some(d) => (
                Sev::Ok,
                format!("diagnostics: no blocking problems ({})", d.at),
            ),
        };
        rows.push(OvRow {
            sev,
            text,
            target: Target::View(View::Diagnostics),
        });
        rows
    }

    /// Severity and text for one slot, combining files, server record and last health result.
    pub fn slot_summary(&self, i: usize) -> (Sev, String) {
        let s = &self.snap.slots[i];
        let files = match (&s.model_state, &s.runtime_state) {
            (_, _) if !s.launchable => "not launched by Bridle".to_owned(),
            (crate::tui::data::FileState::Missing, _) => "model file missing".to_owned(),
            (_, crate::tui::data::FileState::Missing) => "runtime missing".to_owned(),
            (crate::tui::data::FileState::Unresolved, _)
            | (_, crate::tui::data::FileState::Unresolved) => "path not resolvable".to_owned(),
            _ => "files present".to_owned(),
        };
        let server = match &s.server {
            ServerState::NotManaged => "no managed server".to_owned(),
            ServerState::Running { pid } => format!("managed server running (pid {pid})"),
            ServerState::Stale { pid } => format!("stale record (pid {pid})"),
            ServerState::Reused { pid, image } => format!("pid {pid} reused by `{image}`"),
            ServerState::Unreadable(e) => format!("record unreadable ({e})"),
        };
        let (hsev, health) = match self.health.get(&s.name) {
            None => (Sev::Unknown, "health not checked".to_owned()),
            Some(h) => (
                match h.status {
                    harness_runtime::health::Status::Pass => Sev::Ok,
                    harness_runtime::health::Status::Warn => Sev::Warn,
                    harness_runtime::health::Status::Fail => Sev::Fail,
                    harness_runtime::health::Status::Skip => Sev::Unknown,
                },
                format!("health {:?} ({})", h.status, h.at).to_lowercase(),
            ),
        };
        (
            s.static_sev().max(hsev),
            format!("{files}; {server}; {health}"),
        )
    }

    /// Identity of the selected record in `view`, for keeping it selected across refreshes.
    /// `None` when the selection is the first row of a newest-first list (it follows new rows).
    fn identity_of(&self, view: View) -> Option<String> {
        let sel = self.sel[view.index()];
        let event_id = |i: usize| {
            let e = &self.snap.log.events[i];
            format!("{}|{}|{:?}|{}", e.time, e.kind, e.tool, e.summary())
        };
        match view {
            View::Activity if sel > 0 => self.visible_events().get(sel).map(|&i| event_id(i)),
            View::Policy if sel > 0 => self.policy_events().get(sel).map(|&i| event_id(i)),
            View::Tasks => self.visible_tasks().get(sel).map(|t| t.id.clone()),
            View::Models => self.snap.slots.get(sel).map(|s| s.name.clone()),
            _ => None,
        }
    }

    fn position_of(&self, view: View, id: &str) -> Option<usize> {
        let event_id = |i: usize| {
            let e = &self.snap.log.events[i];
            format!("{}|{}|{:?}|{}", e.time, e.kind, e.tool, e.summary())
        };
        match view {
            View::Activity => self
                .visible_events()
                .into_iter()
                .position(|i| event_id(i) == id),
            View::Policy => self
                .policy_events()
                .into_iter()
                .position(|i| event_id(i) == id),
            View::Tasks => self.visible_tasks().iter().position(|t| t.id == id),
            View::Models => self.snap.slots.iter().position(|s| s.name == id),
            _ => None,
        }
    }

    fn list_len(&self) -> usize {
        match self.view {
            View::Chat => 0,
            View::Overview => self.overview_rows().len() + self.overview_decisions().len(),
            View::Tasks => self.visible_tasks().len(),
            View::Activity => self.visible_events().len(),
            View::Policy => self.policy_events().len(),
            View::Models => self.snap.slots.len(),
            View::Diagnostics => self.diag_items().len(),
            View::Config => self.snap.config_lines.len(),
            View::Network => self.snap.network.len(),
        }
    }

    /// Keeps every selection inside its list (after data or filters change).
    pub fn clamp(&mut self) {
        let saved = self.view;
        for v in View::ALL {
            self.view = v;
            let len = self.list_len();
            self.sel[v.index()] = self.sel[v.index()].min(len.saturating_sub(1));
        }
        self.view = saved;
    }

    pub fn selected(&self) -> usize {
        self.sel[self.view.index()]
    }

    fn set_selected(&mut self, n: usize) {
        let max = self.list_len().saturating_sub(1);
        self.sel[self.view.index()] = n.min(max);
    }

    // ---- job results ---------------------------------------------------------------------

    pub fn apply(&mut self, msg: JobMsg) -> Vec<Effect> {
        let mut effects = Vec::new();
        match msg {
            JobMsg::Chat(msg) => {
                let finished = matches!(msg, ChatMsg::Finished { .. } | ChatMsg::Failed(_));
                self.chat.apply(msg);
                // A turn changes the logs and the task journal: read them now, not in two seconds.
                if finished {
                    effects.extend(self.request(Job::Refresh));
                }
            }
            JobMsg::Snapshot(snap) => {
                // Keep each list's selection on the same record when rows are added above it.
                let kept: Vec<(View, String)> =
                    [View::Activity, View::Policy, View::Tasks, View::Models]
                        .into_iter()
                        .filter_map(|v| self.identity_of(v).map(|id| (v, id)))
                        .collect();
                self.snap = *snap;
                self.loaded = true;
                for (v, id) in kept {
                    if let Some(pos) = self.position_of(v, &id) {
                        self.sel[v.index()] = pos;
                    }
                }
            }
            JobMsg::Doctor {
                items,
                blocking,
                at,
            } => {
                self.doctor = Some(DoctorState {
                    items,
                    blocking,
                    at,
                });
            }
            JobMsg::Health { slot, health } => {
                self.health.insert(slot, health);
            }
            JobMsg::Finished { job, outcome } => {
                if job.is_refresh() {
                    self.refreshing = false;
                } else {
                    self.busy = None;
                }
                let (sev, text) = match outcome {
                    Ok(t) => (Sev::Ok, t),
                    Err(t) => (Sev::Fail, t),
                };
                if !(job.is_refresh() && sev == Sev::Ok) {
                    self.notice = Some((sev, format!("{}: {text}", job.label())));
                }
                // A start or stop changes the server records: re-read local state.
                if matches!(job, Job::Start { .. } | Job::Stop { .. }) {
                    effects.extend(self.request(Job::Refresh));
                }
            }
        }
        self.clamp();
        effects
    }

    /// Queues `job` unless the same kind of work is already running.
    pub fn request(&mut self, job: Job) -> Vec<Effect> {
        if job.is_refresh() {
            if self.refreshing {
                return vec![];
            }
            self.refreshing = true;
        } else if let Some(running) = &self.busy {
            self.notice = Some((Sev::Warn, format!("{} is still running", running.label())));
            return vec![];
        } else {
            self.busy = Some(job.clone());
        }
        vec![Effect::Run(job)]
    }

    // ---- keys ----------------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            // While a chat turn runs, Ctrl+C stops it instead of leaving the program.
            if self.chat.running {
                self.chat.stop();
                return vec![];
            }
            if self.in_chat_input() && !self.chat.input.is_empty() {
                self.chat.clear_input();
                return vec![];
            }
            return vec![Effect::Quit];
        }
        // A notice lasts until the next key press.
        self.notice = None;
        if let Some(overlay) = self.overlay.clone() {
            return self.overlay_key(&overlay, key);
        }
        if self.editing_search {
            self.search_key(key);
            return vec![];
        }
        if self.in_chat_input() {
            let slots = self.slot_names();
            let default = self.snap.default_slot.clone();
            match self.chat.key(key, &slots, default.as_deref()) {
                Outcome::Handled => return vec![],
                Outcome::Send => return vec![Effect::ChatSend],
                Outcome::Quit => return vec![Effect::Quit],
                Outcome::Nav => {
                    self.focus = Focus::Nav;
                    return vec![];
                }
                Outcome::Pass => {}
            }
        }
        match key.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Char('?') | KeyCode::F(1) => self.overlay = Some(Overlay::Help),
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = if self.focus == Focus::Nav {
                    Focus::Content
                } else {
                    Focus::Nav
                };
            }
            KeyCode::Char(c @ '1'..='9') => {
                let i = usize::from(c as u8 - b'1');
                self.go(View::ALL[i]);
                // "Go to this view" means "work in it": the next key goes to the screen.
                self.focus = Focus::Content;
            }
            KeyCode::Char('r') => return self.request(Job::Refresh),
            KeyCode::Char('p') => {
                self.auto = !self.auto;
                let state = if self.auto { "on" } else { "paused" };
                self.notice = Some((Sev::Info, format!("auto-refresh {state}")));
            }
            KeyCode::Esc => {
                if self.filter.is_active() {
                    self.filter = Filter::default();
                    self.clamp();
                } else {
                    self.focus = Focus::Nav;
                }
            }
            _ if self.focus == Focus::Nav => self.nav_key(key),
            _ => return self.content_key(key),
        }
        vec![]
    }

    fn go(&mut self, view: View) {
        if self.view != view {
            self.view = view;
            self.filter.text.clear();
            self.editing_search = false;
        }
    }

    fn nav_key(&mut self, key: KeyEvent) {
        let i = self.view.index();
        match key.code {
            KeyCode::Up if i > 0 => self.go(View::ALL[i - 1]),
            KeyCode::Down if i + 1 < View::ALL.len() => self.go(View::ALL[i + 1]),
            KeyCode::Enter | KeyCode::Right => self.focus = Focus::Content,
            _ => {}
        }
    }

    fn overlay_key(&mut self, overlay: &Overlay, key: KeyEvent) -> Vec<Effect> {
        match (overlay, key.code) {
            (Overlay::Help, KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?' | 'q')) => {
                self.overlay = None;
            }
            (Overlay::Confirm(_), KeyCode::Char('n') | KeyCode::Esc) => self.overlay = None,
            (Overlay::Confirm(c), KeyCode::Char('y')) => {
                self.overlay = None;
                let job = match c {
                    Confirm::Start(slot) => Job::Start { slot: slot.clone() },
                    Confirm::Stop(slot) => Job::Stop { slot: slot.clone() },
                };
                return self.request(job);
            }
            _ => {}
        }
        vec![]
    }

    fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filter.text.clear();
                self.editing_search = false;
            }
            KeyCode::Enter => self.editing_search = false,
            KeyCode::Backspace => {
                self.filter.text.pop();
            }
            KeyCode::Char(c) if !c.is_control() && self.filter.text.chars().count() < 80 => {
                self.filter.text.push(c);
            }
            _ => {}
        }
        self.set_selected(0);
    }

    fn content_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let sel = self.selected();
        match key.code {
            KeyCode::Up => self.set_selected(sel.saturating_sub(1)),
            KeyCode::Down => self.set_selected(sel + 1),
            KeyCode::PageUp => self.set_selected(sel.saturating_sub(PAGE)),
            KeyCode::PageDown => self.set_selected(sel + PAGE),
            KeyCode::Home => self.set_selected(0),
            KeyCode::End => self.set_selected(usize::MAX),
            KeyCode::Enter => self.activate(),
            KeyCode::Char('/') if matches!(self.view, View::Activity | View::Tasks) => {
                self.editing_search = true;
            }
            KeyCode::Char(c) => return self.view_key(c),
            _ => {}
        }
        vec![]
    }

    /// Opens the thing the selected row refers to.
    fn activate(&mut self) {
        let sel = self.selected();
        match self.view {
            View::Overview => {
                let rows = self.overview_rows();
                if let Some(row) = rows.get(sel) {
                    match row.target {
                        Target::View(v) => self.go(v),
                        Target::Slot(i) => {
                            self.go(View::Models);
                            self.set_selected(i);
                        }
                    }
                } else if let Some(&idx) = self.overview_decisions().get(sel - rows.len()) {
                    self.goto_event(idx);
                }
            }
            View::Tasks => {
                if let Some(id) = self.visible_tasks().get(sel).map(|t| t.id.clone()) {
                    self.go(View::Activity);
                    self.filter = Filter {
                        group: Some(id),
                        ..Filter::default()
                    };
                    self.set_selected(0);
                }
            }
            View::Activity => {
                let id = self
                    .visible_events()
                    .get(sel)
                    .and_then(|&i| self.canonical_group(&self.snap.log.events[i]));
                if let Some(id) = id
                    && let Some(pos) = self.visible_tasks_all().iter().position(|t| t.id == id)
                {
                    self.go(View::Tasks);
                    self.set_selected(pos);
                }
            }
            View::Policy => {
                if let Some(&idx) = self.policy_events().get(sel) {
                    self.goto_event(idx);
                }
            }
            _ => {}
        }
    }

    fn visible_tasks_all(&self) -> &[TaskRow] {
        &self.snap.tasks
    }

    /// Shows event `idx` (an index into `snap.log.events`) in the Activity view, unfiltered.
    fn goto_event(&mut self, idx: usize) {
        self.go(View::Activity);
        self.filter = Filter::default();
        let total = self.snap.log.events.len();
        self.set_selected(total.saturating_sub(1).saturating_sub(idx));
    }

    fn view_key(&mut self, c: char) -> Vec<Effect> {
        let slot = self.snap.slots.get(self.selected()).map(|s| s.name.clone());
        match (self.view, c) {
            (View::Activity, 'v') => {
                self.filter.decision = match self.filter.decision {
                    None => Some(DecisionFilter::Blocked),
                    Some(DecisionFilter::Blocked) => Some(DecisionFilter::Allowed),
                    Some(DecisionFilter::Allowed) => None,
                };
                self.set_selected(0);
            }
            (View::Activity, 't') => {
                let id = self
                    .visible_events()
                    .get(self.selected())
                    .and_then(|&i| self.canonical_group(&self.snap.log.events[i]));
                if id.is_some() {
                    self.filter.group = id;
                    self.set_selected(0);
                }
            }
            (View::Activity, 'c') => {
                self.filter = Filter::default();
                self.clamp();
            }
            (View::Policy, 'v') => {
                self.policy_blocked_only = !self.policy_blocked_only;
                self.set_selected(0);
            }
            (View::Overview | View::Diagnostics, 'd') => return self.request(Job::Doctor),
            (View::Overview | View::Diagnostics, 'h') => {
                return self.request(Job::Health {
                    slot: None,
                    quick: true,
                });
            }
            (View::Diagnostics, 'H') => {
                return self.request(Job::Health {
                    slot: None,
                    quick: false,
                });
            }
            (View::Models, 'h' | 'H') => {
                if let Some(slot) = slot {
                    return self.request(Job::Health {
                        slot: Some(slot),
                        quick: c == 'h',
                    });
                }
            }
            (View::Models, 's') => self.ask_start(slot),
            (View::Models, 'x') => self.ask_stop(slot),
            _ => {}
        }
        vec![]
    }

    fn ask_start(&mut self, slot: Option<String>) {
        let Some(name) = slot else { return };
        let Some(row) = self.snap.slots.iter().find(|s| s.name == name) else {
            return;
        };
        let msg = if !row.launchable {
            Some(format!(
                "`{name}` has no launch section; Bridle cannot start it"
            ))
        } else if matches!(row.server, ServerState::Running { .. }) {
            Some(format!("`{name}` already has a running managed server"))
        } else if !row.loopback {
            Some(format!(
                "`{name}` points at a non-local endpoint; start it yourself"
            ))
        } else if self.snap.error_count() > 0 {
            Some("the configuration has errors; fix them before starting a server".to_owned())
        } else {
            None
        };
        match msg {
            Some(m) => self.notice = Some((Sev::Warn, m)),
            None => self.overlay = Some(Overlay::Confirm(Confirm::Start(name))),
        }
    }

    fn ask_stop(&mut self, slot: Option<String>) {
        let Some(name) = slot else { return };
        let managed = self
            .snap
            .slots
            .iter()
            .find(|s| s.name == name)
            .is_some_and(|s| !matches!(s.server, ServerState::NotManaged));
        if managed {
            self.overlay = Some(Overlay::Confirm(Confirm::Stop(name)));
        } else {
            self.notice = Some((
                Sev::Warn,
                format!(
                    "no server record for `{name}`: Bridle only stops servers it started \
                     (a responding endpoint is not proof of that)"
                ),
            ));
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use super::*;
    use crate::tui::data::{FileState, SlotRow};
    use crate::tui::events::{Decision, EventView, LogRead};

    pub fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    pub fn slot(name: &str, launchable: bool, server: ServerState) -> SlotRow {
        SlotRow {
            name: name.to_owned(),
            model_id: "m".to_owned(),
            role: "engineer".to_owned(),
            provider: "llama_cpp".to_owned(),
            hardware: "gpu".to_owned(),
            endpoint: "http://127.0.0.1:8080/v1".to_owned(),
            loopback: true,
            runtime: Some("llama".to_owned()),
            runtime_bin: Some("/bin/llama-server".to_owned()),
            runtime_state: FileState::Present(None),
            model_file: Some("/m/x.gguf".to_owned()),
            model_state: FileState::Present(Some(1 << 30)),
            server,
            log_path: "/s/logs/main.log".to_owned(),
            launchable,
        }
    }

    pub fn event(time: &str, d: Option<Decision>, task: Option<&str>) -> EventView {
        EventView {
            time: time.to_owned(),
            kind: "permission".to_owned(),
            task_id: task.map(str::to_owned),
            session_id: None,
            tool: Some("bash".to_owned()),
            tool_class: Some("EXECUTE_RISKY".to_owned()),
            decision: d,
            status: None,
            actor: None,
            model: None,
            duration_ms: None,
            rule: Some("R1".to_owned()),
            reasons: vec!["because".to_owned()],
            command: Some("curl x".to_owned()),
            paths: vec![],
            error: None,
            file: "tool-events.jsonl",
        }
    }

    pub fn loaded_ui() -> Ui {
        let mut ui = Ui::new();
        ui.loaded = true;
        ui.snap.slots = vec![slot("main", true, ServerState::NotManaged)];
        ui.snap.log = LogRead {
            events: vec![
                event(
                    "2026-01-01T00:00:01.000Z",
                    Some(Decision::Allow),
                    Some("t1"),
                ),
                event("2026-01-01T00:00:02.000Z", Some(Decision::Deny), Some("t1")),
                event("2026-01-01T00:00:03.000Z", Some(Decision::Deny), Some("t2")),
            ],
            ..LogRead::default()
        };
        ui.snap.tasks = crate::tui::data::group_tasks(&ui.snap.log);
        // Most tests exercise the list screens; the chat has its own tests.
        ui.view = View::Overview;
        ui
    }

    #[test]
    fn number_keys_navigate_and_q_quits() {
        let mut ui = loaded_ui();
        ui.handle_key(key(KeyCode::Char('6')));
        assert_eq!(ui.view, View::Models);
        assert_eq!(ui.handle_key(key(KeyCode::Char('q'))), vec![Effect::Quit]);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(ui.handle_key(ctrl_c), vec![Effect::Quit]);
    }

    #[test]
    fn tab_moves_focus_and_nav_arrows_change_view() {
        let mut ui = loaded_ui();
        ui.handle_key(key(KeyCode::Tab));
        assert_eq!(ui.focus, Focus::Nav);
        ui.handle_key(key(KeyCode::Down));
        assert_eq!(ui.view, View::Tasks);
        ui.handle_key(key(KeyCode::Enter));
        assert_eq!(ui.focus, Focus::Content);
    }

    #[test]
    fn selection_is_clamped_to_the_list() {
        let mut ui = loaded_ui();
        ui.view = View::Activity;
        for _ in 0..10 {
            ui.handle_key(key(KeyCode::Down));
        }
        assert_eq!(ui.selected(), 2);
        ui.handle_key(key(KeyCode::Home));
        assert_eq!(ui.selected(), 0);
    }

    #[test]
    fn activity_filters_by_verdict_text_and_task() {
        let mut ui = loaded_ui();
        ui.view = View::Activity;
        assert_eq!(ui.visible_events(), vec![2, 1, 0], "newest first");
        ui.handle_key(key(KeyCode::Char('v')));
        assert_eq!(ui.visible_events(), vec![2, 1], "blocked only");
        ui.handle_key(key(KeyCode::Char('t')));
        assert_eq!(ui.visible_events(), vec![2], "the selected event's task");
        ui.handle_key(key(KeyCode::Char('c')));
        assert_eq!(ui.visible_events().len(), 3);
        ui.handle_key(key(KeyCode::Char('/')));
        for c in "CURL".chars() {
            ui.handle_key(key(KeyCode::Char(c)));
        }
        assert!(ui.editing_search);
        assert_eq!(ui.visible_events().len(), 3, "case-insensitive");
        ui.handle_key(key(KeyCode::Char('z')));
        assert!(ui.visible_events().is_empty());
        ui.handle_key(key(KeyCode::Esc));
        assert!(!ui.editing_search && ui.visible_events().len() == 3);
    }

    #[test]
    fn typing_in_search_does_not_trigger_shortcuts() {
        let mut ui = loaded_ui();
        ui.view = View::Activity;
        ui.handle_key(key(KeyCode::Char('/')));
        let effects: Vec<_> = "qr5"
            .chars()
            .flat_map(|c| ui.handle_key(key(KeyCode::Char(c))))
            .collect();
        assert!(effects.is_empty());
        assert_eq!(ui.view, View::Activity);
        assert_eq!(ui.filter.text, "qr5");
    }

    #[test]
    fn enter_on_a_task_filters_activity_and_enter_on_an_event_opens_the_task() {
        let mut ui = loaded_ui();
        ui.view = View::Tasks;
        ui.handle_key(key(KeyCode::Enter));
        assert_eq!(ui.view, View::Activity);
        assert_eq!(ui.filter.group.as_deref(), Some("t2"), "newest task first");
        ui.handle_key(key(KeyCode::Enter));
        assert_eq!(ui.view, View::Tasks);
        assert_eq!(ui.visible_tasks()[ui.selected()].id, "t2");
    }

    #[test]
    fn overview_decision_opens_that_event() {
        let mut ui = loaded_ui();
        let rows = ui.overview_rows().len();
        ui.sel[View::Overview.index()] = rows + 1; // second newest decision = event 1
        ui.handle_key(key(KeyCode::Enter));
        assert_eq!(ui.view, View::Activity);
        let visible = ui.visible_events();
        assert_eq!(visible[ui.selected()], 1);
    }

    #[test]
    fn start_requires_confirmation_and_stop_needs_a_record() {
        let mut ui = loaded_ui();
        ui.view = View::Models;
        assert!(ui.handle_key(key(KeyCode::Char('s'))).is_empty());
        assert_eq!(
            ui.overlay,
            Some(Overlay::Confirm(Confirm::Start("main".into())))
        );
        // Enter and other keys do not confirm.
        assert!(ui.handle_key(key(KeyCode::Enter)).is_empty());
        assert!(ui.overlay.is_some());
        assert_eq!(
            ui.handle_key(key(KeyCode::Char('y'))),
            vec![Effect::Run(Job::Start {
                slot: "main".into()
            })]
        );
        assert!(ui.overlay.is_none() && ui.busy.is_some());
        // Stop without a managed server only explains.
        ui.busy = None;
        ui.handle_key(key(KeyCode::Char('x')));
        assert!(ui.overlay.is_none() && ui.notice.is_some());
    }

    #[test]
    fn cancelling_a_confirmation_runs_nothing() {
        let mut ui = loaded_ui();
        ui.view = View::Models;
        ui.snap.slots[0].server = ServerState::Running { pid: 7 };
        ui.handle_key(key(KeyCode::Char('x')));
        assert_eq!(
            ui.overlay,
            Some(Overlay::Confirm(Confirm::Stop("main".into())))
        );
        assert!(ui.handle_key(key(KeyCode::Char('n'))).is_empty());
        assert!(ui.overlay.is_none() && ui.busy.is_none());
    }

    #[test]
    fn a_second_job_is_refused_while_one_runs() {
        let mut ui = loaded_ui();
        assert_eq!(ui.request(Job::Doctor).len(), 1);
        assert!(ui.request(Job::Doctor).is_empty());
        assert!(ui.notice.is_some());
        assert_eq!(ui.request(Job::Refresh).len(), 1);
        assert!(
            ui.request(Job::Refresh).is_empty(),
            "refresh is not duplicated either"
        );
    }

    #[test]
    fn start_refuses_running_non_launchable_and_non_local_slots() {
        let mut ui = loaded_ui();
        ui.view = View::Models;
        ui.snap.slots[0].server = ServerState::Running { pid: 1 };
        ui.handle_key(key(KeyCode::Char('s')));
        assert!(ui.overlay.is_none() && ui.notice.is_some());
        ui.snap.slots[0].server = ServerState::NotManaged;
        ui.snap.slots[0].loopback = false;
        ui.handle_key(key(KeyCode::Char('s')));
        assert!(ui.overlay.is_none());
        ui.snap.slots[0].loopback = true;
        ui.snap.slots[0].launchable = false;
        ui.handle_key(key(KeyCode::Char('s')));
        assert!(ui.overlay.is_none());
    }

    #[test]
    fn finishing_a_start_refreshes_local_state() {
        let mut ui = loaded_ui();
        ui.busy = Some(Job::Start {
            slot: "main".into(),
        });
        let fx = ui.apply(JobMsg::Finished {
            job: Job::Start {
                slot: "main".into(),
            },
            outcome: Ok("started".into()),
        });
        assert_eq!(fx, vec![Effect::Run(Job::Refresh)]);
        assert!(ui.busy.is_none());
    }

    #[test]
    fn readiness_is_not_ready_on_errors_and_unverified_until_health_runs() {
        let mut ui = loaded_ui();
        assert_eq!(ui.readiness().1, "Not checked");
        ui.health.insert(
            "main".into(),
            SlotHealth {
                status: harness_runtime::health::Status::Pass,
                checks: vec![],
                at: "t".into(),
            },
        );
        assert_eq!(ui.readiness().1, "Ready");
        ui.snap.slots[0].model_state = FileState::Missing;
        assert_eq!(ui.readiness().1, "Not ready");
        ui.snap.slots.clear();
        assert_eq!(ui.readiness().1, "Not ready");
        ui.snap.config_error = Some("boom".into());
        assert!(ui.readiness().2.contains("boom"));
    }

    fn snapshot_with_events(ui: &Ui, extra: usize) -> JobMsg {
        let mut snap = ui.snap.clone();
        for n in 0..extra {
            snap.log.events.push(event(
                &format!("2026-01-01T00:01:{n:02}.000Z"),
                Some(Decision::Deny),
                Some("t3"),
            ));
        }
        snap.tasks = crate::tui::data::group_tasks(&snap.log);
        JobMsg::Snapshot(Box::new(snap))
    }

    #[test]
    fn a_refresh_keeps_the_selected_event_and_follows_new_ones_from_the_top() {
        let mut ui = loaded_ui();
        ui.view = View::Activity;
        ui.sel[View::Activity.index()] = 1; // the middle event
        let selected = ui.visible_events()[1];
        let before = ui.snap.log.events[selected].time.clone();
        ui.apply(snapshot_with_events(&ui, 2));
        let now = ui.visible_events()[ui.selected()];
        assert_eq!(
            ui.snap.log.events[now].time, before,
            "same event, new position"
        );

        ui.sel[View::Activity.index()] = 0;
        ui.apply(snapshot_with_events(&ui, 1));
        assert_eq!(ui.selected(), 0, "the top row keeps showing the newest");
    }

    #[test]
    fn a_refresh_keeps_the_selected_task_and_slot() {
        let mut ui = loaded_ui();
        ui.view = View::Tasks;
        ui.sel[View::Tasks.index()] = 1; // t1 (t2 is newer)
        ui.apply(snapshot_with_events(&ui, 1)); // t3 becomes the newest task
        assert_eq!(ui.visible_tasks()[ui.selected()].id, "t1");
    }

    #[test]
    fn p_toggles_auto_refresh() {
        let mut ui = loaded_ui();
        assert!(ui.auto);
        ui.handle_key(key(KeyCode::Char('p')));
        assert!(!ui.auto && ui.notice.is_some());
        ui.handle_key(key(KeyCode::Char('p')));
        assert!(ui.auto);
    }

    #[test]
    fn in_the_chat_input_letters_are_text_and_enter_sends() {
        let mut ui = loaded_ui();
        ui.view = View::Chat;
        for c in "q?r5p".chars() {
            assert!(ui.handle_key(key(KeyCode::Char(c))).is_empty());
        }
        assert_eq!(ui.chat.input, "q?r5p", "shortcut letters are text here");
        assert!(ui.view == View::Chat && ui.auto && ui.overlay.is_none());
        assert_eq!(ui.handle_key(key(KeyCode::Enter)), vec![Effect::ChatSend]);
        let turn = ui.take_chat_turn().expect("the turn is queued");
        assert_eq!((turn.slot.as_str(), turn.text.as_str()), ("main", "q?r5p"));
        assert!(ui.chat.running && ui.take_chat_turn().is_none());
    }

    #[test]
    fn ctrl_c_stops_a_turn_then_clears_text_then_quits() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut ui = loaded_ui();
        ui.view = View::Chat;
        ui.chat.running = true;
        assert!(ui.handle_key(ctrl_c).is_empty());
        assert!(ui.chat.cancel.load(std::sync::atomic::Ordering::Relaxed));
        ui.chat.running = false;
        ui.handle_key(key(KeyCode::Char('x')));
        assert!(ui.handle_key(ctrl_c).is_empty() && ui.chat.input.is_empty());
        assert_eq!(ui.handle_key(ctrl_c), vec![Effect::Quit]);
    }

    #[test]
    fn tab_leaves_the_chat_input_and_then_digits_navigate() {
        let mut ui = loaded_ui();
        ui.view = View::Chat;
        ui.handle_key(key(KeyCode::Tab));
        assert_eq!(ui.focus, Focus::Nav);
        ui.handle_key(key(KeyCode::Char('6')));
        assert_eq!(ui.view, View::Models);
        assert_eq!(
            ui.focus,
            Focus::Content,
            "a jump hands the focus to the screen"
        );
        // F1 opens help from the input too.
        ui.view = View::Chat;
        ui.focus = Focus::Content;
        ui.handle_key(key(KeyCode::F(1)));
        assert_eq!(ui.overlay, Some(Overlay::Help));
        ui.handle_key(key(KeyCode::F(1)));
        assert!(ui.overlay.is_none());
    }

    #[test]
    fn a_finished_turn_asks_for_a_refresh() {
        let mut ui = loaded_ui();
        ui.chat.running = true;
        let effects = ui.apply(JobMsg::Chat(ChatMsg::Finished {
            agent: None,
            end: crate::chat::TurnEnd::Done,
        }));
        assert_eq!(effects, vec![Effect::Run(Job::Refresh)]);
        assert!(!ui.chat.running);
    }

    #[test]
    fn filtering_by_a_task_includes_the_events_of_its_sessions() {
        let mut ui = loaded_ui();
        ui.snap.log.events.push(event(
            "2026-01-01T00:00:04.000Z",
            Some(Decision::Deny),
            None,
        ));
        let last = ui.snap.log.events.len() - 1;
        ui.snap.log.events[last].session_id = Some("ses_x".to_owned());
        ui.snap.tasks.push(TaskRow {
            id: "task-0007".to_owned(),
            is_task: true,
            sessions: vec!["ses_x".to_owned()],
            ..TaskRow::default()
        });
        ui.filter.group = Some("task-0007".to_owned());
        assert_eq!(ui.visible_events(), vec![last]);
        ui.filter.group = Some("t1".to_owned());
        assert_eq!(ui.visible_events(), vec![1, 0]);
    }

    #[test]
    fn next_steps_follow_what_is_missing() {
        let mut ui = loaded_ui();
        let steps = ui.next_steps().join(" | ");
        assert!(
            steps.contains("Start the `main` server") && steps.contains("press h"),
            "{steps}"
        );
        ui.snap.slots.clear();
        assert!(ui.next_steps().join(" ").contains("agent init --machine"));
        ui.snap.config_error = Some("bad".into());
        assert!(ui.next_steps().join(" ").contains("Diagnostics"));
        ui.snap.config_error = None;
        ui.snap.slots = vec![slot("main", true, ServerState::Running { pid: 1 })];
        ui.health.insert(
            "main".into(),
            SlotHealth {
                status: harness_runtime::health::Status::Pass,
                checks: vec![],
                at: "t".into(),
            },
        );
        assert!(ui.next_steps().join(" ").contains("Open Chat"));
    }

    #[test]
    fn help_overlay_opens_and_closes() {
        let mut ui = loaded_ui();
        ui.handle_key(key(KeyCode::Char('?')));
        assert_eq!(ui.overlay, Some(Overlay::Help));
        ui.handle_key(key(KeyCode::Esc));
        assert!(ui.overlay.is_none());
    }
}
