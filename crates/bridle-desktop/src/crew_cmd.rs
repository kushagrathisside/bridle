//! The commands of the team and the board: the roster, the crew rail's data, tagging, the issues, and a member's turn on an
//! issue. The rules are in `harness_cli::crew`, `harness_cli::board` and `harness_cli::board_tools`; this file finds the
//! current project, runs the member's turn through the same agent loop and policy as any other turn, and keeps the board
//! in step with how the turn ended.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use harness_cli::board::{BoardStore, Issue, NewIssue, Status};
use harness_cli::board_tools;
use harness_cli::chat::{Agent, TurnEnd};
use harness_cli::crew::{Roster, Target, TeamStore};
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
    workspace_of(&bridge.args()).map(|ws| BoardStore::open(&ws)).map_err(|e| format!("{e:#}"))
}

/// After the window opens a folder no turn is running, so any issue left in progress was cut short by a close or a crash.
pub fn recover_board(workspace: &std::path::Path) {
    let _ = BoardStore::open(workspace).recover();
}

fn crew_json(bridge: &Bridge) -> Value {
    let (roster, warning) = roster();
    let load = board(bridge).ok().and_then(|b| b.load().ok()).map(|b| b.load_by_member()).unwrap_or_default();
    let working: Vec<String> = board(bridge)
        .ok()
        .and_then(|b| b.load().ok())
        .map(|b| b.issues.values().filter(|i| i.status == Status::InProgress).filter_map(|i| i.assignee.clone()).collect())
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
pub fn crew_add_member(bridge: State<'_, Bridge>, role: String, name: Option<String>, speciality: String) -> Value {
    edit_roster(&bridge, |r| r.add_member(&role, name.as_deref(), &speciality).map(|_| ()))
}

#[tauri::command(async)]
pub fn crew_update_member(bridge: State<'_, Bridge>, id: String, name: Option<String>, speciality: Option<String>, enabled: Option<bool>) -> Value {
    edit_roster(&bridge, |r| r.update_member(&id, name.as_deref(), speciality.as_deref(), enabled))
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
    let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Request");
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
        let team = roster.member(&pick.member).and_then(|m| roster.role(&m.role)).map(|r| r.team.clone());
        let new = NewIssue { title: title_of(&text), body: text.trim().to_owned(), assignee: Some(pick.member.clone()), track: team, priority: Some(1), ..NewIssue::default() };
        match store.create("user", &new) {
            Ok((issue, _)) => {
                notes.push(format!("{} takes it ({})", roster.member(&pick.member).map_or(pick.member.clone(), |m| m.name.clone()), pick.reason));
                issues.push(json!({"id": issue.id, "assignee": pick.member, "reason": pick.reason}));
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
            "skipped": board.skipped,
        }),
        Err(error) => fail(error),
    }
}

