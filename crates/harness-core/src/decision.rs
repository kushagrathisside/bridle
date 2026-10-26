//! Decisions for the next step (M3 step 5, R-9.2 to R-9.5, R-10).
//!
//! A [`DecisionEngine`] turns a task state and the loop signals into a [`DecisionResult`]. M3 has one
//! engine, [`RulesDecisionEngine`], which is deterministic: the same input always gives the same result.
//! The M4 micro model and the M7 Jev backend implement the same trait later, and the rest of the
//! harness never branches on which engine is active (R-9.3).
//!
//! The vocabulary is closed (R-10.3). A decision is one of the variants below, never free text.

use serde::{Deserialize, Serialize};

use crate::loops::LoopSignal;
use crate::task::{Phase, TaskState};

/// The next action the model should take (R-10.2 `NEXT_ACTION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NextAction {
    Explore,
    Implement,
    Test,
    Review,
    Diagnose,
    Summarize,
    AskUser,
    Escalate,
    Done,
}

/// A decision (R-10.1). The variants are the controlled vocabulary of M3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Decision {
    /// Carry on with this action.
    Next { action: NextAction },
    /// Retry the last action once.
    Retry,
    /// Stop going round the loop and escalate. In M3 there is no deep model yet, so this asks the user.
    Escalate,
    /// A person must decide before the task continues.
    AskUser,
    /// The task cannot continue within its limits.
    TaskBlocked,
    /// The task reached DONE.
    TaskComplete,
}

/// Why a decision was made. Each code is produced by exactly one rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    /// No loop rule fired; the phase decides the action.
    PhaseDefault,
    IterationLimit,
    ImplementationRetries,
    SameFailure,
    RepeatedEdit,
    RepeatedCommand,
}

/// The result of one decision (R-9.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResult {
    pub decision: Decision,
    /// Rules give a deterministic confidence of 1.0 when a loop rule fired, and none when the phase
    /// decided (R-9.5: rules give a deterministic confidence or nothing).
    pub confidence: Option<f32>,
    pub reason_code: ReasonCode,
    /// The engine that made the decision.
    pub provider: String,
    /// Time the engine took. Set by the caller; the pure engine reports 0.
    pub latency_ms: u64,
}

/// The input of one decision.
#[derive(Debug, Clone, Copy)]
pub struct DecisionInput<'a> {
    pub state: &'a TaskState,
    pub signals: &'a [LoopSignal],
}

/// A decision engine. Implementations must be deterministic for a given input, or must say so.
pub trait DecisionEngine {
    /// The name of the engine, recorded with every decision.
    fn provider(&self) -> &'static str;

    /// Decides the next step for the input.
    fn decide(&self, input: &DecisionInput<'_>) -> DecisionResult;
}

/// The rules engine of M3. It takes the first loop rule that fired, in priority order, and otherwise
/// follows the phase.
#[derive(Debug, Clone, Copy, Default)]
pub struct RulesDecisionEngine;

impl DecisionEngine for RulesDecisionEngine {
    fn provider(&self) -> &'static str {
        "rules"
    }

    fn decide(&self, input: &DecisionInput<'_>) -> DecisionResult {
        // A finished task is not going round a loop: DONE comes before any loop rule.
        let (decision, reason_code) = if input.state.phase == Phase::Done {
            (Decision::TaskComplete, ReasonCode::PhaseDefault)
        } else {
            match first_signal(input.signals) {
                Some(LoopSignal::IterationLimit { .. }) => {
                    (Decision::TaskBlocked, ReasonCode::IterationLimit)
                }
                Some(LoopSignal::ImplementationRetries { .. }) => {
                    (Decision::Escalate, ReasonCode::ImplementationRetries)
                }
                Some(LoopSignal::SameFailure { .. }) => {
                    (Decision::Escalate, ReasonCode::SameFailure)
                }
                Some(LoopSignal::RepeatedEdit { .. }) => {
                    (Decision::AskUser, ReasonCode::RepeatedEdit)
                }
                Some(LoopSignal::RepeatedCommand { .. }) => (
                    Decision::Next {
                        action: NextAction::Diagnose,
                    },
                    ReasonCode::RepeatedCommand,
                ),
                None => (phase_decision(input.state.phase), ReasonCode::PhaseDefault),
            }
        };
        let confidence = (reason_code != ReasonCode::PhaseDefault).then_some(1.0);
        DecisionResult {
            decision,
            confidence,
            reason_code,
            provider: self.provider().to_owned(),
            latency_ms: 0,
        }
    }
}

/// Whether a loop refusal may be shown for `result`. A refusal is shown only for a loop rule, and only
/// until `refusal_cap` consecutive refusals have been shown. After that the harness shows a notice
/// instead of refusing (proposed; see the M3 plan, decision 4).
pub fn refuses(result: &DecisionResult, consecutive_refusals: u32, refusal_cap: u32) -> bool {
    result.reason_code != ReasonCode::PhaseDefault
        && result.decision != Decision::TaskComplete
        && consecutive_refusals < refusal_cap
}

/// The rule that takes priority when several fired: a limit before a retry rule, a retry rule before
/// an edit or command rule.
fn first_signal(signals: &[LoopSignal]) -> Option<&LoopSignal> {
    let priority: [fn(&LoopSignal) -> bool; 5] = [
        |s| matches!(s, LoopSignal::IterationLimit { .. }),
        |s| matches!(s, LoopSignal::ImplementationRetries { .. }),
        |s| matches!(s, LoopSignal::SameFailure { .. }),
        |s| matches!(s, LoopSignal::RepeatedEdit { .. }),
        |s| matches!(s, LoopSignal::RepeatedCommand { .. }),
    ];
    priority
        .iter()
        .find_map(|rank| signals.iter().find(|signal| rank(signal)))
}

