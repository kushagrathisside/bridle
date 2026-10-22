//! Temporary git repositories for the tool tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

/// Identity and signing flags so commits work on any machine's git config.
const COMMIT_FLAGS: &[&str] = &[
    "-c",
    "user.name=test",
    "-c",
    "user.email=test@example.invalid",
    "-c",
    "commit.gpgsign=false",
    "commit",
    "-q",
    "-m",
];

pub(super) fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

pub(super) fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A temporary repository with one committed file, or `None` when git is unavailable.
pub(super) fn init_repo() -> Option<tempfile::TempDir> {
    if !git_available() {
        eprintln!("git not found; skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    git(dir.path(), &["add", "README.md"]);
    commit(dir.path(), "init");
    Some(dir)
}

/// Commits `files` (path, content) on top of the repository's history.
pub(super) fn commit_files(dir: &Path, files: &[(&str, &str)]) {
    commit_files_with_message(dir, files, "files");
}

/// [`commit_files`] with a commit message.
pub(super) fn commit_files_with_message(dir: &Path, files: &[(&str, &str)], message: &str) {
    for (path, content) in files {
        let full = dir.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, content).unwrap();
        // Forced: fixtures such as `.env` may match an ignore rule on the machine running tests.
        git(dir, &["add", "-f", path]);
    }
    commit(dir, message);
}

fn commit(dir: &Path, message: &str) {
    let mut args = COMMIT_FLAGS.to_vec();
    args.push(message);
    git(dir, &args);
}
