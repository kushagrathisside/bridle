//! Per-task storage.
//!
//! Each task has a directory `.local-agent/tasks/<task_id>/` with:
//! - `task-events.jsonl`: the journal, the source of truth. Entries carry a `seq`.
//! - `task.json`: a cache of the folded state, written after each append.
//! - `decisions.jsonl`: one record per decision, with the signals it was made from.
//!
//! `.local-agent/state.json` maps OpenCode session ids to task ids. Lock order is fixed: the workspace
//! lock (`.local-agent/.lock`) first, then a task lock (`tasks/<task_id>/lock`). Never the reverse.
//! Every append and every cache write holds the lock of the file it touches.

use std::collections::BTreeMap;
use std::path::PathBuf;

use harness_core::decision::DecisionResult;
use harness_core::journal::{Entry, Journal, Outcome, Record};
use harness_core::loops::LoopSignal;
use harness_core::task::TaskStatus;
use serde::{Deserialize, Serialize};

use super::{FileLock, LOCK_TIMEOUT, StateDir, StateError};

/// Directory under the state directory that holds one directory per task.
pub const TASKS_DIR: &str = "tasks";
/// The journal of a task.
pub const JOURNAL_FILE: &str = "task-events.jsonl";
/// The cache of a task's folded state.
pub const CACHE_FILE: &str = "task.json";
/// The decisions of a task.
pub const DECISIONS_FILE: &str = "decisions.jsonl";
/// The session index in the state directory.
pub const INDEX_FILE: &str = "state.json";
/// The results of verification tools, written by the MCP server.
pub const MCP_RESULTS_FILE: &str = "mcp-results.jsonl";
/// The lock file of a task, inside its directory.
const TASK_LOCK: &str = "lock";
/// The longest session id accepted. Session ids come from the host and become keys and file names.
const MAX_SESSION_ID: usize = 128;

/// Which OpenCode session belongs to which task, and the number of the last task created.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIndex {
    pub next_task: u32,
    pub sessions: BTreeMap<String, String>,
    /// Sequence numbers of MCP results already claimed by a tool-after hook.
    #[serde(default)]
    pub claimed_mcp: Vec<u64>,
    /// A task started or resumed from the command line, waiting for the next new session.
    #[serde(default)]
    pub pending_task: Option<String>,
    /// Tasks that run in a worktree, with the worktree path relative to the repository root.
    #[serde(default)]
    pub worktrees: BTreeMap<String, String>,
}

/// A verification result the MCP server recorded. The tool-after hook claims it for the matching call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResult {
    pub seq: u64,
    /// The tool name without the host's prefix, such as `run_tests`.
    pub tool: String,
    /// The hash of the canonical arguments, from [`harness_core::fingerprint::args_hash`].
    pub args_hash: String,
    pub outcome: Outcome,
    pub fingerprints: Vec<String>,
}

/// One decision, as written to `decisions.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub call_id: String,
    pub signals: Vec<LoopSignal>,
    pub result: DecisionResult,
    /// Whether the call was refused. A decision can be recorded without a refusal, for example when
    /// the cap has been reached.
    pub refused: bool,
}

/// A task in the state directory.
#[derive(Debug)]
pub struct Task<'a> {
    state: &'a StateDir,
    id: String,
}