/// The action the phase asks for when no loop rule fired.
fn phase_decision(phase: Phase) -> Decision {
    let action = |action| Decision::Next { action };
    match phase {
        Phase::Start | Phase::Discover | Phase::Explore => action(NextAction::Explore),
        Phase::Plan | Phase::Implement => action(NextAction::Implement),
        Phase::Verify => action(NextAction::Test),
        Phase::Review => action(NextAction::Review),
        Phase::Fail | Phase::Diagnose => action(NextAction::Diagnose),
        Phase::Done => Decision::TaskComplete,
        Phase::Escalate => Decision::AskUser,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn state(phase: Phase) -> TaskState {
        let mut state = TaskState::new("task-1", "repo", "p");
        state.phase = phase;
        state
    }

    fn decide(state: &TaskState, signals: &[LoopSignal]) -> DecisionResult {
        RulesDecisionEngine.decide(&DecisionInput { state, signals })
    }

    #[test]
    fn every_phase_has_a_default_decision() {
        for phase in Phase::ALL {
            let result = decide(&state(phase), &[]);
            assert_eq!(result.reason_code, ReasonCode::PhaseDefault, "{phase:?}");
            assert_eq!(result.confidence, None, "{phase:?}");
        }
    }

    #[test]
    fn done_completes_and_escalate_asks_the_user() {
        assert_eq!(
            decide(&state(Phase::Done), &[]).decision,
            Decision::TaskComplete
        );
        assert_eq!(
            decide(&state(Phase::Escalate), &[]).decision,
            Decision::AskUser
        );
    }

    #[test]
    fn a_loop_rule_gives_a_deterministic_confidence() {
        let signals = [LoopSignal::SameFailure {
            fingerprint: "fp_v1:aaaa".to_owned(),
            count: 3,
        }];
        let result = decide(&state(Phase::Verify), &signals);
        assert_eq!(result.decision, Decision::Escalate);
        assert_eq!(result.reason_code, ReasonCode::SameFailure);
        assert_eq!(result.confidence, Some(1.0));
    }

    #[test]
    fn each_loop_signal_maps_to_its_decision() {
        let cases = [
            (
                LoopSignal::IterationLimit {
                    iteration: 8,
                    limit: 8,
                },
                Decision::TaskBlocked,
                ReasonCode::IterationLimit,
            ),
            (
                LoopSignal::ImplementationRetries {
                    entries: 4,
                    limit: 3,
                },
                Decision::Escalate,
                ReasonCode::ImplementationRetries,
            ),
            (
                LoopSignal::RepeatedEdit {
                    file: "calc.py".to_owned(),
                    count: 4,
                },
                Decision::AskUser,
                ReasonCode::RepeatedEdit,
            ),
            (
                LoopSignal::RepeatedCommand {
                    command_hash: "h".to_owned(),
                    count: 3,
                },
                Decision::Next {
                    action: NextAction::Diagnose,
                },
                ReasonCode::RepeatedCommand,
            ),
        ];
        for (signal, decision, reason) in cases {
            let result = decide(&state(Phase::Implement), std::slice::from_ref(&signal));
            assert_eq!(
                (result.decision, result.reason_code),
                (decision, reason),
                "{signal:?}"
            );
        }
    }

    #[test]
    fn the_first_rule_in_priority_order_wins() {
        let signals = [
            LoopSignal::RepeatedCommand {
                command_hash: "h".to_owned(),
                count: 3,
            },
            LoopSignal::IterationLimit {
                iteration: 8,
                limit: 8,
            },
        ];
        assert_eq!(
            decide(&state(Phase::Implement), &signals).reason_code,
            ReasonCode::IterationLimit
        );
    }

    #[test]
    fn the_same_inputs_give_the_same_result() {
        let signals = [LoopSignal::RepeatedEdit {
            file: "a.py".to_owned(),
            count: 4,
        }];
        assert_eq!(
            decide(&state(Phase::Implement), &signals),
            decide(&state(Phase::Implement), &signals)
        );
    }

    #[test]
    fn the_provider_name_is_recorded() {
        assert_eq!(decide(&state(Phase::Start), &[]).provider, "rules");
    }

    #[test]
    fn a_refusal_is_shown_for_loop_rules_only_until_the_cap() {
        let loop_result = decide(
            &state(Phase::Implement),
            &[LoopSignal::RepeatedCommand {
                command_hash: "h".to_owned(),
                count: 3,
            }],
        );
        let phase_result = decide(&state(Phase::Implement), &[]);
        assert!(refuses(&loop_result, 0, 3));
        assert!(refuses(&loop_result, 2, 3));
        assert!(!refuses(&loop_result, 3, 3), "the cap stops loop refusals");
        assert!(
            !refuses(&phase_result, 0, 3),
            "a phase default is never a refusal"
        );
    }

    #[test]
    fn a_completed_task_is_never_refused() {
        let done = decide(
            &state(Phase::Done),
            &[LoopSignal::RepeatedCommand {
                command_hash: "h".to_owned(),
                count: 3,
            }],
        );
        assert!(!refuses(&done, 0, 3));
    }
}
