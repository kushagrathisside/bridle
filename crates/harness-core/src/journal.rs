//! The task journal (M3 step 6): the records that describe one task, and the pure fold that turns them
//! into [`TaskState`] and the ring of recent calls.
//!
//! The journal is the source of truth. Each record carries a per-task `seq`, and the fold applies records
//! in `seq` order. Nothing here reads or writes a file; the runtime appends records and keeps the cache.

use serde::{Deserialize, Serialize};

use crate::loops::{CallRecord, RING_SIZE};
use crate::task::{Phase, TaskState, TaskStatus};

/// One fact about a task. The variants are what the observers can see: the hooks, the MCP server and
/// the task commands (see the observation table in the M3 plan).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The first record of a task. It fixes the task id, the repository and the prompt.
    TaskStarted {
        task_id: String,
        repository: String,
        prompt: String,
    },
    /// A call is about to run and the policy allowed it.
    CallStarted {
        call_id: String,
        tool: String,
        read_only: bool,
        verification: bool,
        command_hash: Option<String>,
        /// The file the call edits, for edit tools.
        edited_file: Option<String>,
    },
    /// A verification call ended. `fingerprints` lists the failures it reported; `outcome` is never
    /// `Passed` unless a runner reported success.
    VerificationResult {
        call_id: String,
        outcome: Outcome,
        fingerprints: Vec<String>,
    },
    /// A call was refused by a loop rule. It did not run. The iteration limit also blocks the task.
    Refused { call_id: String, rule: String },
    /// A verification failed and the loop rules sent the case to the deep slot. The task counts the escalation, and
    /// the answer applies to the calls after it until a verification passes or the task is reset.
    Escalated { call_id: String },
    /// A person ended the block with `agent task reset`. The loop counters start again.
    Reset { reason: String },
    /// The task runs in its own Git worktree. `base_commit` is where it starts; `dirty_files` counts the
    /// changes that were already in the repository, taken before the first mutation (R-49.1, R-49.2).
    Worktree {
        branch: String,
        path: String,
        base_commit: String,
        dirty_files: u32,
    },
}

/// What a verification call reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    /// The output could not tell. Never treated as a pass.
    Unknown,
}

/// A record with its position in the task's journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    pub record: Record,
}

/// The state rebuilt from the journal: the task, the ring of recent calls, and the last `seq` applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub state: TaskState,
    pub ring: Vec<CallRecord>,
    pub last_seq: u64,
}

impl Journal {
    /// A journal with no task yet. The first `TaskStarted` entry replaces the placeholder state.
    pub fn empty() -> Self {
        Self::new(TaskState::new("", "", ""))
    }

    /// An empty journal for a new task.
    pub fn new(state: TaskState) -> Self {
        Self {
            state,
            ring: Vec::new(),
            last_seq: 0,
        }
    }

