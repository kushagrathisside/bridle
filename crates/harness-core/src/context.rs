//! Context management (M6, R-23 and R-24): decides what the model sees of each tool result.
//!
//! Pure rules over sizes, ages and kinds, against a budget from `[context]`. No model decides here (R-24.2).
//! The caller applies the actions: [`Action::Compact`] keeps a head, a tail or the failure lines of a result,
//! and [`Action::Drop`] replaces it with a one-line stub. Nothing is ever added to a result.

use serde::{Deserialize, Serialize};

/// The most recent results that are always kept whole.
pub const KEEP_RECENT: usize = 3;
/// A result over this many bytes is compacted when the budget asks for it.
pub const COMPACT_OVER_BYTES: usize = 2000;
/// Lines kept at the start and at the end of a compacted result.
pub const HEAD_LINES: usize = 20;
pub const TAIL_LINES: usize = 10;
/// Failure lines kept from a compacted verification result.
pub const MAX_FAILURE_LINES: usize = 40;
/// Bytes per token in the estimate. The estimate is stated, not measured per model.
pub const BYTES_PER_TOKEN: usize = 4;
/// Bytes a compacted result is assumed to occupy when planning (head and tail, with the marker).
const COMPACTED_BYTES: usize = 2400;
/// Bytes a dropped result is assumed to occupy when planning (the stub).
const STUB_BYTES: usize = 96;

/// One completed tool result, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultView {
    pub call_id: String,
    pub tool: String,
    pub bytes: usize,
    pub verification: bool,
    pub read_only: bool,
}

/// What to do with one result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Keep,
    Compact,
    Drop,
}

/// The budget in bytes: `target` is where compaction stops; `hard` is where dropping starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub target_bytes: usize,
    pub hard_bytes: usize,
}

impl Budget {
    /// The budget from the `[context]` section, in tokens.
    pub fn from_tokens(target_tokens: u32, hard_limit: u32) -> Self {
        Self {
            target_bytes: usize::try_from(target_tokens).unwrap_or(usize::MAX) * BYTES_PER_TOKEN,
            hard_bytes: usize::try_from(hard_limit).unwrap_or(usize::MAX) * BYTES_PER_TOKEN,
        }
    }
}

/// Plans one action per result. `total_bytes` is the size of the whole context, including the results.
/// The most recent [`KEEP_RECENT`] results are kept whole while the context fits the hard limit. Only when it
/// would not fit are the large recent results compacted, as the last step.
pub fn plan(results: &[ResultView], total_bytes: usize, budget: Budget) -> Vec<Action> {
    let mut actions = vec![Action::Keep; results.len()];
    let older = results.len().saturating_sub(KEEP_RECENT);
    let mut total = total_bytes;

    // Verification results older than the recent window keep only their failure lines.
    for index in 0..older {
        if results[index].verification {
            change(&mut actions, &mut total, results, index, Action::Compact);
        }
    }
    // Over the target: compact older large results, oldest first, until the target is met.
    if total > budget.target_bytes {
        for index in 0..older {
            if total <= budget.target_bytes {
                break;
            }
            if actions[index] == Action::Keep && results[index].bytes > COMPACT_OVER_BYTES {
                change(&mut actions, &mut total, results, index, Action::Compact);
            }
        }
    }
    // Over the hard limit: drop older read-only results, oldest first. Write results are never dropped.
    if total > budget.hard_bytes {
        for index in 0..older {
            if total <= budget.hard_bytes {
                break;
            }
            if results[index].read_only && actions[index] != Action::Drop {
                change(&mut actions, &mut total, results, index, Action::Drop);
            }
        }
    }
    // Still over the hard limit: compact the large results that are left, recent ones included, oldest first.
    if total > budget.hard_bytes {
        for index in 0..results.len() {
            if total <= budget.hard_bytes {
                break;
            }
            if actions[index] == Action::Keep && results[index].bytes > COMPACT_OVER_BYTES {
                change(&mut actions, &mut total, results, index, Action::Compact);
            }
        }
    }
    actions
}

/// Sets the action of one result and moves the planned total by the change in that result's planned size.
fn change(
    actions: &mut [Action],
    total: &mut usize,
    results: &[ResultView],
    index: usize,
    to: Action,
) {
    let bytes = results[index].bytes;
    let before = planned_bytes(bytes, actions[index]);
    let after = planned_bytes(bytes, to);
    *total = total.saturating_sub(before).saturating_add(after);
    actions[index] = to;
}

/// The size a result of `bytes` bytes is planned to take after an action.
fn planned_bytes(bytes: usize, action: Action) -> usize {
    match action {
        Action::Keep => bytes,
        Action::Compact => bytes.min(COMPACTED_BYTES),
        Action::Drop => STUB_BYTES,
    }
}

/// The head and the tail of a result, with a marker saying how many lines were omitted. A short result is
/// returned unchanged.
pub fn compact_text(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= HEAD_LINES + TAIL_LINES {
        return text.to_owned();
    }
    let omitted = lines.len() - HEAD_LINES - TAIL_LINES;
    let mut out = lines[..HEAD_LINES].join("\n");
    out.push_str(&format!(
        "\n[compacted by the harness: {omitted} lines omitted]\n"
    ));
    out.push_str(&lines[lines.len() - TAIL_LINES..].join("\n"));
    out
}

