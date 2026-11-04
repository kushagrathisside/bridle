//! The model scheduler's rules and its swap record (plan F0.2 and F1, `docs/spec/team-subagents-ideas-plan.md` sections 3.2 to 3.4).
//!
//! An operating system does not ask a program which program runs next; a deterministic scheduler does. This is that part for the GPU: given
//! what is running and what is wanted, it says what to stop and what to start, in order; it counts swaps against a budget; it checks that
//! the wanted set fits in video memory before anything is stopped; and it keeps a **write-ahead swap record** so that a crash between
//! "stopped the big model" and "started it again" is found at the next start and put right. No model is asked anything here, and nothing
//! here starts or stops a process: the caller does, following the plan (that is what keeps these rules testable).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use harness_core::identity;
use serde::{Deserialize, Serialize};

use crate::state::atomic_write;

/// What to do, in order: stop these slots, then start these.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub stop: Vec<String>,
    pub start: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.stop.is_empty() && self.start.is_empty()
    }
}

/// From the GPU slots that run now to exactly the fleet: stop every running slot that is not in the fleet, then start every fleet slot that
/// is not running, in the fleet's order. (A slot that is in the fleet and running is left alone.)
pub fn plan_up(running: &[String], fleet: &[String]) -> Plan {
    Plan {
        stop: running
            .iter()
            .filter(|s| !fleet.contains(s))
            .cloned()
            .collect(),
        start: fleet
            .iter()
            .filter(|s| !running.contains(s))
            .cloned()
            .collect(),
    }
}

/// From the fleet back to what was there: stop the fleet's slots that run, then start the slots that were stopped, in the order they were
/// stopped, unless they already run.
pub fn plan_down(running: &[String], fleet: &[String], restore: &[String]) -> Plan {
    Plan {
        stop: fleet
            .iter()
            .filter(|s| running.contains(s))
            .cloned()
            .collect(),
        start: restore
            .iter()
            .filter(|s| !running.contains(s))
            .cloned()
            .collect(),
    }
}

/// How many swaps a run may make. Stopping the big model for the fleet is one, putting it back is one; over the budget a step runs on
/// what is loaded, or the run stops and says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    limit: u32,
    used: u32,
}

impl Budget {
    pub fn new(limit: u32) -> Self {
        Self { limit, used: 0 }
    }

    pub fn used(&self) -> u32 {
        self.used
    }

    /// Spends one swap, or refuses with the reason.
    pub fn take(&mut self, why: &str) -> Result<(), String> {
        if self.used >= self.limit {
            return Err(format!(
                "the swap budget of {} is used up; not doing: {why}",
                self.limit
            ));
        }
        self.used += 1;
        Ok(())
    }
}

/// Whether the wanted slots fit together: the sum of their estimates, the desktop's own use and a reserve must be within the GPU's
/// memory. `Ok` carries the MiB that would be left; `Err` says by how much it does not fit. The estimates come from `vram::estimate`.
pub fn fit(
    estimates_mib: &[(String, u64)],
    total_mib: u64,
    desktop_mib: u64,
    reserve_mib: u64,
) -> Result<u64, String> {
    let need: u64 =
        estimates_mib.iter().map(|(_, mib)| mib).sum::<u64>() + desktop_mib + reserve_mib;
    if need <= total_mib {
        Ok(total_mib - need)
    } else {
        let parts: Vec<String> = estimates_mib
            .iter()
            .map(|(slot, mib)| format!("{slot} {mib} MiB"))
            .collect();
        Err(format!(
            "the fleet needs {need} MiB ({}, the desktop {desktop_mib}, a reserve of {reserve_mib}) and the GPU has {total_mib} MiB",
            parts.join(", ")
        ))
    }
}

// ------------------------------------------------------------------------------------------------------------ the swap record

/// How far a swap got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The record is written; the big model is being stopped and the fleet started (or is up).
    Up,
    /// The fleet is being stopped and the big model started again.
    Restoring,
}

/// The write-ahead record of a swap: written **before** anything is stopped, removed only when the slots that were stopped run again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swap {
    pub id: String,
    pub started_unix_s: u64,
    pub phase: Phase,
    /// The fleet's slots.
    pub fleet: Vec<String>,
    /// The slots that ran before and were stopped; they are what a restore starts.
    pub stopped: Vec<String>,
}

fn record_path(workspace: &Path) -> PathBuf {
    identity::state_dir(workspace).join("swap.json")
}

