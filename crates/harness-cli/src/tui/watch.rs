//! Cheap change detection for auto-refresh.
//!
//! Every [`INTERVAL`] the watcher stats a handful of files (the two event logs, the config files,
//! the managed-server records) and reports a change when any size or modification time differs
//! from the last look. It reads no file contents; the actual re-read is a normal refresh job.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use harness_core::config::machine_config_path;
use harness_core::identity;
use harness_runtime::event_log::{EVENTS, TOOL_EVENTS};
use harness_runtime::state::{INDEX_FILE, JOURNAL_FILE, TASKS_DIR};

use crate::app;
use crate::tui::data::{LoadArgs, Snapshot};

/// Minimum time between two looks at the files.
pub const INTERVAL: Duration = Duration::from_secs(2);

type Stamp = Option<(u64, SystemTime)>;
type Fingerprint = Vec<(PathBuf, Stamp)>;

#[derive(Debug, Default)]
pub struct Watcher {
    last: Option<Fingerprint>,
    last_look: Option<Instant>,
}

impl Watcher {
    /// True when something watched changed since the previous look. The first look only records.
    pub fn changed(&mut self, args: &LoadArgs, snap: &Snapshot) -> bool {
        let now = Instant::now();
        if self
            .last_look
            .is_some_and(|t| now.duration_since(t) < INTERVAL)
        {
            return false;
        }
        self.last_look = Some(now);
        let fp = fingerprint(args, snap);
        let changed = self.last.as_ref().is_some_and(|prev| *prev != fp);
        self.last = Some(fp);
        changed
    }
}

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Stamps of every file a refresh would read. Missing files are recorded as absent, so creating
/// one counts as a change.
pub fn fingerprint(args: &LoadArgs, snap: &Snapshot) -> Fingerprint {
    let workspace = snap
        .workspace
        .clone()
        .or_else(|| app::resolve_workspace(args.workspace.as_deref()).ok());
    let mut paths: Vec<PathBuf> = Vec::new();
    paths.extend(args.config.clone());
    paths.extend(machine_config_path());
    paths.extend(snap.config_files.iter().map(|(p, _)| PathBuf::from(p)));
    if let Some(ws) = &workspace {
        paths.push(ws.join(identity::CONFIG_FILE));
        let state = ws.join(identity::STATE_DIR);
        paths.push(state.join(TOOL_EVENTS));
        paths.push(state.join(EVENTS));
        if let Ok(entries) = std::fs::read_dir(state.join("servers")) {
            paths.extend(entries.flatten().map(|e| e.path()));
        }
        // The session index and the journals of the newest tasks: a change to one changes the Tasks view.
        paths.push(state.join(INDEX_FILE));
        if let Ok(entries) = std::fs::read_dir(state.join(TASKS_DIR)) {
            let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            dirs.sort();
            paths.extend(dirs.iter().rev().take(8).map(|dir| dir.join(JOURNAL_FILE)));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|p| {
            let s = stamp(&p);
            (p, s)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;

    fn args(dir: &Path) -> LoadArgs {
        LoadArgs {
            config: None,
            workspace: Some(dir.to_path_buf()),
        }
    }

    #[test]
    fn appending_to_a_log_is_a_change_and_idle_is_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join(identity::STATE_DIR);
        std::fs::create_dir_all(&state).expect("mkdir");
        let log = state.join(TOOL_EVENTS);
        std::fs::write(&log, "{}\n").expect("write");
        let a = args(dir.path());
        let snap = Snapshot::default();

        let before = fingerprint(&a, &snap);
        assert_eq!(before, fingerprint(&a, &snap), "idle files do not change");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .and_then(|mut f| f.write_all(b"{\"x\":1}\n"))
            .expect("append");
        assert_ne!(before, fingerprint(&a, &snap));
    }

    #[test]
    fn creating_a_file_or_a_server_record_is_a_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = args(dir.path());
        let snap = Snapshot::default();
        let before = fingerprint(&a, &snap);
        let servers = dir.path().join(identity::STATE_DIR).join("servers");
        std::fs::create_dir_all(&servers).expect("mkdir");
        std::fs::write(servers.join("main.pid"), "pid=1\n").expect("write");
        assert_ne!(before, fingerprint(&a, &snap));
    }

    #[test]
    fn a_task_journal_append_is_a_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let task = dir
            .path()
            .join(identity::STATE_DIR)
            .join(TASKS_DIR)
            .join("task-0001");
        std::fs::create_dir_all(&task).expect("mkdir");
        let journal = task.join(JOURNAL_FILE);
        std::fs::write(&journal, "{}\n").expect("write");
        let a = args(dir.path());
        let snap = Snapshot::default();
        let before = fingerprint(&a, &snap);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&journal)
            .and_then(|mut f| f.write_all(b"{\"x\":1}\n"))
            .expect("append");
        assert_ne!(before, fingerprint(&a, &snap));
    }

    #[test]
    fn watcher_is_throttled_and_the_first_look_only_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = args(dir.path());
        let snap = Snapshot::default();
        let mut w = Watcher::default();
        assert!(!w.changed(&a, &snap), "first look records");
        std::fs::create_dir_all(dir.path().join(identity::STATE_DIR)).expect("mkdir");
        std::fs::write(dir.path().join(identity::STATE_DIR).join(EVENTS), "{}\n").expect("write");
        assert!(
            !w.changed(&a, &snap),
            "a second look inside the interval is skipped"
        );
        w.last_look = Some(Instant::now() - INTERVAL - Duration::from_millis(1));
        assert!(w.changed(&a, &snap));
        w.last_look = Some(Instant::now() - INTERVAL - Duration::from_millis(1));
        assert!(!w.changed(&a, &snap), "no change since the last look");
    }
}
