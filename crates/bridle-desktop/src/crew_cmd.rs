//! The commands of the team and the board: the roster, the crew rail's data, tagging, the issues, and a member's turn on an
//! issue. The rules are in `harness_cli::crew`, `harness_cli::board` and `harness_cli::board_tools`; this file finds the
//! current project, runs the member's turn through the same agent loop and policy as any other turn, and keeps the board
//! in step with how the turn ended.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use harness_cli::board::{BoardStore, Issue, NewIssue, Status};
use harness_cli::board_tools::{self, NOT_PLANNED, brief};
use harness_cli::chat::{Agent, TurnEnd};
use harness_cli::crew::{Roster, Target, TeamStore};
use harness_cli::gitlayer::{self, RunGuard};
use harness_cli::tui::chat::Turn;
use harness_cli::tui::jobs;
use serde_json::{Value, json};
use tauri::{AppHandle, State};

use crate::bridge::{Active, Bridge, emit, locked, workspace_of};

fn fail(error: impl std::fmt::Display) -> Value {
    json!({"ok": false, "error": format!("{error:#}")})
}

fn roster() -> (Roster, Option<String>) {
    TeamStore::machine().map_or_else(|| (Roster::default(), None), |store| store.load())
}

fn board(bridge: &Bridge) -> Result<BoardStore, String> {
    workspace_of(&bridge.args())
        .map(|ws| BoardStore::open(&ws))
        .map_err(|e| format!("{e:#}"))
}

/// After the window opens a folder no turn is running, so any issue left in progress was cut short by a close or a crash.
pub fn recover_board(workspace: &std::path::Path) {
    // A run whose journal has no end was cut short: its files are saved first, then its issue goes back to ready.
    let cut_short = gitlayer::recover(workspace);
    let store = BoardStore::open(workspace);
    let _ = store.recover();
    for id in cut_short {
        let _ = store.comment("bridle", &id, "The files of the interrupted run were saved. Use Undo changes on the board to put them back as they were.");
    }
}

fn crew_json(bridge: &Bridge) -> Value {
    let (roster, warning) = roster();
    let load = board(bridge)
        .ok()
        .and_then(|b| b.load().ok())
        .map(|b| b.load_by_member())
        .unwrap_or_default();
    let working: Vec<String> = board(bridge)
        .ok()
        .and_then(|b| b.load().ok())
        .map(|b| {
            b.issues
                .values()
                .filter(|i| i.status == Status::InProgress)
                .filter_map(|i| i.assignee.clone())
                .collect()
        })
        .unwrap_or_default();
    json!({"ok": true, "roster": roster, "warning": warning, "load": load, "working": working})
}

#[tauri::command(async)]
pub fn get_crew(bridge: State<'_, Bridge>) -> Value {
    crew_json(&bridge)
}

/// Loads the roster, applies `change`, saves it, and answers with the new crew.
fn edit_roster(bridge: &Bridge, change: impl FnOnce(&mut Roster) -> anyhow::Result<()>) -> Value {
    let Some(store) = TeamStore::machine() else {
        return fail("this system has no folder for the machine configuration");
    };
    let (mut roster, _) = store.load();
    if let Err(error) = change(&mut roster).and_then(|()| store.save(&roster)) {
        return fail(error);
    }
    crew_json(bridge)
}

#[tauri::command(async)]
pub fn crew_add_member(
    bridge: State<'_, Bridge>,
    role: String,
    name: Option<String>,
    speciality: String,
) -> Value {
    edit_roster(&bridge, |r| {
        r.add_member(&role, name.as_deref(), &speciality)
            .map(|_| ())
    })
}

#[tauri::command(async)]
pub fn crew_update_member(
    bridge: State<'_, Bridge>,
    id: String,
    name: Option<String>,
    speciality: Option<String>,
    enabled: Option<bool>,
) -> Value {
    edit_roster(&bridge, |r| {
        r.update_member(&id, name.as_deref(), speciality.as_deref(), enabled)
    })
}

#[tauri::command(async)]
pub fn crew_remove_member(bridge: State<'_, Bridge>, id: String) -> Value {
    edit_roster(&bridge, |r| r.remove_member(&id))
}

#[tauri::command(async)]
pub fn crew_set_headcount(bridge: State<'_, Bridge>, role: String, count: usize) -> Value {
    edit_roster(&bridge, |r| r.set_headcount(&role, count))
}