    /// Applies one entry. Entries must arrive in `seq` order; an entry whose `seq` is not the next one
    /// is ignored, so a replay of entries already applied is harmless.
    pub fn apply(&mut self, entry: &Entry) {
        if entry.seq != self.last_seq + 1 {
            return;
        }
        match &entry.record {
            Record::TaskStarted {
                task_id,
                repository,
                prompt,
            } => {
                self.state = TaskState::new(task_id.clone(), repository.clone(), prompt.clone());
            }
            Record::CallStarted {
                call_id,
                tool,
                read_only,
                verification,
                command_hash,
                edited_file,
            } => {
                if let Some(file) = edited_file {
                    advance(&mut self.state, Phase::Implement);
                    self.state.last_action = Some(format!("edit {file}"));
                } else if *read_only {
                    advance(&mut self.state, Phase::Discover);
                    self.state.last_action = Some(tool.clone());
                } else {
                    self.state.last_action = Some(tool.clone());
                }
                if *verification {
                    advance(&mut self.state, Phase::Verify);
                    self.state.last_test = Some(call_id.clone());
                }
                // Only progress ends a streak of refusals: an edit. Other calls do not, so a read or a
                // retry cannot reset the cap.
                if edited_file.is_some() {
                    self.state.consecutive_refusals = 0;
                }
                self.ring.push(CallRecord {
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    read_only: *read_only,
                    verification: *verification,
                    command_hash: command_hash.clone(),
                    edited_file: edited_file.clone(),
                    fingerprints: Vec::new(),
                    passed: None,
                });
                let excess = self.ring.len().saturating_sub(RING_SIZE);
                self.ring.drain(..excess);
            }
            Record::VerificationResult {
                call_id,
                outcome,
                fingerprints,
            } => {
                if let Some(call) = self.ring.iter_mut().rev().find(|c| &c.call_id == call_id) {
                    call.fingerprints = fingerprints.clone();
                    call.passed = match outcome {
                        Outcome::Passed => Some(true),
                        Outcome::Failed => Some(false),
                        Outcome::Unknown => None,
                    };
                }
                match outcome {
                    Outcome::Passed => {
                        advance(&mut self.state, Phase::Review);
                        self.state.consecutive_refusals = 0;
                        self.state.failure_count = 0;
                        self.state.failure_class = None;
                        self.state.escalation_open = false;
                    }
                    Outcome::Failed => {
                        advance(&mut self.state, Phase::Fail);
                        advance(&mut self.state, Phase::Diagnose);
                        self.state.failure_count += 1;
                        self.state.failure_class = Some("test_failure".to_owned());
                    }
                    Outcome::Unknown => {}
                }
            }
            Record::Refused { rule, .. } => {
                self.state.consecutive_refusals += 1;
                if rule == "iteration_limit" {
                    self.state.status = TaskStatus::Blocked;
                }
            }
            Record::Worktree { branch, path, .. } => {
                self.state.branch = Some(branch.clone());
                self.state.worktree = Some(path.clone());
            }
            Record::Escalated { .. } => {
                self.state.escalation_count = self.state.escalation_count.saturating_add(1);
                self.state.escalation_open = true;
            }
            Record::Reset { .. } => {
                self.state.status = TaskStatus::Active;
                self.state.consecutive_refusals = 0;
                self.state.iteration = 0;
                self.state.failure_count = 0;
                self.state.escalation_open = false;
                self.ring.clear();
            }
        }
        self.last_seq = entry.seq;
    }

    /// Rebuilds a journal from entries, applied in order.
    pub fn replay<'a>(state: TaskState, entries: impl IntoIterator<Item = &'a Entry>) -> Self {
        let mut journal = Self::new(state);
        for entry in entries {
            journal.apply(entry);
        }
        journal
    }
}

