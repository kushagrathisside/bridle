//! The board: the issues the team's members work from. A project manager takes an issue and splits it into issues for
//! other teams; each member works the issues assigned to them; the person can see and change all of it.
//!
//! The board is an **append-only log** in the project's state folder (`board/events.jsonl`). Every line is the complete
//! state of one issue after one change, with who made the change, when, and a checksum. The board is the newest line of
//! each issue, so the log is also the audit trail. Every change takes an exclusive file lock, so members and two windows
//! cannot take the same issue or lose a change. A torn last line (a crash in the middle of a write) is dropped, and a line
//! whose checksum is wrong is skipped.
//!
//! This module has the rules: the states an issue may move between, the dependencies and the ready queue, the one-claim
//! rule, the limits, and recovery after a crash. Who may do what is decided by the tools that members call, not here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use harness_core::identity;
use serde::{Deserialize, Serialize};

/// The most issues the board holds.
pub const MAX_ISSUES: usize = 500;
/// The most children one issue may have.
pub const MAX_CHILDREN: usize = 12;
/// The deepest nesting: an issue, its children, their children.
pub const MAX_DEPTH: usize = 3;
const MAX_TITLE: usize = 200;
const MAX_BODY: usize = 4_000;
const MAX_COMMENT: usize = 2_000;
const MAX_ACCEPTANCE: usize = 10;
const MAX_COMMENTS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Backlog,
    Ready,
    InProgress,
    InReview,
    Blocked,
    Done,
    Cancelled,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Backlog => "backlog",
            Status::Ready => "ready",
            Status::InProgress => "in_progress",
            Status::InReview => "in_review",
            Status::Blocked => "blocked",
            Status::Done => "done",
            Status::Cancelled => "cancelled",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "backlog" => Status::Backlog,
            "ready" => Status::Ready,
            "in_progress" => Status::InProgress,
            "in_review" => Status::InReview,
            "blocked" => Status::Blocked,
            "done" => Status::Done,
            "cancelled" => Status::Cancelled,
            _ => return None,
        })
    }

    /// Whether the issue still needs work from someone.
    pub fn is_open(self) -> bool {
        !matches!(self, Status::Done | Status::Cancelled)
    }

    /// Whether `self` may move to `to` (a move to the same state is refused).
    fn may_move_to(self, to: Status) -> bool {
        use Status::{Backlog, Blocked, Cancelled, Done, InProgress, InReview, Ready};
        matches!(
            (self, to),
            (Backlog, Ready | Cancelled)
                | (Ready, InProgress | Backlog | Blocked | Cancelled)
                | (InProgress, InReview | Done | Ready | Blocked | Cancelled)
                | (InReview, Done | Ready | InProgress | Cancelled)
                | (Blocked, Ready | Cancelled)
                | (Done, Ready)
                | (Cancelled, Backlog)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Task,
    Bug,
    Feature,
    Research,
    Review,
}

impl Kind {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "task" => Kind::Task,
            "bug" => Kind::Bug,
            "feature" => Kind::Feature,
            "research" => Kind::Research,
            "review" => Kind::Review,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub author: String,
    pub time: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// A short hash such as `B-a3f8`; a child is `B-a3f8.1`.
    pub id: String,
    pub title: String,
    pub body: String,
    pub kind: Kind,
    pub status: Status,
    /// The member who works it, or nobody.
    pub assignee: Option<String>,
    /// The team lane it is shown in.
    pub track: Option<String>,
    pub parent: Option<String>,
    /// Issues that must be done first.
    pub blocked_by: Vec<String>,
    /// What must be true for it to be done.
    pub acceptance: Vec<String>,
    /// 0 is the most urgent, 3 the least.
    pub priority: u8,
    pub creator: String,
    pub created: String,
    pub updated: String,
    pub comments: Vec<Comment>,
    /// A reason it was put on hold by hand; an issue on hold is not released when its blockers finish.
    pub hold: Option<String>,
    /// The run (the turn of a member) that works it now.
    pub run: Option<String>,
    /// Makes a repeated creation return the issue that already exists.
    pub key: Option<String>,
}

/// What a creator supplies.
#[derive(Debug, Clone, Default)]
pub struct NewIssue {
    pub title: String,
    pub body: String,
    pub kind: Option<Kind>,
    pub assignee: Option<String>,
    pub track: Option<String>,
    pub parent: Option<String>,
    pub blocked_by: Vec<String>,
    pub acceptance: Vec<String>,
    pub priority: Option<u8>,
    pub key: Option<String>,
}

/// One line of the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    seq: u64,
    time: String,
    actor: String,
    action: String,
    issue: Issue,
    sum: String,
}