#[tauri::command(async)]
pub fn crew_reset(bridge: State<'_, Bridge>) -> Value {
    edit_roster(&bridge, |r| {
        *r = Roster::default();
        Ok(())
    })
}

/// The people a message tags, in order, for the window to show.
#[tauri::command(async)]
pub fn resolve_tags(text: String) -> Value {
    let (roster, _) = roster();
    let tags: Vec<Value> = roster
        .mentions(&text)
        .iter()
        .map(|t| match t {
            Target::Member(id) => json!({"kind": "member", "id": id, "name": roster.member(id).map(|m| m.name.clone())}),
            Target::Role(id) => json!({"kind": "role", "id": id, "name": roster.role(id).map(|r| r.title.clone())}),
            Target::Team(id) => json!({"kind": "team", "id": id, "name": roster.team(id).map(|t| t.name.clone())}),
        })
        .collect();
    json!({"ok": true, "tags": tags})
}

/// The first line of a request, cut to a title.
fn title_of(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Request");
    let mut title: String = first.chars().take(90).collect();
    if first.chars().count() > 90 {
        title.push('…');
    }
    title
}

/// Makes one issue for each person or team the message tags, assigned by the rules, so the board runner works them in order.
#[tauri::command(async)]
pub fn create_tagged_issues(bridge: State<'_, Bridge>, text: String) -> Value {
    let (roster, _) = roster();
    let store = match board(&bridge) {
        Ok(store) => store,
        Err(error) => return fail(error),
    };
    let tags = roster.mentions(&text);
    if tags.is_empty() {
        return json!({"ok": true, "issues": [], "notes": []});
    }
    let mut issues: Vec<Value> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for target in tags {
        let load = store.load().map(|b| b.load_by_member()).unwrap_or_default();
        let pick = match roster.assign(&target, &text, &load) {
            Ok(pick) => pick,
            Err(error) => {
                notes.push(format!("{error:#}"));
                continue;
            }
        };
        let team = roster
            .member(&pick.member)
            .and_then(|m| roster.role(&m.role))
            .map(|r| r.team.clone());
        let new = NewIssue {
            title: title_of(&text),
            body: text.trim().to_owned(),
            assignee: Some(pick.member.clone()),
            track: team,
            priority: Some(1),
            ..NewIssue::default()
        };
        match store.create("user", &new) {
            Ok((issue, _)) => {
                notes.push(format!(
                    "{} takes it ({})",
                    roster
                        .member(&pick.member)
                        .map_or(pick.member.clone(), |m| m.name.clone()),
                    pick.reason
                ));
                issues
                    .push(json!({"id": issue.id, "assignee": pick.member, "reason": pick.reason}));
            }
            Err(error) => notes.push(format!("{error:#}")),
        }
    }
    json!({"ok": true, "issues": issues, "notes": notes})
}

fn issue_json(issue: &Issue) -> Value {
    serde_json::to_value(issue).unwrap_or(Value::Null)
}

#[tauri::command(async)]
pub fn get_board(bridge: State<'_, Bridge>) -> Value {
    let store = match board(&bridge) {
        Ok(store) => store,
        Err(error) => return fail(error),
    };
    match store.load() {
        Ok(board) => json!({
            "ok": true,
            "issues": board.issues.values().map(issue_json).collect::<Vec<_>>(),
            "load": board.load_by_member(),
            "ready": board.ready(None).iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
            "undoable": workspace_of(&bridge.args()).map(|ws| gitlayer::undoable_scopes(&ws).into_iter().collect::<Vec<_>>()).unwrap_or_default(),
            "skipped": board.skipped,
        }),
        Err(error) => fail(error),
    }
}

/// Runs a board change for the person (the actor is `user`) and answers with the issue, or with the reason it was refused.
/// Tells the member whose turn is running on `issue` that the person changed it: a notice follows their next tool result (ADR-0023).
/// Only one turn runs at a time, so the next tool call in the project's workspace is that member's.
fn tell_active_member(bridge: &Bridge, issue: &str, what: &str) {
    let active_issue = locked(&bridge.inner)
        .active
        .as_ref()
        .map(|a| a.issue.clone());
    if let Ok(workspace) = workspace_of(&bridge.args()) {
        board_tools::notify_active(&workspace, active_issue.as_deref(), issue, what);
    }
}