/// Moves the task to `to` along a legal path. Phase inference uses only legal moves.
fn advance(state: &mut TaskState, to: Phase) {
    state.advance(to);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn task() -> TaskState {
        TaskState::new("task-1", "repo", "fix the test")
    }

    fn entry(seq: u64, record: Record) -> Entry {
        Entry { seq, record }
    }

    fn started(id: &str) -> Record {
        Record::CallStarted {
            call_id: id.to_owned(),
            tool: "bash".to_owned(),
            read_only: false,
            verification: false,
            command_hash: Some("h".to_owned()),
            edited_file: None,
        }
    }

    fn edit(id: &str, file: &str) -> Record {
        Record::CallStarted {
            call_id: id.to_owned(),
            tool: "edit".to_owned(),
            read_only: false,
            verification: false,
            command_hash: None,
            edited_file: Some(file.to_owned()),
        }
    }

    fn test_run(id: &str) -> Record {
        Record::CallStarted {
            call_id: id.to_owned(),
            tool: "bash".to_owned(),
            read_only: false,
            verification: true,
            command_hash: Some("t".to_owned()),
            edited_file: None,
        }
    }

    #[test]
    fn an_edit_moves_the_task_to_implement() {
        let mut journal = Journal::new(task());
        journal.apply(&entry(1, edit("c1", "calc.py")));
        assert_eq!(journal.state.phase, Phase::Implement);
        assert_eq!(journal.ring.len(), 1);
        assert_eq!(journal.last_seq, 1);
    }

    #[test]
    fn a_failed_test_run_goes_through_fail_to_diagnose() {
        let mut journal = Journal::new(task());
        journal.apply(&entry(1, edit("c1", "calc.py")));
        journal.apply(&entry(2, test_run("c2")));
        journal.apply(&entry(
            3,
            Record::VerificationResult {
                call_id: "c2".to_owned(),
                outcome: Outcome::Failed,
                fingerprints: vec!["fp_v1:aaaa".to_owned()],
            },
        ));
        assert_eq!(journal.state.phase, Phase::Diagnose);
        assert_eq!(journal.state.failure_count, 1);
        assert_eq!(
            journal.ring.last().unwrap().fingerprints,
            vec!["fp_v1:aaaa".to_owned()]
        );
    }

    #[test]
    fn a_passing_run_goes_to_review_and_clears_failures() {
        let mut journal = Journal::new(task());
        journal.apply(&entry(1, edit("c1", "calc.py")));
        journal.apply(&entry(2, test_run("c2")));
        journal.apply(&entry(
            3,
            Record::VerificationResult {
                call_id: "c2".to_owned(),
                outcome: Outcome::Passed,
                fingerprints: Vec::new(),
            },
        ));
        assert_eq!(journal.state.phase, Phase::Review);
        assert_eq!(journal.state.failure_count, 0);
    }

    #[test]
    fn an_unknown_outcome_is_never_a_pass() {
        let mut journal = Journal::new(task());
        journal.apply(&entry(1, edit("c1", "calc.py")));
        journal.apply(&entry(2, test_run("c2")));
        journal.apply(&entry(
            3,
            Record::VerificationResult {
                call_id: "c2".to_owned(),
                outcome: Outcome::Unknown,
                fingerprints: Vec::new(),
            },
        ));
        assert_eq!(journal.state.phase, Phase::Verify);
        assert_eq!(journal.ring.last().unwrap().passed, None);
    }

    #[test]
    fn the_first_entry_fixes_the_task_identity() {
        let entries = vec![
            entry(
                1,
                Record::TaskStarted {
                    task_id: "task-0001".to_owned(),
                    repository: "repo".to_owned(),
                    prompt: "fix it".to_owned(),
                },
            ),
            entry(2, started("c1")),
        ];
        let journal = Journal::replay(Journal::empty().state, &entries);
        assert_eq!(journal.state.task_id, "task-0001");
        assert_eq!(journal.state.original_prompt, "fix it");
        assert_eq!(journal.last_seq, 2);
    }

    #[test]
    fn an_out_of_order_entry_is_ignored() {
        let mut journal = Journal::new(task());
        journal.apply(&entry(2, started("c1")));
        assert_eq!(journal.last_seq, 0);
        assert!(journal.ring.is_empty());
    }

    #[test]
    fn replaying_the_same_entries_gives_the_same_journal() {
        let entries = vec![
            entry(1, started("c1")),
            entry(2, edit("c2", "a.py")),
            entry(3, started("c3")),
        ];
        let once = Journal::replay(task(), &entries);
        let again = Journal::replay(task(), &entries);
        assert_eq!(once, again);
        assert_eq!(once.last_seq, 3);
    }

    #[test]
    fn a_refusal_counts_and_only_an_edit_resets_the_count() {
        let mut journal = Journal::new(task());
        journal.apply(&entry(
            1,
            Record::Refused {
                call_id: "c1".to_owned(),
                rule: "repeated_command".to_owned(),
            },
        ));
        journal.apply(&entry(
            2,
            Record::Refused {
                call_id: "c2".to_owned(),
                rule: "repeated_command".to_owned(),
            },
        ));
        assert_eq!(journal.state.consecutive_refusals, 2);
        journal.apply(&entry(3, started("c3")));
        assert_eq!(
            journal.state.consecutive_refusals, 2,
            "a call that is not an edit does not reset it"
        );
        journal.apply(&entry(4, edit("c4", "calc.py")));
        assert_eq!(journal.state.consecutive_refusals, 0);
    }

    #[test]
    fn an_escalation_is_counted_and_stays_open_until_a_pass_or_a_reset() {
        let mut journal = Journal::new(task());
        journal.apply(&entry(1, started("c1")));
        journal.apply(&entry(
            2,
            Record::Escalated {
                call_id: "c1".to_owned(),
            },
        ));
        assert_eq!(journal.state.escalation_count, 1);
        assert!(journal.state.escalation_open);

        journal.apply(&entry(3, test_run("t1")));
        journal.apply(&entry(
            4,
            Record::VerificationResult {
                call_id: "t1".to_owned(),
                outcome: Outcome::Failed,
                fingerprints: vec!["fp_v1:aaaa".to_owned()],
            },
        ));
        assert!(
            journal.state.escalation_open,
            "a failure keeps the answer open"
        );

        journal.apply(&entry(5, test_run("t2")));
        journal.apply(&entry(
            6,
            Record::VerificationResult {
                call_id: "t2".to_owned(),
                outcome: Outcome::Passed,
                fingerprints: Vec::new(),
            },
        ));
        assert!(!journal.state.escalation_open, "a pass closes the answer");
        assert_eq!(journal.state.escalation_count, 1, "the count stays");

        journal.apply(&entry(
            7,
            Record::Escalated {
                call_id: "t2".to_owned(),
            },
        ));
        journal.apply(&entry(
            8,
            Record::Reset {
                reason: "test".to_owned(),
            },
        ));
        assert!(!journal.state.escalation_open, "a reset closes the answer");
        assert_eq!(journal.state.escalation_count, 2);
    }

    #[test]
    fn a_worktree_record_sets_the_branch_and_the_path() {
        let mut journal = Journal::new(task());
        journal.apply(&entry(
            1,
            Record::Worktree {
                branch: "agent-task-001".to_owned(),
                path: ".worktrees/agent-task-001".to_owned(),
                base_commit: "abc123".to_owned(),
                dirty_files: 0,
            },
        ));
        assert_eq!(journal.state.branch.as_deref(), Some("agent-task-001"));
        assert_eq!(
            journal.state.worktree.as_deref(),
            Some(".worktrees/agent-task-001")
        );
    }

    #[test]
    fn the_iteration_limit_blocks_and_a_reset_starts_the_counters_again() {
        let mut journal = Journal::new(task());
        journal.apply(&entry(1, started("c1")));
        journal.apply(&entry(
            2,
            Record::Refused {
                call_id: "c2".to_owned(),
                rule: "iteration_limit".to_owned(),
            },
        ));
        assert_eq!(journal.state.status, TaskStatus::Blocked);
        journal.apply(&entry(
            3,
            Record::Reset {
                reason: "user".to_owned(),
            },
        ));
        assert_eq!(journal.state.status, TaskStatus::Active);
        assert_eq!(journal.state.consecutive_refusals, 0);
        assert!(journal.ring.is_empty());
    }

    #[test]
    fn the_ring_keeps_only_the_most_recent_calls() {
        let entries: Vec<Entry> = (1..=(RING_SIZE as u64 + 5))
            .map(|seq| entry(seq, started(&format!("c{seq}"))))
            .collect();
        let journal = Journal::replay(task(), &entries);
        assert_eq!(journal.ring.len(), RING_SIZE);
        assert_eq!(journal.ring.first().unwrap().call_id, "c6");
    }
}