/// Runs a board change for the person (the actor is `user`) and answers with the issue, or with the reason it was refused.
fn user_change(bridge: &Bridge, change: impl FnOnce(&BoardStore, &Roster) -> anyhow::Result<Issue>) -> Value {
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
pub fn board_new_issue(bridge: State<'_, Bridge>, title: String, body: Option<String>, assignee: Option<String>, parent: Option<String>, acceptance: Option<Vec<String>>) -> Value {
    user_change(&bridge, |store, roster| {
        let handle = assignee.as_deref().map(str::trim).filter(|a| !a.is_empty());
        let member = match handle {
            Some(handle) => {
                let target = roster.resolve(handle).ok_or_else(|| anyhow::anyhow!("`{handle}` is not a member, role or team"))?;
                let load = store.load()?.load_by_member();
                Some(roster.assign(&target, &format!("{title} {}", body.as_deref().unwrap_or("")), &load)?.member)
            }
            None => None,
        };
        let track = member.as_deref().and_then(|m| roster.member(m)).and_then(|m| roster.role(&m.role)).map(|r| r.team.clone());
        let new = NewIssue { title, body: body.unwrap_or_default(), assignee: member, track, parent, acceptance: acceptance.unwrap_or_default(), ..NewIssue::default() };
        store.create("user", &new).map(|(issue, _)| issue)
    })
}

#[tauri::command(async)]
pub fn board_set_status(bridge: State<'_, Bridge>, id: String, status: String, note: Option<String>) -> Value {
    user_change(&bridge, |store, _| {
        let to = Status::parse(&status).ok_or_else(|| anyhow::anyhow!("unknown status `{status}`"))?;
        store.set_status("user", &id, to, note.as_deref())
    })
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
    user_change(&bridge, |store, _| store.comment("user", &id, &text))
}

#[tauri::command(async)]
pub fn board_export(bridge: State<'_, Bridge>) -> Value {
    match board(&bridge).and_then(|s| s.export_markdown().map_err(|e| format!("{e:#}"))) {
        Ok(text) => json!({"ok": true, "markdown": text}),
        Err(error) => fail(error),
    }
}

/// What a member is told when an issue is given to them.
fn brief(issue: &Issue) -> String {
    let mut text = format!("You hold issue {}: {}\n", issue.id, issue.title);
    if !issue.body.is_empty() {
        text.push_str(&format!("\n{}\n", issue.body));
    }
    if !issue.acceptance.is_empty() {
        text.push_str("\nIt is done when:\n");
        for item in &issue.acceptance {
            text.push_str(&format!("- {item}\n"));
        }
    }
    text.push_str("\nDo the work, say what you did with board_comment, and move the issue on with board_update.");
    text
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
    let Some(issue) = current else { return fail(format!("there is no issue {id}")) };
    let Some(member) = issue.assignee.clone() else { return fail(format!("{id} has nobody assigned")) };
    if !roster.member(&member).is_some_and(|m| m.enabled) {
        return fail(format!("{member} is not an enabled member of the team"));
    }
    let Some(persona) = roster.persona(&member) else { return fail(format!("{member} has no role")) };
    let args = bridge.args();
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
                    .and_then(|app| Agent::for_member(app, format!("{persona}\n\n{}", board_tools::GUIDE), board_tools::member_tools(store.clone(), roster.clone(), &member, &format!("run-{id}"))));
                match built {
                    Ok(agent) => Box::new(agent),
                    Err(error) => return fail(error),
                }
            }
        };
        inner.running = true;
        inner.cancel = Arc::new(AtomicBool::new(false));
        inner.active = Some(Active { member: member.clone(), issue: id.clone() });
        (agent, Arc::clone(&inner.cancel))
    };
    // The harness takes the issue for the member; the turn starts only when that succeeds.
    let claimed = match store.claim(&id, &member, Some(&format!("run-{id}"))) {
        Ok(issue) => issue,
        Err(error) => {
            let mut inner = locked(&bridge.inner);
            inner.running = false;
            inner.active = None;
            inner.members.insert(member, agent);
            return fail(error);
        }
    };
    emit(&app, &json!({"type": "member_start", "member": member, "issue": claimed.id, "title": claimed.title}));
    jobs::spawn_chat(Turn { text: brief(&claimed), slot, agent: Some(agent), cancel }, args, bridge.tx.clone());
    json!({"ok": true, "issue": issue_json(&claimed)})
}

/// Brings the board in step with how a member's turn ended, when the member did not do it themselves.
pub fn finish_member_turn(bridge: &Bridge, active: &Active, end: &TurnEnd, answer: Option<&str>) {
    let Ok(store) = board(bridge) else { return };
    let (roster, _) = roster();
    let Ok(current) = store.load() else { return };
    let Some(issue) = current.issues.get(&active.issue) else { return };
    let report = answer.map(|a| a.chars().take(1_200).collect::<String>()).filter(|a| !a.trim().is_empty());
    if let (Some(report), true) = (&report, matches!(end, TurnEnd::Done)) {
        let _ = store.comment(&active.member, &active.issue, report);
    }
    if issue.status != Status::InProgress {
        return; // the member moved it on with the board tools
    }
    let review = roster.member(&active.member).and_then(|m| roster.role(&m.role)).is_some_and(|r| r.review_required);
    let open_children = current.children(&active.issue).iter().any(|c| c.status.is_open());
    let (to, note): (Status, Option<String>) = match end {
        TurnEnd::Done if open_children => return, // it is waiting for the issues it created
        TurnEnd::Done if review => (Status::InReview, None),
        TurnEnd::Done => (Status::Done, None),
        TurnEnd::Stopped => (Status::Ready, Some("Stopped by the user.".to_owned())),
        TurnEnd::StepLimit => (Status::Blocked, Some("The turn used all its model calls. Continue it from the board.".to_owned())),
        TurnEnd::Failed(reason) => (Status::Blocked, Some(format!("The turn failed: {reason}"))),
    };
    let _ = store.set_status("bridle", &active.issue, to, note.as_deref());
}
