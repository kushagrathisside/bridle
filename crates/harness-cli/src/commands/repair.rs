//! `bridle doctor --repair`: puts right what a crash or a kill leaves behind.
//!
//! Every step is safe to repeat, touches only things Bridle itself made, and says what it did. Nothing that belongs to a live run is
//! touched: when another Bridle holds the project's run lock, the steps that depend on "no run is alive" are skipped and say so.
//!
//! | Leftover | What is done |
//! |---|---|
//! | Leases of a run that died | All leases are ended (only when no run is alive) |
//! | A run whose journal has no end | The state of the files is saved as a checkpoint and the run is marked interrupted |
//! | `bridle-index-*` files in the Git directory | Removed |
//! | `.part-*` files in the web downloads and screenshots folders | Removed (a transfer that never finished) |
//! | `*.tmp-*` files in the state folders | Removed when they are over an hour old |
//! | Browser session folders of processes that are gone | Removed |
//! | Background commands whose Bridle is gone | Stopped, after checking the process id still belongs to that program; the record is removed |

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use harness_core::identity;
use harness_runtime::leases::Leases;
use harness_runtime::state::remove_stale_temps;
use harness_runtime::web::browser::manager::sweep_session_folders;
use harness_runtime::web::browser::runtime::Layout;

use crate::gitlayer::{self, RepoLock};

/// Removes `.part-*` files in `dir` older than `age`. Returns how many.
fn remove_parts(dir: &Path, age: Duration) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(".part-") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|a| a >= age);
        if old && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Repairs a project folder. Returns one line for each thing done or skipped.
pub fn repair_project(workspace: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    match RepoLock::acquire(workspace) {
        Ok(lock) => {
            // No run is alive: whatever a lease or a journal still says belongs to a run that is gone.
            let leases = Leases::open(workspace).clear_all();
            if leases > 0 {
                lines.push(format!(
                    "ended {leases} lease(s) left by a run that is gone"
                ));
            }
            for scope in gitlayer::recover(workspace) {
                lines.push(format!(
                    "run {scope} was cut short: the state of the files was saved and the run marked interrupted (undo is on the issue)"
                ));
            }
            let indexes = gitlayer::remove_leftover_indexes(workspace);
            if indexes > 0 {
                lines.push(format!("removed {indexes} temporary Git index file(s) from a checkpoint that was cut short"));
            }
            drop(lock);
        }
        Err(why) => lines.push(format!(
            "skipped the steps that need no run to be alive: {why}"
        )),
    }
    if let Some(swap) = harness_runtime::scheduler::Swap::pending(workspace) {
        lines.push(format!(
            "an unfinished model swap is recorded ({:?} were stopped for the fleet): run `bridle fleet recover` to put them back (not done here, because starting a model takes memory and time)",
            swap.stopped
        ));
    }
    lines.extend(harness_runtime::mcp::tools::background::sweep_orphans(
        workspace,
    ));
    let state = identity::state_dir(workspace);
    let web = state.join("web");
    let parts = remove_parts(&web.join("downloads"), Duration::from_secs(300))
        + remove_parts(&web.join("screenshots"), Duration::from_secs(300));
    if parts > 0 {
        lines.push(format!(
            "removed {parts} unfinished download or screenshot file(s)"
        ));
    }
    for dir in [state.clone(), web, state.join("board")] {
        if dir.is_dir() {
            let before = fs::read_dir(&dir).map_or(0, |d| d.count());
            let _ = remove_stale_temps(&dir);
            let after = fs::read_dir(&dir).map_or(0, |d| d.count());
            if after < before {
                lines.push(format!(
                    "removed {} old temporary file(s) from {}",
                    before - after,
                    dir.display()
                ));
            }
        }
    }
    lines
}

/// Repairs the browser runtime's leftovers: session folders of processes that are gone.
pub fn repair_browser(layout: &Layout) -> Vec<String> {
    let removed = sweep_session_folders(&layout.sessions_dir());
    if removed > 0 {
        vec![format!(
            "removed {removed} browser session folder(s) of processes that are gone"
        )]
    } else {
        Vec::new()
    }
}

/// Everything, as the lines `bridle doctor --repair` prints. Never empty: it says so when there was nothing to do.
pub fn run(workspace: &Path) -> Vec<String> {
    let mut lines = repair_project(workspace);
    if let Some(layout) = Layout::default_location() {
        lines.extend(repair_browser(&layout));
    }
    if lines.is_empty() {
        lines.push("nothing needed repair".to_owned());
    }
    lines
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use harness_runtime::leases::DEFAULT_TTL;

    fn plain(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_clean_project_needs_no_repair() {
        let dir = tempfile::tempdir().unwrap();
        plain(dir.path(), &["init", "-q"]);
        assert!(repair_project(dir.path()).is_empty());
        assert_eq!(
            run(dir.path())
                .last()
                .map(String::as_str)
                .map(|l| l.contains("nothing") || l.contains("browser")),
            Some(true)
        );
    }

    #[test]
    fn what_a_crash_leaves_behind_is_put_right_and_a_live_run_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        plain(p, &["init", "-q"]);
        fs::write(p.join("a.txt"), "one\n").unwrap();
        plain(p, &["add", "-A"]);
        plain(p, &["commit", "-q", "-m", "init"]);
        // A run that began and never ended, a lease it held, a checkpoint index it never removed, and an unfinished download.
        gitlayer::checkpoint(p, "B-9", "0001-before").unwrap();
        gitlayer::journal(p, "B-9", "start", &serde_json::json!({})).unwrap();
        Leases::open(p)
            .claim("a.txt", "B-9", "run-1", DEFAULT_TTL)
            .unwrap()
            .unwrap();
        let git_dir = p.join(".git");
        fs::write(git_dir.join("bridle-index-123-x"), "junk").unwrap();
        let downloads = identity::state_dir(p).join("web").join("downloads");
        fs::create_dir_all(&downloads).unwrap();
        let part = downloads.join(".part-9-file.pdf");
        fs::write(&part, "half").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(&part)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let fresh = downloads.join(".part-9-fresh.pdf");
        fs::write(&fresh, "still coming").unwrap();

        // While another Bridle holds the project's run lock, nothing of a run is touched.
        let live = RepoLock::acquire(p).unwrap();
        let skipped = repair_project(p);
        assert!(skipped.iter().any(|l| l.contains("skipped")), "{skipped:?}");
        assert_eq!(
            Leases::open(p).list().len(),
            1,
            "a live run's lease is kept"
        );
        assert!(git_dir.join("bridle-index-123-x").exists());
        // The steps that do not depend on a run still happened: the old unfinished download is gone, a fresh one is not touched.
        assert!(!part.exists(), "the old unfinished download is gone");
        assert!(
            fresh.exists(),
            "a transfer that may still be running is left"
        );
        assert!(
            skipped.iter().any(|l| l.contains("unfinished download")),
            "{skipped:?}"
        );
        drop(live);

        let done = repair_project(p);
        let text = done.join("\n");
        assert!(text.contains("1 lease"), "{text}");
        assert!(text.contains("B-9") && text.contains("cut short"), "{text}");
        assert!(text.contains("temporary Git index"), "{text}");
        assert!(Leases::open(p).list().is_empty());
        assert!(!git_dir.join("bridle-index-123-x").exists());
        assert!(fresh.exists());
        // And it is safe to run again.
        assert!(repair_project(p).iter().all(|l| !l.contains("cut short")));
    }
}