impl StateDir {
    /// The task with this id. The id must have the form `task-NNNN`, so it can name a directory.
    pub fn task(&self, id: &str) -> Result<Task<'_>, StateError> {
        validate_task_id(id)?;
        Ok(Task {
            state: self,
            id: id.to_owned(),
        })
    }

    /// The task bound to an OpenCode session. The first call from a session creates a task and records
    /// its start in the journal.
    pub fn bind_session(&self, session_id: &str, repository: &str) -> Result<String, StateError> {
        validate_session_id(session_id)?;
        let _workspace = self.lock()?;
        let mut index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        if let Some(id) = index.sessions.get(session_id) {
            return Ok(id.clone());
        }
        if let Some(id) = index.pending_task.take() {
            index.sessions.insert(session_id.to_owned(), id.clone());
            self.write_json(INDEX_FILE, &index)?;
            return Ok(id);
        }
        index.next_task += 1;
        let id = format!("task-{:04}", index.next_task);
        index.sessions.insert(session_id.to_owned(), id.clone());
        self.write_json(INDEX_FILE, &index)?;
        self.task(&id)?.append(Record::TaskStarted {
            task_id: id.clone(),
            repository: repository.to_owned(),
            prompt: String::new(),
        })?;
        Ok(id)
    }

    /// Appends a verification result from the MCP server. Takes the workspace lock and returns its `seq`.
    pub fn append_mcp_result(
        &self,
        tool: &str,
        args_hash: &str,
        outcome: Outcome,
        fingerprints: Vec<String>,
    ) -> Result<u64, StateError> {
        let _workspace = self.lock()?;
        let existing = self.read_records::<McpResult>(MCP_RESULTS_FILE)?.records;
        let seq = existing.last().map_or(1, |last| last.seq + 1);
        let result = McpResult {
            seq,
            tool: tool.to_owned(),
            args_hash: args_hash.to_owned(),
            outcome,
            fingerprints,
        };
        self.append_unlocked(MCP_RESULTS_FILE, &result)?;
        Ok(seq)
    }

    /// Claims the oldest unclaimed result of `tool` with these arguments. The claim is kept in the
    /// index, so each result is used once.
    pub fn claim_mcp_result(
        &self,
        tool: &str,
        args_hash: &str,
    ) -> Result<Option<McpResult>, StateError> {
        let _workspace = self.lock()?;
        let mut index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        let results = self.read_records::<McpResult>(MCP_RESULTS_FILE)?.records;
        let found = results.into_iter().find(|result| {
            result.tool == tool
                && result.args_hash == args_hash
                && !index.claimed_mcp.contains(&result.seq)
        });
        if let Some(result) = &found {
            index.claimed_mcp.push(result.seq);
            self.write_json(INDEX_FILE, &index)?;
        }
        Ok(found)
    }

    /// Takes the next task number and returns its id. A worktree task records its path in the index.
    pub fn reserve_task_id(&self, worktree: Option<&str>) -> Result<String, StateError> {
        let _workspace = self.lock()?;
        let mut index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        index.next_task += 1;
        let id = format!("task-{:04}", index.next_task);
        if let Some(path) = worktree {
            index.worktrees.insert(id.clone(), path.to_owned());
        }
        self.write_json(INDEX_FILE, &index)?;
        Ok(id)
    }

    /// Records the worktree path of a reserved task.
    pub fn register_worktree(&self, id: &str, path: &str) -> Result<(), StateError> {
        let _workspace = self.lock()?;
        let mut index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        index.worktrees.insert(id.to_owned(), path.to_owned());
        self.write_json(INDEX_FILE, &index)
    }

    /// Forgets a reserved worktree task, for example when `git worktree add` failed.
    pub fn release_task_id(&self, id: &str) -> Result<(), StateError> {
        let _workspace = self.lock()?;
        let mut index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        index.worktrees.remove(id);
        self.write_json(INDEX_FILE, &index)
    }

    /// The worktree path of a task, when the task runs in a worktree.
    pub fn worktree_of(&self, id: &str) -> Result<Option<String>, StateError> {
        let index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        Ok(index.worktrees.get(id).cloned())
    }

    /// Creates a task with a given id in this state directory, and leaves the next new session bound to it.
    /// Used for a worktree, whose number comes from the primary repository.
    pub fn start_task_with_id(
        &self,
        id: &str,
        repository: &str,
        prompt: &str,
    ) -> Result<(), StateError> {
        let _workspace = self.lock()?;
        let mut index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        index.pending_task = Some(id.to_owned());
        self.write_json(INDEX_FILE, &index)?;
        self.task(id)?.append(Record::TaskStarted {
            task_id: id.to_owned(),
            repository: repository.to_owned(),
            prompt: prompt.to_owned(),
        })?;
        Ok(())
    }

    /// Starts a task and leaves the next new OpenCode session bound to it. Takes the workspace lock.
    pub fn start_task(&self, repository: &str, prompt: &str) -> Result<String, StateError> {
        let _workspace = self.lock()?;
        let mut index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        index.next_task += 1;
        let id = format!("task-{:04}", index.next_task);
        index.pending_task = Some(id.clone());
        self.write_json(INDEX_FILE, &index)?;
        self.task(&id)?.append(Record::TaskStarted {
            task_id: id.clone(),
            repository: repository.to_owned(),
            prompt: prompt.to_owned(),
        })?;
        Ok(id)
    }

    /// Leaves the next new OpenCode session bound to an existing task.
    pub fn resume_task(&self, id: &str) -> Result<(), StateError> {
        let task = self.task(id)?;
        if task.entries()?.is_empty() {
            return Err(StateError::InvalidId {
                what: "task id",
                value: id.to_owned(),
                rule: "no task has this id",
            });
        }
        let _workspace = self.lock()?;
        let mut index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        index.pending_task = Some(id.to_owned());
        self.write_json(INDEX_FILE, &index)
    }

    /// The ids of the tasks on disk, in order.
    pub fn task_ids(&self) -> Result<Vec<String>, StateError> {
        let dir = self.root().join(TASKS_DIR);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(StateError::Io {
                    action: "list",
                    path: dir,
                    source,
                });
            }
        };
        let mut ids: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| validate_task_id(name).is_ok())
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Removes the directories of finished tasks (completed or aborted) and their session bindings.
    /// Explicit only: nothing calls this automatically. Returns the ids removed.
    pub fn prune_finished(&self) -> Result<Vec<String>, StateError> {
        let _workspace = self.lock()?;
        let mut index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        let mut removed = Vec::new();
        for id in self.task_ids()? {
            let journal = self.task(&id)?.snapshot()?;
            if !matches!(
                journal.state.status,
                TaskStatus::Completed | TaskStatus::Aborted
            ) {
                continue;
            }
            let dir = task_dir(self, &id);
            std::fs::remove_dir_all(&dir).map_err(|source| StateError::Io {
                action: "remove",
                path: dir,
                source,
            })?;
            index.sessions.retain(|_, bound| bound != &id);
            if index.pending_task.as_deref() == Some(id.as_str()) {
                index.pending_task = None;
            }
            removed.push(id);
        }
        self.write_json(INDEX_FILE, &index)?;
        Ok(removed)
    }

    /// Rebuilds the cache of every task from its journal. A damaged session index is set aside and a new
    /// one is written, so the journals still describe every task. Returns the ids rebuilt.
    pub fn repair_tasks(&self) -> Result<Vec<String>, StateError> {
        let _workspace = self.lock()?;
        let index_path = self.path(INDEX_FILE);
        if let Err(StateError::Corrupt { .. }) = self.read_json::<SessionIndex>(INDEX_FILE) {
            let aside = self.path(&format!("{INDEX_FILE}.corrupt"));
            std::fs::rename(&index_path, &aside).map_err(|source| StateError::Io {
                action: "move aside",
                path: index_path.clone(),
                source,
            })?;
        }
        let ids = self.task_ids()?;
        let highest = ids
            .iter()
            .filter_map(|id| id.strip_prefix("task-")?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        if self.read_json::<SessionIndex>(INDEX_FILE)?.is_none() {
            self.write_json(
                INDEX_FILE,
                &SessionIndex {
                    next_task: highest,
                    ..SessionIndex::default()
                },
            )?;
        }
        for id in &ids {
            self.task(id)?.rebuild_cache()?;
        }
        Ok(ids)
    }

    /// The decisions recorded for a task, in order.
    pub fn task_decisions(&self, id: &str) -> Result<Vec<DecisionRecord>, StateError> {
        self.task(id)?.decisions()
    }

    /// The task bound to a session, if there is one. Unlike [`StateDir::bind_session`], this never
    /// creates a task.
    pub fn session_task(&self, session_id: &str) -> Result<Option<String>, StateError> {
        validate_session_id(session_id)?;
        let index: SessionIndex = self.read_json(INDEX_FILE)?.unwrap_or_default();
        Ok(index.sessions.get(session_id).cloned())
    }
}

