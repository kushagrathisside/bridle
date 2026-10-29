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
    /// Plan again from the current state (R-20.2 REPLAN). Only a stalled loop can ask for it.
    Replan,
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
    /// Rules give a deterministic confidence when a loop rule fired: 1.0 for a counter, and a curve for a
    /// repeated failure (see [`same_failure_confidence`]). None when the phase decided (R-9.5).
    pub confidence: Option<f32>,
    pub reason_code: ReasonCode,
    /// The engine that made the decision.
    pub provider: String,
    /// Time the engine took. Set by the caller; the pure engine reports 0.
    pub latency_ms: u64,
    /// The short reason a model gave, when the decision came from one. Capped and cleaned by `lfm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_reason: Option<String>,
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
        let (decision, reason_code, confidence) = if input.state.phase == Phase::Done {
            (Decision::TaskComplete, ReasonCode::PhaseDefault, None)
        } else {
            match first_signal(input.signals) {
                Some(LoopSignal::IterationLimit { .. }) => {
                    (Decision::TaskBlocked, ReasonCode::IterationLimit, Some(1.0))
                }
                Some(LoopSignal::ImplementationRetries { .. }) => (
                    Decision::Escalate,
                    ReasonCode::ImplementationRetries,
                    Some(1.0),
                ),
                Some(LoopSignal::SameFailure { count, .. }) => (
                    Decision::Escalate,
                    ReasonCode::SameFailure,
                    Some(same_failure_confidence(*count)),
                ),
                Some(LoopSignal::RepeatedEdit { .. }) => {
                    (Decision::AskUser, ReasonCode::RepeatedEdit, Some(1.0))
                }
                Some(LoopSignal::RepeatedCommand { .. }) => (
                    Decision::Next {
                        action: NextAction::Diagnose,
                    },
                    ReasonCode::RepeatedCommand,
                    Some(1.0),
                ),
                None => (
                    phase_decision(input.state.phase),
                    ReasonCode::PhaseDefault,
                    None,
                ),
            }
        };
        DecisionResult {
            decision,
            confidence,
            reason_code,
            provider: self.provider().to_owned(),
            latency_ms: 0,
            model_reason: None,
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

/// The confidence of the same-failure rule after `count` identical failures in a row. Each repeat after the first
/// halves the doubt: two identical failures give 0.5, three give 0.75 and four give 0.875. The curve is a fixed choice,
/// not a measured probability. It is picked so the default threshold (0.75) is met at the third identical failure,
/// which is the R-17.3 limit with `max_same_failure = 2`.
fn same_failure_confidence(count: u32) -> f32 {
    let repeats = i32::try_from(count.saturating_sub(1)).unwrap_or(i32::MAX);
    1.0 - 0.5_f32.powi(repeats)
}

/// Whether a decision is due for escalation to the deep slot (R-3.6, R-18.1). An ESCALATE decision is sent to the deep
/// slot when its confidence reaches `threshold`: the threshold is the evidence needed before the deep model is asked.
/// A decision with no confidence (the micro model's answers) is always sent: the model answered for the case, and
/// there is no number to compare.
pub fn escalation_due(result: &DecisionResult, threshold: f32) -> bool {
    result.decision == Decision::Escalate && result.confidence.is_none_or(|c| c >= threshold)
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
        assert_eq!(result.confidence, Some(0.75));
    }

    #[test]
    fn the_same_failure_confidence_grows_with_each_repeat() {
        assert_eq!(same_failure_confidence(2), 0.5);
        assert_eq!(same_failure_confidence(3), 0.75);
        assert_eq!(same_failure_confidence(4), 0.875);
        assert!(same_failure_confidence(5) > same_failure_confidence(4));
    }

    #[test]
    fn a_counter_rule_keeps_full_confidence() {
        let signals = [LoopSignal::ImplementationRetries {
            entries: 4,
            limit: 3,
        }];
        assert_eq!(
            decide(&state(Phase::Implement), &signals).confidence,
            Some(1.0)
        );
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

    #[test]
    fn an_escalate_decision_is_due_when_its_confidence_reaches_the_threshold() {
        let escalate = |confidence| DecisionResult {
            decision: Decision::Escalate,
            confidence,
            reason_code: ReasonCode::SameFailure,
            provider: "rules".to_owned(),
            latency_ms: 0,
            model_reason: None,
        };
        assert!(escalation_due(&escalate(Some(1.0)), 0.75));
        assert!(escalation_due(&escalate(Some(0.75)), 0.75));
        assert!(!escalation_due(&escalate(Some(0.5)), 0.75));
        assert!(
            escalation_due(&escalate(None), 0.75),
            "a model answer has no number to compare"
        );
        let replan = DecisionResult {
            decision: Decision::Replan,
            ..escalate(Some(1.0))
        };
        assert!(!escalation_due(&replan, 0.75), "only ESCALATE is escalated");
    }

    #[test]
    fn the_threshold_asks_for_more_repeats_before_the_deep_model() {
        let after = |count| DecisionResult {
            decision: Decision::Escalate,
            confidence: Some(same_failure_confidence(count)),
            reason_code: ReasonCode::SameFailure,
            provider: "rules".to_owned(),
            latency_ms: 0,
            model_reason: None,
        };
        assert!(
            escalation_due(&after(3), 0.75),
            "the default: three repeats"
        );
        assert!(!escalation_due(&after(3), 0.9), "three is not enough");
        assert!(escalation_due(&after(5), 0.9), "five repeats are enough");
    }
}