/// A verification result reduced to its failure lines. Without any failure line it falls back to
/// [`compact_text`].
pub fn compact_verification(text: &str) -> String {
    let failure_lines: Vec<&str> = text
        .lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            [
                "fail",
                "error",
                "assert",
                "panick",
                "exception",
                "traceback",
                "expected",
            ]
            .iter()
            .any(|word| lower.contains(word))
        })
        .collect();
    if failure_lines.is_empty() {
        return compact_text(text);
    }
    let shown = failure_lines.len().min(MAX_FAILURE_LINES);
    let mut out = failure_lines[..shown].join("\n");
    if failure_lines.len() > shown {
        out.push_str(&format!(
            "\n[compacted by the harness: {} more failure lines omitted]",
            failure_lines.len() - shown
        ));
    }
    out
}

/// The stub that replaces a dropped result.
pub fn stub(tool: &str) -> String {
    format!("[dropped by the harness to fit the context budget: an earlier {tool} result]")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn view(index: usize, bytes: usize, verification: bool, read_only: bool) -> ResultView {
        ResultView {
            call_id: format!("c{index}"),
            tool: if read_only {
                "read".to_owned()
            } else {
                "run_tests".to_owned()
            },
            bytes,
            verification,
            read_only,
        }
    }

    const BUDGET: Budget = Budget {
        target_bytes: 10_000,
        hard_bytes: 20_000,
    };

    #[test]
    fn under_the_budget_only_old_verifications_are_compacted() {
        let results = vec![
            view(0, 500, false, true),
            view(1, 500, true, false),
            view(2, 500, false, true),
            view(3, 500, false, true),
            view(4, 500, false, true),
        ];
        let actions = plan(&results, 2_500, BUDGET);
        assert_eq!(
            actions,
            [
                Action::Keep,
                Action::Compact,
                Action::Keep,
                Action::Keep,
                Action::Keep
            ]
        );
    }

    #[test]
    fn recent_results_are_kept_whole_while_the_context_fits() {
        // Five results of 3000 bytes: over the target, but within the hard limit. Only the two older ones change.
        let results: Vec<ResultView> = (0..5).map(|i| view(i, 3_000, false, false)).collect();
        let actions = plan(&results, 15_000, BUDGET);
        assert_eq!(
            actions,
            vec![
                Action::Compact,
                Action::Compact,
                Action::Keep,
                Action::Keep,
                Action::Keep,
            ]
        );
    }

    #[test]
    fn a_recent_result_is_compacted_only_when_the_context_would_not_fit() {
        // The last result alone is 30000 bytes, far over the hard limit. It is compacted; the small ones are kept.
        let mut results: Vec<ResultView> = (0..3).map(|i| view(i, 100, false, false)).collect();
        results.push(view(3, 30_000, false, false));
        let actions = plan(&results, 30_300, BUDGET);
        assert_eq!(
            actions,
            vec![Action::Keep, Action::Keep, Action::Keep, Action::Compact,]
        );
    }

    #[test]
    fn over_the_target_compaction_stops_once_the_target_is_met() {
        // Six results of 4000 bytes: total 12000, target 10000. Two compactions (4000 to 2400 each) are enough.
        let results: Vec<ResultView> = (0..6).map(|i| view(i, 4_000, false, false)).collect();
        let actions = plan(&results, 12_000, BUDGET);
        assert_eq!(
            actions,
            [
                Action::Compact,
                Action::Compact,
                Action::Keep,
                Action::Keep,
                Action::Keep,
                Action::Keep,
            ]
        );
    }

    #[test]
    fn over_the_hard_limit_read_only_results_are_dropped_before_write_results() {
        let results: Vec<ResultView> = (0..6).map(|i| view(i, 6_000, false, i % 2 == 0)).collect();
        let actions = plan(&results, 36_000, BUDGET);
        assert_eq!(
            actions[0],
            Action::Drop,
            "the oldest read-only result goes first"
        );
        assert_eq!(
            actions[1],
            Action::Compact,
            "a write result is compacted, not dropped"
        );
    }

    #[test]
    fn the_plan_is_deterministic() {
        let results: Vec<ResultView> = (0..8)
            .map(|i| view(i, 3_000 + i * 100, i % 3 == 0, i % 2 == 0))
            .collect();
        assert_eq!(
            plan(&results, 40_000, BUDGET),
            plan(&results, 40_000, BUDGET)
        );
    }

    #[test]
    fn compact_text_keeps_the_head_and_tail_and_says_what_was_omitted() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let compacted = compact_text(&text);
        assert!(compacted.starts_with("line 0\n"));
        assert!(compacted.ends_with("line 99"));
        assert!(compacted.contains("[compacted by the harness: 70 lines omitted]"));
    }

    #[test]
    fn a_short_text_is_not_compacted() {
        assert_eq!(compact_text("one\ntwo"), "one\ntwo");
    }

    #[test]
    fn compact_verification_keeps_the_failure_lines() {
        let text = "collecting\n....\nFAILED tests/test_calc.py::test_add - assert -1 == 3\nall the rest is quiet\n";
        let compacted = compact_verification(text);
        assert!(compacted.contains("FAILED tests/test_calc.py::test_add"));
        assert!(!compacted.contains("collecting"));
    }

    #[test]
    fn the_budget_is_taken_in_tokens() {
        let budget = Budget::from_tokens(16_000, 24_000);
        assert_eq!(budget.target_bytes, 64_000);
        assert_eq!(budget.hard_bytes, 96_000);
    }
}