fn user_change(
    bridge: &Bridge,
    change: impl FnOnce(&BoardStore, &Roster) -> anyhow::Result<Issue>,
) -> Value {
    let store = match board(bridge) {
        Ok(store) => store,
        Err(error) => return fail(error),
    };
    let (roster, _) = roster();
    match change(&store, &roster) {
        Ok(issue) => json!({"ok": true, "issue": issue_json(&issue)}),
        Err(error) => fail(error),
    }
}

#[tauri::command(async)]
pub fn board_new_issue(
    bridge: State<'_, Bridge>,
    title: String,
    body: Option<String>,
    assignee: Option<String>,
    parent: Option<String>,
    acceptance: Option<Vec<String>>,
) -> Value {
    user_change(&bridge, |store, roster| {
        let handle = assignee.as_deref().map(str::trim).filter(|a| !a.is_empty());
        let member = match handle {
            Some(handle) => {
                let target = roster
                    .resolve(handle)
                    .ok_or_else(|| anyhow::anyhow!("`{handle}` is not a member, role or team"))?;
                let load = store.load()?.load_by_member();
                Some(
                    roster
                        .assign(
                            &target,
                            &format!("{title} {}", body.as_deref().unwrap_or("")),
                            &load,
                        )?
                        .member,
                )
            }
            None => None,
        };
        let track = member
            .as_deref()
            .and_then(|m| roster.member(m))
            .and_then(|m| roster.role(&m.role))
            .map(|r| r.team.clone());
        let new = NewIssue {
            title,
            body: body.unwrap_or_default(),
            assignee: member,
            track,
            parent,
            acceptance: acceptance.unwrap_or_default(),
            ..NewIssue::default()
        };
        store.create("user", &new).map(|(issue, _)| issue)
    })
}

#[tauri::command(async)]
pub fn board_set_status(
    bridge: State<'_, Bridge>,
    id: String,
    status: String,
    note: Option<String>,
) -> Value {
    let result = user_change(&bridge, |store, _| {
        let to =
            Status::parse(&status).ok_or_else(|| anyhow::anyhow!("unknown status `{status}`"))?;
        store.set_status("user", &id, to, note.as_deref())
    });
    if result["ok"] == json!(true) {
        let why = note.as_deref().filter(|n| !n.trim().is_empty());
        tell_active_member(
            &bridge,
            &id,
            &format!(
                "The person set issue {id} to {status}{}.",
                why.map_or(String::new(), |n| format!(": {n}"))
            ),
        );
    }
    result
}

#[tauri::command(async)]
pub fn board_assign(bridge: State<'_, Bridge>, id: String, assignee: Option<String>) -> Value {
    user_change(&bridge, |store, roster| {
        let member = match assignee.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            Some(handle) => match roster.resolve(handle) {
                Some(Target::Member(m)) => Some(m),
                Some(target) => {
                    let load = store.load()?.load_by_member();
                    Some(roster.assign(&target, &id, &load)?.member)
                }
                None => anyhow::bail!("`{handle}` is not a member, role or team"),
            },
            None => None,
        };
        store.assign("user", &id, member.as_deref())
    })
}

#[tauri::command(async)]
pub fn board_comment(bridge: State<'_, Bridge>, id: String, text: String) -> Value {
    let result = user_change(&bridge, |store, _| store.comment("user", &id, &text));
    if result["ok"] == json!(true) {
        tell_active_member(
            &bridge,
            &id,
            &format!("The person commented on issue {id}: {text}"),
        );
    }
    result
}

#[tauri::command(async)]
pub fn board_export(bridge: State<'_, Bridge>) -> Value {
    match board(&bridge).and_then(|s| s.export_markdown().map_err(|e| format!("{e:#}"))) {
        Ok(text) => json!({"ok": true, "markdown": text}),
        Err(error) => fail(error),
    }
}