/// The board as it is now, and how many log lines could not be used.
#[derive(Debug, Clone, Default)]
pub struct Board {
    pub issues: BTreeMap<String, Issue>,
    pub seq: u64,
    pub skipped: usize,
}

#[derive(Debug, Clone)]
pub struct BoardStore {
    dir: PathBuf,
}

fn now() -> String {
    humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string()
}

/// FNV-1a over the bytes: a checksum to notice a damaged line, not a defence against anyone.
fn checksum(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn entry_sum(seq: u64, time: &str, actor: &str, action: &str, issue: &Issue) -> String {
    checksum(&format!("{seq}|{time}|{actor}|{action}|{}", serde_json::to_string(issue).unwrap_or_default()))
}

impl BoardStore {
    pub fn open(workspace: &Path) -> Self {
        Self { dir: identity::state_dir(workspace).join("board") }
    }

    fn log_path(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    /// The board as it is now.
    pub fn load(&self) -> Result<Board> {
        self.locked(|board, _| Ok(board.clone()))
    }

    /// Takes the lock, reads the board, runs `change`, and appends the entries it returns.
    fn locked<T>(&self, change: impl FnOnce(&mut Board, &mut Vec<(String, Issue)>) -> Result<T>) -> Result<T> {
        fs::create_dir_all(&self.dir)?;
        let lock = OpenOptions::new().create(true).truncate(false).write(true).open(self.dir.join("board.lock"))?;
        lock.lock()?;
        let path = self.log_path();
        trim_torn_tail(&path)?;
        let mut board = read_board(&path)?;
        let mut out: Vec<(String, Issue)> = Vec::new();
        let result = change(&mut board, &mut out)?;
        if !out.is_empty() {
            let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
            let mut seq = board.seq;
            let mut text = String::new();
            let time = now();
            for (action, issue) in out {
                seq += 1;
                let (actor, action) = action.split_once('\t').map_or(("?", action.as_str()), |(a, b)| (a, b));
                let sum = entry_sum(seq, &time, actor, action, &issue);
                let entry = Entry { seq, time: time.clone(), actor: actor.to_owned(), action: action.to_owned(), issue, sum };
                text.push_str(&serde_json::to_string(&entry)?);
                text.push('\n');
            }
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        drop(lock);
        Ok(result)
    }

    /// Creates an issue. A repeated call with the same `key` and parent returns the issue that exists (the second value is
    /// `false`). An issue with an assignee and no open blocker starts `ready`; one with an open blocker starts `blocked`;
    /// one with nobody to do it starts in the backlog.
    pub fn create(&self, actor: &str, new: &NewIssue) -> Result<(Issue, bool)> {
        self.locked(|board, out| {
            let title = clean_line("the title", &new.title, MAX_TITLE)?;
            let body = clean_text("the description", &new.body, MAX_BODY)?;
            if new.acceptance.len() > MAX_ACCEPTANCE {
                bail!("at most {MAX_ACCEPTANCE} acceptance criteria");
            }
            let acceptance = new.acceptance.iter().map(|a| clean_line("an acceptance criterion", a, 300)).collect::<Result<Vec<_>>>()?;
            if let Some(key) = &new.key
                && let Some(found) = board.issues.values().find(|i| i.key.as_deref() == Some(key) && i.parent == new.parent)
            {
                return Ok((found.clone(), false));
            }
            if board.issues.len() >= MAX_ISSUES {
                bail!("the board is full: {MAX_ISSUES} issues");
            }
            let id = match &new.parent {
                Some(parent) => {
                    let parent_issue = board.issues.get(parent).ok_or_else(|| anyhow::anyhow!("there is no issue {parent}"))?;
                    if depth(&parent_issue.id) >= MAX_DEPTH {
                        bail!("issues can be nested at most {MAX_DEPTH} deep");
                    }
                    let siblings = board.issues.values().filter(|i| i.parent.as_deref() == Some(parent)).count();
                    if siblings >= MAX_CHILDREN {
                        bail!("{parent} already has {MAX_CHILDREN} child issues");
                    }
                    let mut n = siblings + 1;
                    while board.issues.contains_key(&format!("{parent}.{n}")) {
                        n += 1;
                    }
                    format!("{parent}.{n}")
                }
                None => fresh_id(board, &title),
            };
            for blocker in &new.blocked_by {
                if !board.issues.contains_key(blocker) {
                    bail!("there is no issue {blocker} to wait for");
                }
            }
            let waiting = new.blocked_by.iter().any(|b| board.issues.get(b).is_some_and(|i| !matches!(i.status, Status::Done | Status::Cancelled)));
            let status = if new.assignee.is_none() {
                Status::Backlog
            } else if waiting {
                Status::Blocked
            } else {
                Status::Ready
            };
            let time = now();
            let issue = Issue {
                id: id.clone(),
                title,
                body,
                kind: new.kind.unwrap_or(Kind::Task),
                status,
                assignee: new.assignee.clone(),
                track: new.track.clone(),
                parent: new.parent.clone(),
                blocked_by: new.blocked_by.clone(),
                acceptance,
                priority: new.priority.unwrap_or(2).min(3),
                creator: actor.to_owned(),
                created: time.clone(),
                updated: time,
                comments: Vec::new(),
                hold: None,
                run: None,
                key: new.key.clone(),
            };
            board.issues.insert(id, issue.clone());
            out.push((format!("{actor}\tcreated"), issue.clone()));
            Ok((issue, true))
        })
    }

    /// Changes who works an issue. Giving a backlog issue an assignee makes it ready (or blocked, if it waits).
    pub fn assign(&self, actor: &str, id: &str, assignee: Option<&str>) -> Result<Issue> {
        self.edit(actor, id, "assigned", |issue, _| {
            if !issue.status.is_open() {
                bail!("{} is {}; reopen it first", issue.id, issue.status.word());
            }
            issue.assignee = assignee.map(str::to_owned);
            if issue.status == Status::InProgress {
                issue.status = Status::Ready;
                issue.run = None;
            }
            if issue.assignee.is_none() && matches!(issue.status, Status::Ready | Status::Blocked) {
                issue.status = Status::Backlog;
            } else if issue.assignee.is_some() && issue.status == Status::Backlog {
                issue.status = Status::Ready;
            }
            Ok(())
        })
    }

    /// Takes a ready issue for `member`: ready, assigned to them (or to nobody), and they have no other issue in progress.
    pub fn claim(&self, id: &str, member: &str, run: Option<&str>) -> Result<Issue> {
        self.locked(|board, out| {
            let busy = board.issues.values().find(|i| i.status == Status::InProgress && i.assignee.as_deref() == Some(member) && i.id != id);
            if let Some(other) = busy {
                bail!("{member} is already working on {}; finish or release it first", other.id);
            }
            let issue = board.issues.get_mut(id).ok_or_else(|| anyhow::anyhow!("there is no issue {id}"))?;
            if issue.status != Status::Ready {
                bail!("{id} is {}, not ready", issue.status.word());
            }
            if issue.assignee.as_deref().is_some_and(|a| a != member) {
                bail!("{id} is assigned to {}", issue.assignee.as_deref().unwrap_or("someone else"));
            }
            issue.assignee = Some(member.to_owned());
            issue.status = Status::InProgress;
            issue.run = run.map(str::to_owned);
            issue.updated = now();
            let done = issue.clone();
            out.push((format!("{member}\tclaimed"), done.clone()));
            Ok(done)
        })
    }

    /// Moves an issue to `to`, if the move is allowed. `note` is added as a comment. A move to `blocked` by hand puts the
    /// issue on hold with `note` as the reason; a move to `ready` lifts the hold.
    pub fn set_status(&self, actor: &str, id: &str, to: Status, note: Option<&str>) -> Result<Issue> {
        self.locked(|board, out| {
            let issue = board.issues.get_mut(id).ok_or_else(|| anyhow::anyhow!("there is no issue {id}"))?;
            if !issue.status.may_move_to(to) {
                bail!("{id} cannot move from {} to {}", issue.status.word(), to.word());
            }
            let from = issue.status;
            issue.status = to;
            issue.updated = now();
            match to {
                Status::Blocked => issue.hold = Some(note.unwrap_or("put on hold").to_owned()),
                Status::Ready => {
                    issue.hold = None;
                    issue.run = None;
                }
                Status::Done | Status::Cancelled | Status::InReview => issue.run = None,
                _ => {}
            }
            if let Some(note) = note.filter(|n| !n.trim().is_empty()) {
                push_comment(issue, actor, note)?;
            }
            out.push((format!("{actor}\tstatus {} -> {}", from.word(), to.word()), issue.clone()));
            settle(board, out, actor);
            board.issues.get(id).cloned().ok_or_else(|| anyhow::anyhow!("issue {id} is no longer on the board"))
        })
    }

    pub fn comment(&self, actor: &str, id: &str, text: &str) -> Result<Issue> {
        self.edit(actor, id, "comment", |issue, actor| push_comment(issue, actor, text))
    }

    /// Changes the title, description, acceptance criteria or priority.
    pub fn update(&self, actor: &str, id: &str, title: Option<&str>, body: Option<&str>, acceptance: Option<&[String]>, priority: Option<u8>) -> Result<Issue> {
        self.edit(actor, id, "updated", |issue, _| {
            if let Some(title) = title {
                issue.title = clean_line("the title", title, MAX_TITLE)?;
            }
            if let Some(body) = body {
                issue.body = clean_text("the description", body, MAX_BODY)?;
            }
            if let Some(items) = acceptance {
                if items.len() > MAX_ACCEPTANCE {
                    bail!("at most {MAX_ACCEPTANCE} acceptance criteria");
                }
                issue.acceptance = items.iter().map(|a| clean_line("an acceptance criterion", a, 300)).collect::<Result<_>>()?;
            }
            if let Some(priority) = priority {
                issue.priority = priority.min(3);
            }
            Ok(())
        })
    }

    /// Makes `id` wait for `blocker`. A dependency that would make a loop is refused.
    pub fn add_dependency(&self, actor: &str, id: &str, blocker: &str) -> Result<Issue> {
        self.locked(|board, out| {
            if id == blocker {
                bail!("an issue cannot wait for itself");
            }
            if !board.issues.contains_key(blocker) {
                bail!("there is no issue {blocker}");
            }
            if waits_for(board, blocker, id) {
                bail!("{blocker} already waits for {id}, so this would make a loop");
            }
            let issue = board.issues.get_mut(id).ok_or_else(|| anyhow::anyhow!("there is no issue {id}"))?;
            if !issue.blocked_by.iter().any(|b| b == blocker) {
                issue.blocked_by.push(blocker.to_owned());
            }
            issue.updated = now();
            out.push((format!("{actor}\tdepends on {blocker}"), issue.clone()));
            settle(board, out, actor);
            board.issues.get(id).cloned().ok_or_else(|| anyhow::anyhow!("issue {id} is no longer on the board"))
        })
    }

    /// Records the run that works an issue.
    pub fn link_run(&self, actor: &str, id: &str, run: Option<&str>) -> Result<Issue> {
        self.edit(actor, id, "linked", |issue, _| {
            issue.run = run.map(str::to_owned);
            Ok(())
        })
    }

    /// After a crash or a restart no run is working: every issue in progress goes back to ready with a comment.
    pub fn recover(&self) -> Result<Vec<String>> {
        self.locked(|board, out| {
            let stale: Vec<String> = board.issues.values().filter(|i| i.status == Status::InProgress).map(|i| i.id.clone()).collect();
            for id in &stale {
                if let Some(issue) = board.issues.get_mut(id) {
                    issue.status = Status::Ready;
                    issue.run = None;
                    issue.updated = now();
                    let _ = push_comment(issue, "bridle", "The work was interrupted (Bridle was closed or stopped). The issue is ready again.");
                    out.push(("bridle\trecovered".to_owned(), issue.clone()));
                }
            }
            Ok(stale)
        })
    }

    /// An edit that does not change the state: runs `change`, stamps the time, and records one entry.
    fn edit(&self, actor: &str, id: &str, action: &str, change: impl FnOnce(&mut Issue, &str) -> Result<()>) -> Result<Issue> {
        self.locked(|board, out| {
            let issue = board.issues.get_mut(id).ok_or_else(|| anyhow::anyhow!("there is no issue {id}"))?;
            change(issue, actor)?;
            issue.updated = now();
            out.push((format!("{actor}\t{action}"), issue.clone()));
            settle(board, out, actor);
            board.issues.get(id).cloned().ok_or_else(|| anyhow::anyhow!("issue {id} is no longer on the board"))
        })
    }

    /// The Markdown of the board, for sharing or review.
    pub fn export_markdown(&self) -> Result<String> {
        let board = self.load()?;
        let mut text = String::from("# Board\n\n");
        for status in [Status::InProgress, Status::InReview, Status::Ready, Status::Blocked, Status::Backlog, Status::Done, Status::Cancelled] {
            let issues: Vec<&Issue> = board.issues.values().filter(|i| i.status == status).collect();
            if issues.is_empty() {
                continue;
            }
            text.push_str(&format!("## {}\n\n", status.word().replace('_', " ")));
            for issue in issues {
                let who = issue.assignee.as_deref().unwrap_or("nobody");
                text.push_str(&format!("### {} {}\n\nAssigned to: {who}. Priority: p{}.\n\n", issue.id, issue.title, issue.priority));
                if !issue.body.is_empty() {
                    text.push_str(&format!("{}\n\n", issue.body));
                }
                for item in &issue.acceptance {
                    text.push_str(&format!("- [ ] {item}\n"));
                }
                if !issue.acceptance.is_empty() {
                    text.push('\n');
                }
            }
        }
        Ok(text)
    }
}

impl Board {
    /// The issues that can be started now: ready, in priority order and then oldest first. With `member`, only those assigned
    /// to them or to nobody.
    pub fn ready(&self, member: Option<&str>) -> Vec<&Issue> {
        let mut ready: Vec<&Issue> = self
            .issues
            .values()
            .filter(|i| i.status == Status::Ready && member.is_none_or(|m| i.assignee.as_deref().is_none_or(|a| a == m)))
            .collect();
        ready.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.created.cmp(&b.created)).then_with(|| a.id.cmp(&b.id)));
        ready
    }

    pub fn children(&self, id: &str) -> Vec<&Issue> {
        self.issues.values().filter(|i| i.parent.as_deref() == Some(id)).collect()
    }

    /// How many open issues each member has, for the badges and for picking who has the least work.
    pub fn load_by_member(&self) -> BTreeMap<String, usize> {
        let mut load: BTreeMap<String, usize> = BTreeMap::new();
        for issue in self.issues.values().filter(|i| i.status.is_open()) {
            if let Some(who) = &issue.assignee {
                *load.entry(who.clone()).or_insert(0) += 1;
            }
        }
        load
    }
}

/// How deep an id is nested: `B-a3f8` is 1, `B-a3f8.1` is 2.
fn depth(id: &str) -> usize {
    id.matches('.').count() + 1
}

fn fresh_id(board: &Board, title: &str) -> String {
    let mut salt = 0u64;
    loop {
        let hash = checksum(&format!("{}|{}|{title}|{salt}", board.seq, now()));
        let id = format!("B-{}", &hash[..4]);
        if !board.issues.contains_key(&id) {
            return id;
        }
        salt += 1;
    }
}

fn clean_line(what: &str, text: &str, max: usize) -> Result<String> {
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    let text = text.trim().to_owned();
    if text.is_empty() {
        bail!("{what} cannot be empty");
    }
    if text.chars().count() > max {
        bail!("{what} can be at most {max} characters long");
    }
    Ok(text)
}

fn clean_text(what: &str, text: &str, max: usize) -> Result<String> {
    let text: String = text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect();
    let text = text.trim().to_owned();
    if text.chars().count() > max {
        bail!("{what} can be at most {max} characters long");
    }
    Ok(text)
}

fn push_comment(issue: &mut Issue, author: &str, text: &str) -> Result<()> {
    let text = clean_text("a comment", text, MAX_COMMENT)?;
    if text.is_empty() {
        bail!("a comment cannot be empty");
    }
    if issue.comments.len() >= MAX_COMMENTS {
        bail!("{} has {MAX_COMMENTS} comments already", issue.id);
    }
    issue.comments.push(Comment { author: author.to_owned(), time: now(), text });
    Ok(())
}

/// Whether `from` waits, directly or through others, for `target`.
fn waits_for(board: &Board, from: &str, target: &str) -> bool {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = vec![from];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(issue) = board.issues.get(id) {
            for blocker in &issue.blocked_by {
                if blocker == target {
                    return true;
                }
                stack.push(blocker);
            }
        }
    }
    false
}

