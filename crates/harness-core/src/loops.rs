//! Loop detection: a pure function over the recent calls of a task and its state.
//!
//! The detector reports signals. It does not refuse calls, and it does not decide what happens next:
//! that is the decision engine's job (step 5). Every rule is a function of the ring of recent calls and
//! the task state, so the same input always gives the same signals.
//!
//! Two rules use the defaults; the other thresholds are proposals and are tuned from recorded
//! trajectories. All thresholds are plain values here. Config loading and validation are added in
//! step 5.

use serde::{Deserialize, Serialize};

use crate::config::{AgentSection, WorkflowSection};
use crate::task::TaskState;

/// One call in the ring of recent calls. The ring holds the last [`RING_SIZE`] calls of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRecord {
    pub call_id: String,
    pub tool: String,
    /// Read-only calls never count toward a repeated-command loop.
    pub read_only: bool,
    /// Verification calls (tests, lint, type checks) are counted by the same-failure rule instead.
    pub verification: bool,
    /// Hash of the normalised command, for shell commands.
    pub command_hash: Option<String>,
    /// The file an edit changed, for edit calls.
    pub edited_file: Option<String>,
    /// The fingerprints of the failures a verification call reported. Empty when it passed.
    pub fingerprints: Vec<String>,
    /// Whether a verification call passed. `None` for calls that are not verifications.
    pub passed: Option<bool>,
}

/// How many recent calls are kept per task (proposed).
pub const RING_SIZE: usize = 16;

/// The thresholds of the loop rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thresholds {
    /// Consecutive verifications with the same failure before ESCALATE (two retries).
    pub same_failure_verifications: u32,
    /// IMPLEMENT entries beyond which the task escalates (three retries).
    pub implementation_retries: u32,
    /// Identical commands in the window, with no edit between, that count as a loop. Proposed.
    pub repeated_command_count: u32,
    /// The window, in calls, in which identical commands are counted. Proposed.
    pub repeated_command_window: u32,
    /// Edits of one file since the last verification that count as a loop. Proposed.
    pub repeated_edit_count: u32,
    /// Iterations after which the task is blocked. The value is the starting value in `[agent]`.
    pub max_iterations: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self::from_config(&AgentSection::default(), &WorkflowSection::default())
    }
}

impl Thresholds {
    /// The thresholds from the configuration. `[agent]` holds the bounds; `[workflow]` holds the
    /// thresholds. Same-failure retries are configured as retries, so the rule fires on the
    /// verification after the last retry.
    pub fn from_config(agent: &AgentSection, workflow: &WorkflowSection) -> Self {
        Self {
            same_failure_verifications: agent.max_same_failure.saturating_add(1),
            implementation_retries: agent.max_implementation_retries,
            repeated_command_count: workflow.repeated_command_count,
            repeated_command_window: workflow.repeated_command_window,
            repeated_edit_count: workflow.repeated_edit_count,
            max_iterations: agent.max_iterations,
        }
    }
}

/// A loop rule that fired. Each variant carries the evidence the decision engine reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum LoopSignal {
    /// The same command ran again and again with no edit in between.
    RepeatedCommand { command_hash: String, count: u32 },
    /// The same failure came back in consecutive verifications.
    SameFailure { fingerprint: String, count: u32 },
    /// One file was edited again and again since the last verification.
    RepeatedEdit { file: String, count: u32 },
    /// The task has reached its iteration limit.
    IterationLimit { iteration: u32, limit: u32 },
    /// IMPLEMENT was entered more often than the retry limit allows.
    ImplementationRetries { entries: u32, limit: u32 },
}

/// Runs every loop rule over the ring and the task state. Signals come back in a fixed order.
pub fn detect(ring: &[CallRecord], task: &TaskState, thresholds: &Thresholds) -> Vec<LoopSignal> {
    let mut signals = Vec::new();
    if let Some(signal) = repeated_command(ring, thresholds) {
        signals.push(signal);
    }
    if let Some(signal) = same_failure(ring, thresholds) {
        signals.push(signal);
    }
    if let Some(signal) = repeated_edit(ring, thresholds) {
        signals.push(signal);
    }
    if task.iteration >= thresholds.max_iterations {
        signals.push(LoopSignal::IterationLimit {
            iteration: task.iteration,
            limit: thresholds.max_iterations,
        });
    }
    if task.iteration > thresholds.implementation_retries {
        signals.push(LoopSignal::ImplementationRetries {
            entries: task.iteration,
            limit: thresholds.implementation_retries,
        });
    }
    signals
}