/// Starts a member's turn on a ready issue: the issue is taken for them, and the turn runs under their prompt with the board
/// tools, through the same agent loop and policy as any other turn.
#[tauri::command(async)]
pub fn run_issue(app: AppHandle, bridge: State<'_, Bridge>, id: String, slot: String) -> Value {
    let store = match board(&bridge) {
        Ok(store) => store,
        Err(error) => return fail(error),
    };
    let (roster, _) = roster();
    let current = match store.load() {
        Ok(board) => board.issues.get(&id).cloned(),
        Err(error) => return fail(error),
    };
    let Some(issue) = current else {
        return fail(format!("there is no issue {id}"));
    };
    let Some(member) = issue.assignee.clone() else {
        return fail(format!("{id} has nobody assigned"));
    };
    if !roster.member(&member).is_some_and(|m| m.enabled) {
        return fail(format!("{member} is not an enabled member of the team"));
    }
    let Some(persona) = roster.persona(&member) else {
        return fail(format!("{member} has no role"));
    };
    let args = bridge.args();
    if locked(&bridge.inner).running {
        return fail("a turn is already running; stop it or wait for it to finish");
    }
    // The project's lock and a checkpoint of the files, before anything changes. Refuses in the middle of a merge or rebase.
    let guard = match workspace_of(&args)
        .map_err(|e| format!("{e:#}"))
        .and_then(|ws| RunGuard::begin(&ws, &id))
    {
        Ok(guard) => guard,
        Err(error) => return fail(error),
    };
    let (agent, cancel) = {
        let mut inner = locked(&bridge.inner);
        if inner.running {
            return fail("a turn is already running; stop it or wait for it to finish");
        }
        let existing = inner.members.remove(&member);
        let agent = match existing {
            Some(agent) => agent,
            None => {
                let built = args
                    .load_app()
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|app| {
                        Agent::for_member(
                            app,
                            format!("{persona}\n\n{}", board_tools::GUIDE),
                            board_tools::member_tools(
                                store.clone(),
                                roster.clone(),
                                &member,
                                &format!("run-{id}"),
                            ),
                        )
                    });
                match built {
                    Ok(agent) => Box::new(agent),
                    Err(error) => return fail(error),
                }
            }
        };
        inner.running = true;
        inner.cancel = Arc::new(AtomicBool::new(false));
        inner.active = Some(Active {
            member: member.clone(),
            issue: id.clone(),
        });
        inner.guard = Some(guard);
        (agent, Arc::clone(&inner.cancel))
    };
    // The harness takes the issue for the member; the turn starts only when that succeeds.
    let claimed = match store.claim(&id, &member, Some(&format!("run-{id}"))) {
        Ok(issue) => issue,
        Err(error) => {
            let mut inner = locked(&bridge.inner);
            inner.running = false;
            inner.active = None;
            inner.guard = None;
            inner.members.insert(member, agent);
            return fail(error);
        }
    };
    emit(
        &app,
        &json!({"type": "member_start", "member": member, "issue": claimed.id, "title": claimed.title}),
    );
    jobs::spawn_chat(
        Turn {
            text: brief(&claimed),
            slot,
            agent: Some(agent),
            cancel,
        },
        args,
        bridge.tx.clone(),
    );
    json!({"ok": true, "issue": issue_json(&claimed)})
}

/// Brings the board in step with how a member's turn ended, when the member did not do it themselves.
pub fn finish_member_turn(
    bridge: &Bridge,
    active: &Active,
    end: &TurnEnd,
    answer: Option<&str>,
    guard: Option<RunGuard>,
) {
    settle_issue(bridge, active, end, answer);
    // The checkpoint after the run, and a note on the issue when the files changed.
    let Some(guard) = guard else { return };
    let outcome = match end {
        TurnEnd::Done => "done",
        TurnEnd::Stopped => "stopped",
        TurnEnd::StepLimit => "step_limit",
        TurnEnd::Failed(_) => "failed",
    };
    let finished = guard.finish(outcome);
    let (Ok(store), Ok(ws)) = (board(bridge), workspace_of(&bridge.args())) else {
        return;
    };
    if let Some(note) = finished.note {
        let _ = store.comment("bridle", &active.issue, &note);
    } else if let (Some(before), Some(after)) = (&finished.before, &finished.after)
        && let Ok(files) = gitlayer::changed_paths(&ws, &before.reference, &after.reference)
        && !files.is_empty()
    {
        let shown: Vec<&str> = files.iter().take(8).map(String::as_str).collect();
        let more = if files.len() > 8 {
            format!(" and {} more", files.len() - 8)
        } else {
            String::new()
        };
        let _ = store.comment(
            "bridle",
            &active.issue,
            &format!(
                "The run changed {} file(s): {}{more}. Undo changes on the board puts them back.",
                files.len(),
                shown.join(", ")
            ),
        );
    }
}