/// Applies the rules that follow from a change: an issue waiting for an open blocker is blocked; a blocked issue whose
/// blockers are all done (and that is not on hold) is ready; and when the last child of an issue is done the parent moves
/// to review, so whoever owns it can close it. Each change is recorded.
fn settle(board: &mut Board, out: &mut Vec<(String, Issue)>, actor: &str) {
    loop {
        let mut changed = false;
        let ids: Vec<String> = board.issues.keys().cloned().collect();
        for id in ids {
            let open_blocker = board.issues[&id].blocked_by.iter().any(|b| board.issues.get(b).is_some_and(|i| !matches!(i.status, Status::Done | Status::Cancelled)));
            let issue = &board.issues[&id];
            let next = match issue.status {
                Status::Ready if open_blocker => Some((Status::Blocked, "blocked by an open issue")),
                Status::Blocked if !open_blocker && issue.hold.is_none() => Some((Status::Ready, "its blockers are done")),
                _ => None,
            };
            if let Some((status, why)) = next
                && let Some(issue) = board.issues.get_mut(&id)
            {
                issue.status = status;
                issue.updated = now();
                out.push((format!("{actor}\tauto: {why}"), issue.clone()));
                changed = true;
            }
        }
        let parents: Vec<String> = board.issues.values().filter_map(|i| i.parent.clone()).collect::<BTreeSet<_>>().into_iter().collect();
        for parent in parents {
            let children: Vec<&Issue> = board.children(&parent);
            let all_done = !children.is_empty() && children.iter().all(|c| matches!(c.status, Status::Done | Status::Cancelled)) && children.iter().any(|c| c.status == Status::Done);
            let waiting = board.issues.get(&parent).is_some_and(|p| matches!(p.status, Status::InProgress | Status::Ready));
            if all_done
                && waiting
                && let Some(issue) = board.issues.get_mut(&parent)
            {
                issue.status = Status::InReview;
                issue.run = None;
                issue.updated = now();
                let _ = push_comment(issue, "bridle", "All the child issues are done. This issue is ready for its owner to check and close.");
                out.push((format!("{actor}\tauto: all children done"), issue.clone()));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Removes a partly written last line, so the next entry starts on a line of its own.
fn trim_torn_tail(path: &Path) -> Result<()> {
    let Ok(bytes) = fs::read(path) else { return Ok(()) };
    if bytes.is_empty() || bytes.last() == Some(&b'\n') {
        return Ok(());
    }
    let keep = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(u64::try_from(keep)?)?;
    file.sync_all()?;
    Ok(())
}

fn read_board(path: &Path) -> Result<Board> {
    let mut board = Board::default();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(board),
        Err(error) => return Err(error.into()),
    };
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Entry>(line) {
            Ok(entry) if entry.sum == entry_sum(entry.seq, &entry.time, &entry.actor, &entry.action, &entry.issue) => {
                board.seq = board.seq.max(entry.seq);
                board.issues.insert(entry.issue.id.clone(), entry.issue);
            }
            _ => board.skipped += 1,
        }
    }
    Ok(board)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, BoardStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = BoardStore::open(dir.path());
        (dir, store)
    }

    fn new(title: &str, assignee: Option<&str>) -> NewIssue {
        NewIssue { title: title.into(), assignee: assignee.map(str::to_owned), ..NewIssue::default() }
    }

    fn status(store: &BoardStore, id: &str) -> Status {
        store.load().unwrap().issues[id].status
    }

    #[test]
    fn an_issue_starts_in_the_backlog_or_ready_by_whether_someone_owns_it() {
        let (_d, s) = store();
        let (a, created) = s.create("user", &new("no owner", None)).unwrap();
        assert!(created);
        assert_eq!(a.status, Status::Backlog);
        let (b, _) = s.create("user", &new("with owner", Some("maya"))).unwrap();
        assert_eq!(b.status, Status::Ready);
        assert!(b.id.starts_with("B-") && b.id.len() == 6, "{}", b.id);
    }

    #[test]
    fn the_project_manager_flow_children_wait_for_their_blocker_and_release_the_parent() {
        let (_d, s) = store();
        let (parent, _) = s.create("priya", &new("Add a cache", Some("priya"))).unwrap();
        s.claim(&parent.id, "priya", None).unwrap();
        let child = |title: &str, who: &str, blockers: &[&str]| NewIssue {
            title: title.into(),
            assignee: Some(who.into()),
            parent: Some(parent.id.clone()),
            blocked_by: blockers.iter().map(|b| (*b).to_owned()).collect(),
            ..NewIssue::default()
        };
        let (eng, _) = s.create("priya", &child("Implement the cache", "maya", &[])).unwrap();
        let (qa, _) = s.create("priya", &child("Review and run the checks", "lena", &[&eng.id])).unwrap();
        let (product, _) = s.create("priya", &child("Write the release note", "omar", &[&eng.id])).unwrap();
        assert_eq!(eng.id, format!("{}.1", parent.id));
        assert_eq!(status(&s, &eng.id), Status::Ready);
        assert_eq!(status(&s, &qa.id), Status::Blocked, "QA waits for the engineer");
        assert_eq!(status(&s, &product.id), Status::Blocked);
        let ready: Vec<String> = s.load().unwrap().ready(None).iter().map(|i| i.id.clone()).collect();
        assert_eq!(ready, [eng.id.clone()]);
        s.claim(&eng.id, "maya", None).unwrap();
        s.set_status("maya", &eng.id, Status::Done, Some("done, tests pass")).unwrap();
        assert_eq!(status(&s, &qa.id), Status::Ready, "finishing the blocker releases the others");
        assert_eq!(status(&s, &product.id), Status::Ready);
        assert_eq!(status(&s, &parent.id), Status::InProgress, "not yet: two children are open");
        for id in [&qa.id, &product.id] {
            let who = if *id == qa.id { "lena" } else { "omar" };
            s.claim(id, who, None).unwrap();
            s.set_status(who, id, Status::Done, None).unwrap();
        }
        assert_eq!(status(&s, &parent.id), Status::InReview, "the last child done returns the parent to its owner");
        s.set_status("priya", &parent.id, Status::Done, Some("shipped")).unwrap();
        assert_eq!(status(&s, &parent.id), Status::Done);
    }

    #[test]
    fn creating_twice_with_the_same_key_returns_the_first_issue() {
        let (_d, s) = store();
        let one = NewIssue { key: Some("k1".into()), ..new("Once", Some("maya")) };
        let (first, created) = s.create("priya", &one).unwrap();
        let (again, created_again) = s.create("priya", &one).unwrap();
        assert!(created && !created_again);
        assert_eq!(first.id, again.id);
        assert_eq!(s.load().unwrap().issues.len(), 1);
    }

    #[test]
    fn one_claim_wins_and_a_member_works_one_issue_at_a_time() {
        let (_d, s) = store();
        let (a, _) = s.create("u", &new("a", Some("maya"))).unwrap();
        let (b, _) = s.create("u", &new("b", Some("maya"))).unwrap();
        s.claim(&a.id, "maya", None).unwrap();
        assert!(s.claim(&a.id, "maya", None).is_err(), "already in progress");
        let error = s.claim(&b.id, "maya", None).unwrap_err().to_string();
        assert!(error.contains("already working"), "{error}");
        let (c, _) = s.create("u", &new("c", Some("sam"))).unwrap();
        assert!(s.claim(&c.id, "noor", None).unwrap_err().to_string().contains("assigned to sam"));
        s.set_status("maya", &a.id, Status::InReview, None).unwrap();
        s.claim(&b.id, "maya", None).unwrap();
    }

    #[test]
    fn only_allowed_moves_are_made_and_a_manual_block_holds_until_released() {
        let (_d, s) = store();
        let (a, _) = s.create("u", &new("a", Some("maya"))).unwrap();
        assert!(s.set_status("u", &a.id, Status::Done, None).is_err(), "ready cannot jump to done");
        assert!(s.set_status("u", &a.id, Status::Ready, None).is_err(), "same state");
        s.set_status("u", &a.id, Status::Blocked, Some("needs a decision")).unwrap();
        assert_eq!(s.load().unwrap().issues[&a.id].hold.as_deref(), Some("needs a decision"));
        s.set_status("u", &a.id, Status::Ready, None).unwrap();
        assert!(s.load().unwrap().issues[&a.id].hold.is_none());
        s.set_status("u", &a.id, Status::Cancelled, None).unwrap();
        assert!(s.claim(&a.id, "maya", None).is_err());
        s.set_status("u", &a.id, Status::Backlog, None).unwrap();
    }

    #[test]
    fn a_dependency_loop_is_refused_and_a_missing_issue_is_refused() {
        let (_d, s) = store();
        let (a, _) = s.create("u", &new("a", Some("maya"))).unwrap();
        let (b, _) = s.create("u", &NewIssue { blocked_by: vec![a.id.clone()], ..new("b", Some("sam")) }).unwrap();
        let error = s.add_dependency("u", &a.id, &b.id).unwrap_err().to_string();
        assert!(error.contains("loop"), "{error}");
        assert!(s.add_dependency("u", &a.id, &a.id).is_err());
        assert!(s.create("u", &NewIssue { blocked_by: vec!["B-nope".into()], ..new("c", None) }).is_err());
        assert!(s.set_status("u", "B-nope", Status::Ready, None).is_err());
    }

    #[test]
    fn nesting_and_the_number_of_children_are_limited() {
        let (_d, s) = store();
        let (root, _) = s.create("u", &new("root", Some("maya"))).unwrap();
        let (child, _) = s.create("u", &NewIssue { parent: Some(root.id.clone()), ..new("child", Some("maya")) }).unwrap();
        let (grand, _) = s.create("u", &NewIssue { parent: Some(child.id.clone()), ..new("grandchild", Some("maya")) }).unwrap();
        let too_deep = s.create("u", &NewIssue { parent: Some(grand.id.clone()), ..new("too deep", None) });
        assert!(too_deep.unwrap_err().to_string().contains("nested"));
        for n in 1..MAX_CHILDREN {
            s.create("u", &NewIssue { parent: Some(root.id.clone()), ..new(&format!("c{n}"), None) }).unwrap();
        }
        let full = s.create("u", &NewIssue { parent: Some(root.id.clone()), ..new("one more", None) });
        assert!(full.unwrap_err().to_string().contains("child issues"));
    }

    #[test]
    fn the_ready_queue_is_ordered_by_priority_and_filtered_by_member() {
        let (_d, s) = store();
        s.create("u", &NewIssue { priority: Some(3), ..new("low", Some("maya")) }).unwrap();
        s.create("u", &NewIssue { priority: Some(0), ..new("urgent", Some("maya")) }).unwrap();
        s.create("u", &new("someone else", Some("sam"))).unwrap();
        s.create("u", &new("anyone", None)).unwrap();
        let board = s.load().unwrap();
        let titles: Vec<&str> = board.ready(Some("maya")).iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["urgent", "low"], "backlog and other people's issues are not offered");
        assert_eq!(board.load_by_member().get("maya"), Some(&2));
    }

    #[test]
    fn after_a_crash_issues_in_progress_return_to_ready_with_a_comment() {
        let (_d, s) = store();
        let (a, _) = s.create("u", &new("a", Some("maya"))).unwrap();
        s.claim(&a.id, "maya", Some("run-1")).unwrap();
        assert_eq!(s.recover().unwrap(), [a.id.clone()]);
        let board = s.load().unwrap();
        assert_eq!(board.issues[&a.id].status, Status::Ready);
        assert!(board.issues[&a.id].run.is_none());
        assert!(board.issues[&a.id].comments.last().unwrap().text.contains("interrupted"));
        assert!(s.recover().unwrap().is_empty());
    }

    #[test]
    fn the_board_survives_a_new_store_a_torn_last_line_and_a_damaged_line() {
        let (dir, s) = store();
        let (a, _) = s.create("u", &new("a", Some("maya"))).unwrap();
        s.comment("maya", &a.id, "first").unwrap();
        let log = dir.path().join(".bridle").join("board").join("events.jsonl");
        let mut text = fs::read_to_string(&log).unwrap();
        // A write cut short in the middle of a line.
        text.push_str("{\"seq\": 99, \"time\": \"x\", \"actor\": \"maya\", \"act");
        fs::write(&log, &text).unwrap();
        let again = BoardStore::open(dir.path());
        assert_eq!(again.load().unwrap().issues[&a.id].comments.len(), 1);
        again.comment("maya", &a.id, "second").unwrap();
        let board = again.load().unwrap();
        assert_eq!(board.issues[&a.id].comments.len(), 2, "the torn tail was dropped before appending");
        assert_eq!(board.skipped, 0);
        // A line whose content was changed no longer matches its checksum.
        let damaged = fs::read_to_string(&log).unwrap().replacen("first", "forged", 1);
        fs::write(&log, damaged).unwrap();
        let board = again.load().unwrap();
        assert_eq!(board.skipped, 1);
        assert_eq!(board.issues[&a.id].comments.len(), 2, "later lines still give the newest state");
    }

    #[test]
    fn many_writers_create_distinct_issues_and_lose_none() {
        let (dir, s) = store();
        let path = dir.path().to_path_buf();
        let handles: Vec<_> = (0..6)
            .map(|w| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let s = BoardStore::open(&path);
                    (0..5).map(|n| s.create("u", &new(&format!("w{w}-{n}"), Some("maya"))).unwrap().0.id).collect::<Vec<_>>()
                })
            })
            .collect();
        let mut ids: Vec<String> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 30);
        assert_eq!(s.load().unwrap().issues.len(), 30);
    }

    #[test]
    fn titles_comments_and_the_board_size_are_checked() {
        let (_d, s) = store();
        assert!(s.create("u", &new("   ", None)).is_err());
        assert!(s.create("u", &new(&"x".repeat(MAX_TITLE + 1), None)).is_err());
        let (a, _) = s.create("u", &new("a", None)).unwrap();
        assert!(s.comment("u", &a.id, "  ").is_err());
        assert!(s.comment("u", &a.id, &"x".repeat(MAX_COMMENT + 1)).is_err());
        let many = NewIssue { acceptance: vec!["x".into(); MAX_ACCEPTANCE + 1], ..new("b", None) };
        assert!(s.create("u", &many).is_err());
    }

    #[test]
    fn assigning_moves_a_backlog_issue_to_ready_and_unassigning_back() {
        let (_d, s) = store();
        let (a, _) = s.create("u", &new("a", None)).unwrap();
        assert_eq!(s.assign("u", &a.id, Some("sam")).unwrap().status, Status::Ready);
        assert_eq!(s.assign("u", &a.id, None).unwrap().status, Status::Backlog);
        s.assign("u", &a.id, Some("sam")).unwrap();
        s.claim(&a.id, "sam", None).unwrap();
        let moved = s.assign("u", &a.id, Some("noor")).unwrap();
        assert_eq!((moved.status, moved.assignee.as_deref()), (Status::Ready, Some("noor")), "reassigning an issue in progress releases it");
    }

    #[test]
    fn the_markdown_export_lists_issues_by_state() {
        let (_d, s) = store();
        s.create("u", &NewIssue { acceptance: vec!["tests pass".into()], ..new("Ship it", Some("maya")) }).unwrap();
        let text = s.export_markdown().unwrap();
        assert!(text.contains("## ready") && text.contains("Ship it") && text.contains("- [ ] tests pass"), "{text}");
    }
}