impl Swap {
    pub fn new(fleet: &[String], stopped: &[String]) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self {
            id: format!("swap-{now}-{}", std::process::id()),
            started_unix_s: now,
            phase: Phase::Up,
            fleet: fleet.to_vec(),
            stopped: stopped.to_vec(),
        }
    }

    /// Writes the record (atomically). Called before the first process is stopped.
    pub fn write(&self, workspace: &Path) -> std::io::Result<()> {
        fs::create_dir_all(identity::state_dir(workspace))?;
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        atomic_write(&record_path(workspace), text.as_bytes())
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// The record left by a swap that was not finished, if there is one.
    pub fn pending(workspace: &Path) -> Option<Self> {
        serde_json::from_str(&fs::read_to_string(record_path(workspace)).ok()?).ok()
    }

    /// Removes the record: the restore is done.
    pub fn finish(workspace: &Path) {
        let _ = fs::remove_file(record_path(workspace));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn going_up_stops_what_is_not_in_the_fleet_and_starts_what_is_missing_in_order() {
        let plan = plan_up(&names(&["main"]), &names(&["manager", "worker"]));
        assert_eq!(plan.stop, names(&["main"]));
        assert_eq!(plan.start, names(&["manager", "worker"]));
        // Nothing runs: only starts. A fleet slot that already runs is left alone.
        assert_eq!(
            plan_up(&[], &names(&["manager", "worker"])).stop,
            Vec::<String>::new()
        );
        let partly = plan_up(&names(&["worker"]), &names(&["manager", "worker"]));
        assert_eq!(
            (partly.stop, partly.start),
            (Vec::new(), names(&["manager"]))
        );
        // Already the fleet: nothing to do.
        assert!(
            plan_up(
                &names(&["manager", "worker"]),
                &names(&["manager", "worker"])
            )
            .is_empty()
        );
        // Two big models running: both are stopped.
        assert_eq!(
            plan_up(&names(&["main", "deep"]), &names(&["worker"])).stop,
            names(&["main", "deep"])
        );
    }

    #[test]
    fn going_down_stops_the_fleet_and_starts_what_was_stopped() {
        let plan = plan_down(
            &names(&["manager", "worker"]),
            &names(&["manager", "worker"]),
            &names(&["main"]),
        );
        assert_eq!(plan.stop, names(&["manager", "worker"]));
        assert_eq!(plan.start, names(&["main"]));
        // The big model already runs again (a crash after the restore): nothing is started twice.
        let again = plan_down(
            &names(&["main"]),
            &names(&["manager", "worker"]),
            &names(&["main"]),
        );
        assert!(again.is_empty(), "{again:?}");
        // Nothing was running before: nothing to restore, the fleet is just stopped.
        assert_eq!(
            plan_down(&names(&["worker"]), &names(&["worker"]), &[]).start,
            Vec::<String>::new()
        );
    }

    #[test]
    fn swaps_are_counted_and_the_one_over_the_budget_is_refused_with_the_reason() {
        let mut budget = Budget::new(2);
        assert!(budget.take("stop main for the fleet").is_ok());
        assert!(budget.take("put main back").is_ok());
        let refused = budget.take("a second fleet").unwrap_err();
        assert!(
            refused.contains("budget of 2") && refused.contains("a second fleet"),
            "{refused}"
        );
        assert_eq!(budget.used(), 2);
        assert!(Budget::new(0).take("anything").is_err());
    }

    #[test]
    fn the_fleet_must_fit_in_video_memory_with_the_desktop_and_a_reserve() {
        let estimates = vec![("manager".to_owned(), 3_501), ("worker".to_owned(), 2_144)];
        // The plan's worked example: 5,645 + 230 desktop + 768 reserve in an 8,188 MiB card leaves 1,545.
        assert_eq!(
            fit(&estimates, 8_188, 230, 768).unwrap(),
            8_188 - 5_645 - 230 - 768
        );
        let error = fit(&estimates, 6_000, 230, 768).unwrap_err();
        assert!(
            error.contains("manager 3501 MiB") && error.contains("6000 MiB"),
            "{error}"
        );
        assert!(fit(&[], 8_188, 230, 768).is_ok());
    }

    #[test]
    fn the_swap_record_is_written_before_and_survives_until_the_restore_is_done() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Swap::pending(dir.path()).is_none());
        let swap = Swap::new(&names(&["manager", "worker"]), &names(&["main"]));
        swap.write(dir.path()).unwrap();
        // "The process died here": the next start finds what to put back.
        let found = Swap::pending(dir.path()).unwrap();
        assert_eq!(found, swap);
        assert_eq!(found.phase, Phase::Up);
        assert_eq!(found.stopped, names(&["main"]));
        let mut restoring = found;
        restoring.phase = Phase::Restoring;
        restoring.write(dir.path()).unwrap();
        assert_eq!(Swap::pending(dir.path()).unwrap().phase, Phase::Restoring);
        Swap::finish(dir.path());
        assert!(Swap::pending(dir.path()).is_none());
        // A damaged record is not a crash of its own.
        std::fs::write(dir.path().join(".bridle").join("swap.json"), "{ nope").unwrap();
        assert!(Swap::pending(dir.path()).is_none());
    }
}