/// Brings the board in step with how a member's turn ended, when the member did not do it themselves.
fn settle_issue(bridge: &Bridge, active: &Active, end: &TurnEnd, answer: Option<&str>) {
    let Ok(store) = board(bridge) else { return };
    let (roster, _) = roster();
    let Ok(current) = store.load() else { return };
    let Some(issue) = current.issues.get(&active.issue) else {
        return;
    };
    let report = answer
        .map(|a| a.chars().take(1_200).collect::<String>())
        .filter(|a| !a.trim().is_empty());
    if let (Some(report), true) = (&report, matches!(end, TurnEnd::Done)) {
        let _ = store.comment(&active.member, &active.issue, report);
    }
    if issue.status != Status::InProgress {
        return; // the member moved it on with the board tools
    }
    let review = roster
        .member(&active.member)
        .and_then(|m| roster.role(&m.role))
        .is_some_and(|r| r.review_required);
    let open_children = current
        .children(&active.issue)
        .iter()
        .any(|c| c.status.is_open());
    // A project manager who ends a turn without having split the work did not do the job. The issue is given back to her, up to
    // twice, with a reminder in her brief; after that it is blocked so a person looks at it.
    let planner = roster
        .member(&active.member)
        .and_then(|m| roster.role(&m.role))
        .is_some_and(|r| r.team == "pm");
    let no_children = current.children(&active.issue).is_empty();
    let tries = issue
        .comments
        .iter()
        .filter(|c| c.text.starts_with(NOT_PLANNED))
        .count();
    let (to, note): (Status, Option<String>) = match end {
        TurnEnd::Done if planner && no_children && tries < 2 => {
            (Status::Ready, Some(format!("{NOT_PLANNED} Trying again.")))
        }
        TurnEnd::Done if planner && no_children => (
            Status::Blocked,
            Some(format!(
                "{NOT_PLANNED} It was tried three times. Split the work by hand or ask again."
            )),
        ),
        TurnEnd::Done if open_children => return, // it is waiting for the issues it created
        TurnEnd::Done if review => (Status::InReview, None),
        TurnEnd::Done => (Status::Done, None),
        TurnEnd::Stopped => (Status::Ready, Some("Stopped by the user.".to_owned())),
        TurnEnd::StepLimit => (
            Status::Blocked,
            Some("The turn used all its model calls. Continue it from the board.".to_owned()),
        ),
        TurnEnd::Failed(reason) => (Status::Blocked, Some(format!("The turn failed: {reason}"))),
    };
    let _ = store.set_status("bridle", &active.issue, to, note.as_deref());
}

/// Puts back what the latest run on an issue changed, from the checkpoints taken before and after it. Files the person changed
/// since are left alone and listed.
#[tauri::command(async)]
pub fn undo_issue(bridge: State<'_, Bridge>, id: String) -> Value {
    if locked(&bridge.inner).running {
        return fail("a turn is running; stop it or wait for it to finish before undoing");
    }
    let ws = match workspace_of(&bridge.args()) {
        Ok(ws) => ws,
        Err(error) => return fail(error),
    };
    let _lock = match gitlayer::RepoLock::acquire(&ws) {
        Ok(lock) => lock,
        Err(error) => return fail(error),
    };
    let Some((before, after)) = gitlayer::latest_pair(&ws, &id) else {
        return fail(format!("{id} has no saved run to undo"));
    };
    match gitlayer::undo(&ws, &before, &after) {
        Ok(report) => {
            let mut text = format!(
                "Undid the latest run: put back {} file(s), removed {} file(s) it created.",
                report.restored.len(),
                report.removed.len()
            );
            if !report.skipped.is_empty() {
                let names: Vec<String> = report
                    .skipped
                    .iter()
                    .map(|(p, why)| format!("{p} ({why})"))
                    .collect();
                text.push_str(&format!(" Left alone: {}.", names.join(", ")));
            }
            if let Ok(store) = board(&bridge) {
                let _ = store.comment("user", &id, &text);
            }
            json!({"ok": true, "restored": report.restored, "removed": report.removed, "skipped": report.skipped.iter().map(|(p, w)| json!({"path": p, "why": w})).collect::<Vec<_>>()})
        }
        Err(error) => fail(error),
    }
}