/// The same command, with no edit between, several times within the window.
fn repeated_command(ring: &[CallRecord], thresholds: &Thresholds) -> Option<LoopSignal> {
    let window = usize::try_from(thresholds.repeated_command_window).ok()?;
    let recent: Vec<&CallRecord> = ring.iter().rev().take(window).collect();
    // Only the calls after the most recent edit count: an edit is progress.
    let since_edit: Vec<&CallRecord> = recent
        .iter()
        .copied()
        .take_while(|call| call.edited_file.is_none())
        .collect();
    let last = since_edit
        .iter()
        .find(|call| !call.read_only && !call.verification && call.command_hash.is_some())?;
    let command_hash = last.command_hash.clone()?;
    let count = since_edit
        .iter()
        .filter(|call| {
            !call.read_only
                && !call.verification
                && call.command_hash.as_deref() == Some(&command_hash)
        })
        .count();
    let count = u32::try_from(count).ok()?;
    (count >= thresholds.repeated_command_count).then_some(LoopSignal::RepeatedCommand {
        command_hash,
        count,
    })
}

/// The same failure in the last consecutive verifications, with no pass between them. The count is every
/// consecutive verification that reports the failure, so it keeps growing after the rule first fires.
fn same_failure(ring: &[CallRecord], thresholds: &Thresholds) -> Option<LoopSignal> {
    let needed = usize::try_from(thresholds.same_failure_verifications).ok()?;
    let failing: Vec<&CallRecord> = ring
        .iter()
        .rev()
        .filter(|call| call.verification)
        .take_while(|call| call.passed != Some(true))
        .collect();
    if failing.len() < needed {
        return None;
    }
    let first = failing.first()?;
    let shared = first.fingerprints.iter().find(|fp| {
        failing[..needed]
            .iter()
            .all(|call| call.fingerprints.contains(fp))
    })?;
    let count = failing
        .iter()
        .take_while(|call| call.fingerprints.contains(shared))
        .count();
    Some(LoopSignal::SameFailure {
        fingerprint: shared.clone(),
        count: u32::try_from(count).ok()?,
    })
}

