//! The task model of the orchestrator (M3 step 2): phases, the transition table and `TaskState`.
//!
//! Pure data and rules. Nothing here reads a file or a clock. The state store in `harness-runtime`
//! persists these values, and the loop detector and the decision engine read them.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The workflow phases of R-17.1. A task is always in exactly one phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Phase {
    Start,
    Discover,
    Plan,
    Implement,
    Verify,
    Review,
    Done,
    Fail,
    Diagnose,
    Explore,
    Escalate,
}

impl Phase {
    /// Every phase, in declaration order. Used by the exhaustive transition test.
    #[cfg(test)]
    pub const ALL: [Phase; 11] = [
        Phase::Start,
        Phase::Discover,
        Phase::Plan,
        Phase::Implement,
        Phase::Verify,
        Phase::Review,
        Phase::Done,
        Phase::Fail,
        Phase::Diagnose,
        Phase::Explore,
        Phase::Escalate,
    ];

    /// The name used in logs and in the controlled vocabulary (R-10.1).
    pub fn name(self) -> &'static str {
        match self {
            Phase::Start => "START",
            Phase::Discover => "DISCOVER",
            Phase::Plan => "PLAN",
            Phase::Implement => "IMPLEMENT",
            Phase::Verify => "VERIFY",
            Phase::Review => "REVIEW",
            Phase::Done => "DONE",
            Phase::Fail => "FAIL",
            Phase::Diagnose => "DIAGNOSE",
            Phase::Explore => "EXPLORE",
            Phase::Escalate => "ESCALATE",
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The legal moves between phases. A move to the same phase is not a transition and is always
/// allowed (see [`transition`]). The table follows R-17.1; the comment on each row says why it exists.
pub const TRANSITIONS: &[(Phase, Phase)] = &[
    (Phase::Start, Phase::Discover),
    (Phase::Discover, Phase::Plan),
    // The model went straight to an edit without a plan.
    (Phase::Discover, Phase::Implement),
    (Phase::Plan, Phase::Implement),
    (Phase::Implement, Phase::Verify),
    // Implementation retries are exhausted (R-17.3).
    (Phase::Implement, Phase::Escalate),
    // Verification passed (R-17.1 PASS).
    // An edit after a run whose outcome was unknown, or before its result was recorded.
    (Phase::Verify, Phase::Implement),
    (Phase::Verify, Phase::Review),
    // Verification failed (R-17.1 FAIL).
    (Phase::Verify, Phase::Fail),
    // The same failure has repeated beyond its retries (R-17.3).
    (Phase::Verify, Phase::Escalate),
    (Phase::Review, Phase::Done),
    // A review asks for changes.
    (Phase::Review, Phase::Implement),
    (Phase::Fail, Phase::Diagnose),
    // Recoverable failure (R-17.1).
    (Phase::Diagnose, Phase::Implement),
    // Unclear failure (R-17.1).
    (Phase::Diagnose, Phase::Explore),
    // Repeated failure (R-17.1).
    (Phase::Diagnose, Phase::Escalate),
    (Phase::Explore, Phase::Plan),
    (Phase::Explore, Phase::Implement),
    (Phase::Explore, Phase::Escalate),
    // Resumed after a user decision or `agent task reset`.
    (Phase::Escalate, Phase::Implement),
    (Phase::Escalate, Phase::Plan),
];

/// A phase move that the table does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("illegal phase move {from} -> {to}")]
pub struct TransitionError {
    pub from: Phase,
    pub to: Phase,
}

/// Checks a move from `from` to `to`. Returns the new phase, or the illegal move.
pub fn transition(from: Phase, to: Phase) -> Result<Phase, TransitionError> {
    if from == to || TRANSITIONS.contains(&(from, to)) {
        Ok(to)
    } else {
        Err(TransitionError { from, to })
    }
}

/// The shortest sequence of legal moves from `from` to `to`, not including `from`. Empty when the phases
/// are equal; `None` when the table has no way from one to the other.
pub fn path(from: Phase, to: Phase) -> Option<Vec<Phase>> {
    if from == to {
        return Some(Vec::new());
    }
    let mut previous: HashMap<Phase, Phase> = HashMap::new();
    let mut seen: HashSet<Phase> = HashSet::from([from]);
    let mut queue = VecDeque::from([from]);
    while let Some(phase) = queue.pop_front() {
        for &(a, b) in TRANSITIONS {
            if a != phase || !seen.insert(b) {
                continue;
            }
            previous.insert(b, a);
            if b == to {
                let mut steps = vec![to];
                let mut at = to;
                while let Some(&before) = previous.get(&at) {
                    if before == from {
                        break;
                    }
                    steps.push(before);
                    at = before;
                }
                steps.reverse();
                return Some(steps);
            }
            queue.push_back(b);
        }
    }
    None
}

/// Whether a task is still running, waiting for a person, or finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskStatus {
    /// Running. The phase says what the task is doing.
    Active,
    /// Refused by a rule, such as the iteration limit. A person or a reset must change it.
    Blocked,
    /// Finished by reaching DONE.
    Completed,
    /// Ended without success.
    Aborted,
}

/// The state of one task (R-16.4). Every field is set when the task starts, and the orchestrator
/// updates it from observed events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskState {
    pub task_id: String,
    pub repository: String,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub original_prompt: String,
    pub current_objective: String,
    pub phase: Phase,
    pub iteration: u32,
    pub active_model: String,
    pub last_action: Option<String>,
    pub last_test: Option<String>,
    pub failure_class: Option<String>,
    pub failure_count: u32,
    pub escalation_count: u32,
    pub context_version: u32,
    pub status: TaskStatus,
    /// Loop refusals since the last call that was not refused. Reset by a successful call and by
    /// `agent task reset`. Serde default keeps files written before this field readable.
    #[serde(default)]
    pub consecutive_refusals: u32,
}