impl Task<'_> {
    /// The id of the task.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The journal as it stands: the cache, plus the entries the cache does not have yet. Takes the
    /// task lock, so the result is consistent with the last append.
    pub fn snapshot(&self) -> Result<Journal, StateError> {
        let _guard = self.lock()?;
        self.load_unlocked()
    }

    /// The journal as [`Task::snapshot`] reads it, without the task lock and without creating
    /// anything. For viewers that must not write. An append in flight may not be included yet.
    pub fn peek(&self) -> Result<Journal, StateError> {
        self.load_unlocked()
    }

    /// Appends one record under the task lock, folds it, and writes the cache. Returns the journal
    /// after the append.
    pub fn append(&self, record: Record) -> Result<Journal, StateError> {
        let _guard = self.lock()?;
        let mut journal = self.load_unlocked()?;
        let entry = Entry {
            seq: journal.last_seq + 1,
            record,
        };
        self.state
            .append_unlocked(&self.file(JOURNAL_FILE), &entry)?;
        journal.apply(&entry);
        self.state.write_json(&self.file(CACHE_FILE), &journal)?;
        Ok(journal)
    }

    /// Records a decision under the task lock.
    pub fn record_decision(&self, decision: &DecisionRecord) -> Result<(), StateError> {
        let _guard = self.lock()?;
        self.state
            .append_unlocked(&self.file(DECISIONS_FILE), decision)
    }

    /// Rebuilds the cache from the journal and writes it. Used by repair.
    pub fn rebuild_cache(&self) -> Result<Journal, StateError> {
        let _guard = self.lock()?;
        let journal = self.load_unlocked()?;
        self.state.write_json(&self.file(CACHE_FILE), &journal)?;
        Ok(journal)
    }

    /// The decisions of this task, in order.
    pub fn decisions(&self) -> Result<Vec<DecisionRecord>, StateError> {
        Ok(self
            .state
            .read_records::<DecisionRecord>(&self.file(DECISIONS_FILE))?
            .records)
    }

    /// The journal entries, in order. Used by `agent trace` and by repair.
    pub fn entries(&self) -> Result<Vec<Entry>, StateError> {
        Ok(self
            .state
            .read_records::<Entry>(&self.file(JOURNAL_FILE))?
            .records)
    }

    fn load_unlocked(&self) -> Result<Journal, StateError> {
        let cached: Option<Journal> = self.state.read_json(&self.file(CACHE_FILE))?;
        let mut journal = cached.unwrap_or_else(Journal::empty);
        let read = self.state.read_records::<Entry>(&self.file(JOURNAL_FILE))?;
        for entry in &read.records {
            if entry.seq > journal.last_seq {
                journal.apply(entry);
            }
        }
        Ok(journal)
    }

    fn lock(&self) -> Result<FileLock, StateError> {
        let dir = task_dir(self.state, &self.id);
        std::fs::create_dir_all(&dir).map_err(|source| StateError::Io {
            action: "create",
            path: dir.clone(),
            source,
        })?;
        FileLock::acquire(&dir.join(TASK_LOCK), LOCK_TIMEOUT)
    }

    fn file(&self, name: &str) -> String {
        format!("{TASKS_DIR}/{}/{name}", self.id)
    }
}

