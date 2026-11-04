//! The JSON the window receives. Every value comes from the same collectors the terminal UI uses
//! (`Snapshot::collect`, the chat `Event`s), already redacted and stripped of control characters.

use harness_cli::chat::{Event, TurnEnd};
use harness_cli::commands::models::ServerState;
use harness_cli::tui::data::{FileState, Snapshot, TaskRow};
use harness_cli::tui::diag::DiagItem;
use harness_cli::tui::events::EventView;
use harness_runtime::health::{CheckResult, Status};
use serde_json::{Value, json};

/// How many of the newest log events the window shows.
const EVENT_LIMIT: usize = 300;

pub fn snapshot(snap: &Snapshot) -> Value {
    json!({
        "workspace": snap.workspace.as_ref().map(|p| p.display().to_string()),
        "config_error": snap.config_error,
        // Each pair is (file path, layer label), in that order.
        "config_files": snap.config_files.iter().map(|(path, origin)| json!({"origin": origin, "path": path})).collect::<Vec<_>>(),
        "machine_config": snap.machine_config.as_ref().map(|(path, exists)| json!({"path": path, "exists": exists})),
        "issues": snap.issues.iter().map(|i| json!({
            "error": i.error, "rule": i.rule, "path": i.path, "message": i.message,
        })).collect::<Vec<_>>(),
        "slots": snap.slots.iter().map(slot).collect::<Vec<_>>(),
        "default_slot": snap.default_slot,
        "policy": snap.policy.as_ref().map(|p| json!({
            "rows": p.rows.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
            "allowed_paths": p.allowed_paths,
        })),
        "network": snap.network.iter().map(|n| json!({
            "name": n.name, "target": n.target, "scope": n.scope, "note": n.note, "sev": n.sev.word(),
        })).collect::<Vec<_>>(),
        "tasks": snap.tasks.iter().map(task).collect::<Vec<_>>(),
        "events": snap.log.events.iter().rev().take(EVENT_LIMIT).map(event_row).collect::<Vec<_>>(),
        "collected_at": snap.collected_at,
    })
}

fn slot(row: &harness_cli::tui::data::SlotRow) -> Value {
    let (server, pid) = match &row.server {
        ServerState::NotManaged => ("stopped", None),
        ServerState::Running { pid } => ("running", Some(*pid)),
        ServerState::Stale { pid } => ("stale", Some(*pid)),
        ServerState::Reused { pid, .. } => ("reused", Some(*pid)),
        ServerState::Unreadable(_) => ("unreadable", None),
    };
    json!({
        "name": row.name,
        "model_id": row.model_id,
        "role": row.role,
        "provider": row.provider,
        "hardware": row.hardware,
        "endpoint": row.endpoint,
        "runtime": row.runtime,
        "runtime_state": file_state(&row.runtime_state),
        "model_file": row.model_file,
        "model_state": file_state(&row.model_state),
        "mmproj_file": row.mmproj_file,
        "mmproj_state": file_state(&row.mmproj_state),
        "vision": row.vision,
        "server": server,
        "pid": pid,
        "launchable": row.launchable,
        "log_path": row.log_path,
        "ready_to_start": row.launchable
            && !matches!(row.model_state, FileState::Missing | FileState::Unresolved)
            && !matches!(row.runtime_state, FileState::Missing | FileState::Unresolved)
            && !matches!(row.mmproj_state, FileState::Missing | FileState::Unresolved),
    })
}

fn file_state(state: &FileState) -> &'static str {
    match state {
        FileState::NotManaged => "not managed",
        FileState::Unresolved => "unresolved",
        FileState::Present(_) => "present",
        FileState::Missing => "missing",
    }
}

fn task(row: &TaskRow) -> Value {
    json!({
        "id": row.id,
        "is_task": row.is_task,
        "objective": row.objective,
        "status": row.status,
        "phase": row.phase,
        "iteration": row.iteration,
        "last_action": row.last_action,
        "last_test": row.last_test,
        "failures": row.failures,
        "refusals": row.refusals,
        "worktree": row.worktree,
        "first": row.first,
        "last": row.last,
        "events": row.events,
        "allowed": row.allowed,
        "blocked": row.blocked,
        "failed": row.failed,
    })
}

fn event_row(event: &EventView) -> Value {
    json!({
        "time": event.time,
        "kind": event.kind,
        "task": event.task_id,
        "session": event.session_id,
        "tool": event.tool,
        "class": event.tool_class,
        "decision": event.decision.map(|d| d.label()),
        "status": event.status,
        "rule": event.rule,
        "summary": event.summary(),
        "blocked": event.is_blocked(),
        "failure": event.is_failure(),
        "duration_ms": event.duration_ms,
    })
}

/// One chat event, as the window draws it.
pub fn chat_event(event: &Event) -> Value {
    match event {
        Event::Delta(text) => json!({"type": "delta", "text": text}),
        Event::ToolStart { id, name, summary } => {
            json!({"type": "tool_start", "id": id, "name": name, "summary": summary})
        }
        Event::ToolBlocked {
            id,
            name,
            summary,
            rule,
            message,
        } => json!({
            "type": "tool_blocked", "id": id, "name": name, "summary": summary,
            "rule": rule, "message": message,
        }),
        Event::ToolDone {
            id,
            name,
            ok,
            preview,
        } => json!({"type": "tool_done", "id": id, "name": name, "ok": ok, "preview": preview}),
        Event::Notice(text) => json!({"type": "notice", "text": text}),
        Event::Working(text) => json!({"type": "working", "text": text}),
        Event::Usage { used, window } => json!({"type": "usage", "used": used, "window": window}),
    }
}

/// How a turn ended, as the window draws it.
pub fn turn_end(end: &TurnEnd) -> Value {
    let (kind, message) = match end {
        TurnEnd::Done => ("done", String::new()),
        TurnEnd::Stopped => ("stopped", String::new()),
        TurnEnd::StepLimit => (
            "step_limit",
            format!(
                "The turn used its {} model calls and stopped. Send a message to continue.",
                harness_cli::chat::MAX_STEPS
            ),
        ),
        TurnEnd::Failed(reason) => ("failed", reason.clone()),
    };
    json!({"type": "turn_end", "end": kind, "message": message})
}

pub fn doctor(items: &[DiagItem], blocking: bool, at: &str) -> Value {
    json!({
        "type": "doctor",
        "blocking": blocking,
        "at": at,
        "items": items.iter().map(|i| json!({
            "sev": i.sev.word(), "category": i.category, "title": i.title,
            "detail": i.detail, "impact": i.impact, "next": i.next,
        })).collect::<Vec<_>>(),
    })
}

pub fn health(slot: &str, status: Status, checks: &[CheckResult], at: &str) -> Value {
    json!({
        "type": "health",
        "slot": slot,
        "status": status_word(status),
        "at": at,
        "checks": checks.iter().map(|c| json!({
            "kind": format!("{:?}", c.kind),
            "status": status_word(c.status),
            "detail": c.detail,
            "latency_ms": c.latency_ms,
        })).collect::<Vec<_>>(),
    })
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Pass => "pass",
        Status::Skip => "skip",
        Status::Warn => "warn",
        Status::Fail => "fail",
    }
}