impl TaskState {
    /// A new task in phase START, with no history.
    pub fn new(
        task_id: impl Into<String>,
        repository: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        let prompt = prompt.into();
        Self {
            task_id: task_id.into(),
            repository: repository.into(),
            branch: None,
            worktree: None,
            original_prompt: prompt.clone(),
            current_objective: prompt,
            phase: Phase::Start,
            iteration: 0,
            active_model: "main".to_owned(),
            last_action: None,
            last_test: None,
            failure_class: None,
            failure_count: 0,
            escalation_count: 0,
            context_version: 0,
            status: TaskStatus::Active,
            consecutive_refusals: 0,
        }
    }

    /// Moves to `to` along the shortest legal path. An observation can imply phases that were passed through
    /// without being seen, such as DISCOVER and PLAN before a first edit. Unreachable targets leave the
    /// state unchanged.
    pub fn advance(&mut self, to: Phase) {
        if let Some(steps) = path(self.phase, to) {
            for step in steps {
                let _ = self.move_to(step);
            }
        }
    }

    /// Moves to `to`, or reports the illegal move and leaves the state unchanged. Entering IMPLEMENT
    /// from another phase starts a new iteration, which is what the iteration limit counts.
    pub fn move_to(&mut self, to: Phase) -> Result<(), TransitionError> {
        let next = transition(self.phase, to)?;
        if next != self.phase {
            if next == Phase::Implement {
                self.iteration += 1;
            }
            self.phase = next;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn every_pair_of_phases_is_allowed_exactly_when_the_table_says() {
        for from in Phase::ALL {
            for to in Phase::ALL {
                let listed = TRANSITIONS.contains(&(from, to));
                let result = transition(from, to);
                if from == to {
                    assert_eq!(
                        result,
                        Ok(to),
                        "{from:?} -> {to:?} is not a transition but must be allowed"
                    );
                } else if listed {
                    assert_eq!(
                        result,
                        Ok(to),
                        "{from:?} -> {to:?} is listed and must be allowed"
                    );
                } else {
                    assert_eq!(
                        result,
                        Err(TransitionError { from, to }),
                        "{from:?} -> {to:?} is not listed and must be refused"
                    );
                }
            }
        }
    }

    #[test]
    fn advance_follows_the_shortest_legal_path() {
        let mut task = TaskState::new("task-1", "repo", "p");
        task.advance(Phase::Implement);
        assert_eq!((task.phase, task.iteration), (Phase::Implement, 1));
        assert_eq!(
            path(Phase::Start, Phase::Implement),
            Some(vec![Phase::Discover, Phase::Implement])
        );
        assert_eq!(
            path(Phase::Done, Phase::Start),
            None,
            "DONE has no move back"
        );
    }

    #[test]
    fn every_phase_is_reachable_from_start() {
        let mut reached = vec![Phase::Start];
        let mut frontier = vec![Phase::Start];
        while let Some(phase) = frontier.pop() {
            for &(from, to) in TRANSITIONS {
                if from == phase && !reached.contains(&to) {
                    reached.push(to);
                    frontier.push(to);
                }
            }
        }
        for phase in Phase::ALL {
            assert!(reached.contains(&phase), "{phase:?} is unreachable");
        }
    }

    #[test]
    fn the_table_has_no_duplicate_rows() {
        for (index, row) in TRANSITIONS.iter().enumerate() {
            assert!(!TRANSITIONS[..index].contains(row), "duplicate row {row:?}");
        }
    }

    #[test]
    fn a_new_task_starts_in_start_and_active() {
        let task = TaskState::new("task-1", "repo", "fix the test");
        assert_eq!(task.phase, Phase::Start);
        assert_eq!(task.status, TaskStatus::Active);
        assert_eq!(task.current_objective, "fix the test");
        assert_eq!(task.iteration, 0);
    }

    #[test]
    fn an_illegal_move_leaves_the_task_unchanged() {
        let mut task = TaskState::new("task-1", "repo", "p");
        let before = task.clone();
        assert!(task.move_to(Phase::Done).is_err());
        assert_eq!(task, before);
    }

    #[test]
    fn entering_implement_counts_one_iteration() {
        let mut task = TaskState::new("task-1", "repo", "p");
        task.move_to(Phase::Discover).unwrap();
        task.move_to(Phase::Plan).unwrap();
        assert_eq!((task.phase, task.iteration), (Phase::Plan, 0));
        task.move_to(Phase::Implement).unwrap();
        task.move_to(Phase::Implement).unwrap();
        assert_eq!((task.phase, task.iteration), (Phase::Implement, 1));
    }

    #[test]
    fn phase_names_match_the_vocabulary() {
        assert_eq!(Phase::Escalate.name(), "ESCALATE");
        assert_eq!(Phase::Diagnose.name(), "DIAGNOSE");
    }
}