/// Checks a task id: `task-` and four digits.
pub fn validate_task_id(id: &str) -> Result<(), StateError> {
    let valid = id
        .strip_prefix("task-")
        .is_some_and(|digits| digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_digit()));
    if valid {
        Ok(())
    } else {
        Err(StateError::InvalidId {
            what: "task id",
            value: id.to_owned(),
            rule: "the form is task-NNNN",
        })
    }
}

/// Checks a session id: letters, digits, `-` and `_`, at most [`MAX_SESSION_ID`] characters.
pub fn validate_session_id(id: &str) -> Result<(), StateError> {
    let valid = !id.is_empty()
        && id.len() <= MAX_SESSION_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(())
    } else {
        Err(StateError::InvalidId {
            what: "session id",
            value: id.to_owned(),
            rule: "use letters, digits, '-' and '_', at most 128 characters",
        })
    }
}

/// The path of a task directory under a state directory. Exposed for the repair and the tests.
pub fn task_dir(state: &StateDir, id: &str) -> PathBuf {
    state.root().join(TASKS_DIR).join(id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn task_ids_must_have_the_harness_form() {
        assert!(validate_task_id("task-0001").is_ok());
        for bad in [
            "task-1",
            "task-00001",
            "../task-0001",
            "TASK-0001",
            "task-000a",
        ] {
            assert!(validate_task_id(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn session_ids_are_a_strict_allowlist() {
        assert!(validate_session_id("ses_abc-123").is_ok());
        for bad in ["", "../etc", "a/b", "a b", "a.b", &"x".repeat(129)] {
            assert!(
                validate_session_id(bad).is_err(),
                "{bad:?} should be refused"
            );
        }
    }
}