/// One file edited again and again since the last verification.
fn repeated_edit(ring: &[CallRecord], thresholds: &Thresholds) -> Option<LoopSignal> {
    let since_verification: Vec<&CallRecord> = ring
        .iter()
        .rev()
        .take_while(|call| !call.verification)
        .collect();
    let mut counts: Vec<(String, u32)> = Vec::new();
    for call in since_verification {
        let Some(file) = &call.edited_file else {
            continue;
        };
        match counts.iter_mut().find(|(name, _)| name == file) {
            Some((_, count)) => *count += 1,
            None => counts.push((file.clone(), 1)),
        }
    }
    counts
        .into_iter()
        .find(|(_, count)| *count >= thresholds.repeated_edit_count)
        .map(|(file, count)| LoopSignal::RepeatedEdit { file, count })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn call(id: &str) -> CallRecord {
        CallRecord {
            call_id: id.to_owned(),
            tool: "bash".to_owned(),
            read_only: false,
            verification: false,
            command_hash: None,
            edited_file: None,
            fingerprints: Vec::new(),
            passed: None,
        }
    }

    fn command(id: &str, hash: &str) -> CallRecord {
        CallRecord {
            command_hash: Some(hash.to_owned()),
            ..call(id)
        }
    }

    fn edit(id: &str, file: &str) -> CallRecord {
        CallRecord {
            edited_file: Some(file.to_owned()),
            ..call(id)
        }
    }

    fn verification(id: &str, fingerprints: &[&str]) -> CallRecord {
        CallRecord {
            verification: true,
            passed: Some(fingerprints.is_empty()),
            fingerprints: fingerprints.iter().map(|f| (*f).to_owned()).collect(),
            ..call(id)
        }
    }

    fn task() -> TaskState {
        TaskState::new("task-1", "repo", "fix the test")
    }

    fn thresholds() -> Thresholds {
        Thresholds::default()
    }

    #[test]
    fn an_empty_ring_gives_no_signals() {
        assert!(detect(&[], &task(), &thresholds()).is_empty());
    }

    #[test]
    fn three_identical_commands_with_no_edit_are_a_loop() {
        let ring = vec![command("1", "h"), command("2", "h"), command("3", "h")];
        let signals = detect(&ring, &task(), &thresholds());
        assert_eq!(
            signals,
            vec![LoopSignal::RepeatedCommand {
                command_hash: "h".to_owned(),
                count: 3
            }]
        );
    }

    #[test]
    fn an_edit_between_identical_commands_is_progress() {
        let ring = vec![
            command("1", "h"),
            edit("2", "calc.py"),
            command("3", "h"),
            command("4", "h"),
        ];
        assert!(detect(&ring, &task(), &thresholds()).is_empty());
    }

    #[test]
    fn repeated_read_only_calls_are_not_a_loop() {
        let read = CallRecord {
            read_only: true,
            ..command("x", "g")
        };
        let ring = vec![read.clone(), read.clone(), read.clone()];
        assert!(detect(&ring, &task(), &thresholds()).is_empty());
    }

    #[test]
    fn repeated_verifications_are_left_to_the_same_failure_rule() {
        let ring = vec![
            CallRecord {
                verification: true,
                ..command("1", "t")
            },
            CallRecord {
                verification: true,
                ..command("2", "t")
            },
            CallRecord {
                verification: true,
                ..command("3", "t")
            },
        ];
        let signals = detect(&ring, &task(), &thresholds());
        assert!(
            !signals
                .iter()
                .any(|s| matches!(s, LoopSignal::RepeatedCommand { .. }))
        );
    }

    #[test]
    fn the_same_failure_in_three_verifications_is_a_loop() {
        let ring = vec![
            verification("1", &["fp_v1:aaaa"]),
            verification("2", &["fp_v1:aaaa", "fp_v1:bbbb"]),
            verification("3", &["fp_v1:aaaa"]),
        ];
        let signals = detect(&ring, &task(), &thresholds());
        assert_eq!(
            signals,
            vec![LoopSignal::SameFailure {
                fingerprint: "fp_v1:aaaa".to_owned(),
                count: 3
            }]
        );
    }

    #[test]
    fn the_same_failure_count_keeps_growing_after_the_rule_fires() {
        let ring = vec![
            verification("1", &["fp_v1:aaaa"]),
            verification("2", &["fp_v1:aaaa"]),
            verification("3", &["fp_v1:aaaa"]),
            verification("4", &["fp_v1:aaaa"]),
        ];
        assert_eq!(
            detect(&ring, &task(), &thresholds()),
            vec![LoopSignal::SameFailure {
                fingerprint: "fp_v1:aaaa".to_owned(),
                count: 4
            }]
        );
    }

    #[test]
    fn a_pass_between_failures_breaks_the_same_failure_run() {
        let ring = vec![
            verification("1", &["fp_v1:aaaa"]),
            verification("2", &[]),
            verification("3", &["fp_v1:aaaa"]),
            verification("4", &["fp_v1:aaaa"]),
        ];
        assert!(detect(&ring, &task(), &thresholds()).is_empty());
    }

    #[test]
    fn a_different_failure_each_time_is_not_the_same_failure() {
        let ring = vec![
            verification("1", &["fp_v1:aaaa"]),
            verification("2", &["fp_v1:bbbb"]),
            verification("3", &["fp_v1:cccc"]),
        ];
        assert!(detect(&ring, &task(), &thresholds()).is_empty());
    }

    #[test]
    fn four_edits_of_one_file_since_the_last_verification_are_a_loop() {
        let ring = vec![
            verification("0", &["fp_v1:aaaa"]),
            edit("1", "calc.py"),
            edit("2", "calc.py"),
            edit("3", "calc.py"),
            edit("4", "calc.py"),
        ];
        let signals = detect(&ring, &task(), &thresholds());
        assert_eq!(
            signals,
            vec![LoopSignal::RepeatedEdit {
                file: "calc.py".to_owned(),
                count: 4
            }]
        );
    }

    #[test]
    fn edits_of_different_files_are_not_a_loop() {
        let ring = vec![
            edit("1", "a.py"),
            edit("2", "b.py"),
            edit("3", "c.py"),
            edit("4", "d.py"),
        ];
        assert!(detect(&ring, &task(), &thresholds()).is_empty());
    }

    #[test]
    fn reaching_the_iteration_limit_blocks_the_task() {
        let mut state = task();
        let limit = Thresholds::default().max_iterations;
        state.iteration = limit;
        let signals = detect(&[], &state, &thresholds());
        assert!(
            signals.contains(&LoopSignal::IterationLimit {
                iteration: limit,
                limit
            }),
            "{signals:?}"
        );
    }

    #[test]
    fn implement_entries_beyond_the_retry_limit_escalate() {
        let mut state = task();
        state.iteration = 3;
        assert!(detect(&[], &state, &thresholds()).is_empty());
        state.iteration = 4;
        assert_eq!(
            detect(&[], &state, &thresholds()),
            vec![LoopSignal::ImplementationRetries {
                entries: 4,
                limit: 3
            }]
        );
    }

    #[test]
    fn detection_is_deterministic() {
        let ring = vec![command("1", "h"), command("2", "h"), command("3", "h")];
        assert_eq!(
            detect(&ring, &task(), &thresholds()),
            detect(&ring, &task(), &thresholds())
        );
    }
}
