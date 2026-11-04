//! The Git layer for team runs: a safety net under the members' work in the project's working copy.
//!
//! Before and after a member's turn the working tree is saved as a **checkpoint**: a Git commit under `refs/bridle/runs/`,
//! made with plumbing and a temporary index, so the person's branch, index, staged files and stash are never touched. A run
//! can be **undone** from its two checkpoints: each file the run changed is put back, unless it changed again since (the
//! person typed in it), in which case it is left and reported. One write-run per project at a time is enforced with a file lock
//! that the system releases if Bridle dies. A run writes a small journal, so a run cut short by a crash is found at the next start.
//!
//! The commands are plain `git`, so everything here can be inspected and undone by hand (docs/reference/run-recovery.md).

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use harness_core::identity;
use harness_runtime::process::hide_console;
use serde_json::{Value, json};

/// Where checkpoints live. They are refs, not branches: they appear in no branch's log and a normal push does not send them.
pub const REF_ROOT: &str = "refs/bridle/runs";
const AUTHOR: &str = "Bridle";
const AUTHOR_EMAIL: &str = "bridle@localhost";
/// Whether a path is left out of a checkpoint: a secret-looking file, or one of the harness's own folders.
fn left_out(path: &str) -> bool {
    let lower = path.to_ascii_lowercase().replace('\\', "/");
    if [".bridle/", ".local-agent/", ".worktrees/"]
        .iter()
        .any(|dir| lower.starts_with(dir))
    {
        return true;
    }
    let name = lower.rsplit('/').next().unwrap_or("");
    name == ".env"
        || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name == ".netrc"
        || name == ".npmrc"
}

fn git(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
    git_with_input(dir, args, env, None)
}

fn git_with_input(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    input: Option<&[u8]>,
) -> Result<String> {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "core.safecrlf=false",
            "-c",
            "advice.addIgnoredFile=false",
        ])
        .args(args)
        .current_dir(dir)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0");
    for (key, value) in env {
        command.env(key, value);
    }
    hide_console(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("run git {}", args.first().copied().unwrap_or("")))?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(bytes);
    }
    let output = child
        .wait_with_output()
        .with_context(|| format!("wait for git {}", args.first().copied().unwrap_or("")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned())
}

/// What is known about the project's repository before a run starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preflight {
    /// Whether the folder is inside a Git work tree. When it is not, runs go ahead without checkpoints.
    pub repo: bool,
    /// The commit HEAD names, when there is one.
    pub head: Option<String>,
    /// Paths with uncommitted changes of the person's, at most 50.
    pub dirty: Vec<String>,
    /// Why a run must not start: a merge, rebase, cherry-pick, revert or bisect is in progress, or paths are in conflict.
    pub problem: Option<String>,
}

pub fn preflight(workspace: &Path) -> Preflight {
    let mut found = Preflight::default();
    if git(workspace, &["rev-parse", "--is-inside-work-tree"], &[])
        .ok()
        .as_deref()
        != Some("true")
    {
        return found;
    }
    found.repo = true;
    found.head = git(workspace, &["rev-parse", "--verify", "-q", "HEAD"], &[])
        .ok()
        .filter(|h| !h.is_empty());
    if let Ok(dir) = git(workspace, &["rev-parse", "--absolute-git-dir"], &[]) {
        let dir = PathBuf::from(dir);
        let states = [
            ("MERGE_HEAD", "a merge"),
            ("CHERRY_PICK_HEAD", "a cherry-pick"),
            ("REVERT_HEAD", "a revert"),
            ("BISECT_LOG", "a bisect"),
            ("rebase-merge", "a rebase"),
            ("rebase-apply", "a rebase"),
        ];
        if let Some((_, what)) = states.iter().find(|(name, _)| dir.join(name).exists()) {
            found.problem = Some(format!(
                "the repository is in the middle of {what}; finish or abort it first, then start the work again"
            ));
        }
    }
    if found.problem.is_none()
        && git(workspace, &["diff", "--name-only", "--diff-filter=U"], &[])
            .is_ok_and(|conflicts| !conflicts.is_empty())
    {
        found.problem = Some(
            "some paths are in conflict; resolve them first, then start the work again".to_owned(),
        );
    }
    if let Ok(status) = git(
        workspace,
        &["status", "--porcelain", "--no-renames"],
        &[("GIT_OPTIONAL_LOCKS", "0")],
    ) {
        found.dirty = status
            .lines()
            .filter_map(|l| l.get(3..))
            .map(|p| p.trim_matches('"').to_owned())
            .take(50)
            .collect();
    }
    found
}

/// A saved state of the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub reference: String,
    pub commit: String,
}

fn safe_scope(scope: &str) -> String {
    scope
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Saves the working tree as `refs/bridle/runs/<scope>/<name>`. Files that Git ignores, secret-looking files and the harness's own
/// folders are left out. The person's index, HEAD and branch are not touched. An unborn branch (no commit yet) works.
pub fn checkpoint(workspace: &Path, scope: &str, name: &str) -> Result<Checkpoint> {
    let info = preflight(workspace);
    if !info.repo {
        bail!("this folder is not a Git repository, so there is nothing to checkpoint");
    }
    let git_dir = git(workspace, &["rev-parse", "--absolute-git-dir"], &[])?;
    let index = PathBuf::from(git_dir).join(format!(
        "bridle-index-{}-{}",
        std::process::id(),
        name.replace(['/', '\\'], "-")
    ));
    let index_text = index.display().to_string();
    let env = [
        ("GIT_INDEX_FILE", index_text.as_str()),
        ("GIT_AUTHOR_NAME", AUTHOR),
        ("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL),
        ("GIT_COMMITTER_NAME", AUTHOR),
        ("GIT_COMMITTER_EMAIL", AUTHOR_EMAIL),
    ];
    let made = (|| -> Result<Checkpoint> {
        if info.head.is_some() {
            git(workspace, &["read-tree", "HEAD"], &env)?;
        }
        // Everything Git would add (tracked files, deleted ones, untracked files that are not ignored), without the files left out.
        let listed = git(
            workspace,
            &["ls-files", "-z", "-c", "-o", "--exclude-standard"],
            &[],
        )?;
        let mut paths: Vec<u8> = Vec::new();
        for path in listed.split('\0').filter(|p| !p.is_empty() && !left_out(p)) {
            paths.extend_from_slice(path.as_bytes());
            paths.push(0);
        }
        if !paths.is_empty() {
            git_with_input(
                workspace,
                &[
                    "--literal-pathspecs",
                    "add",
                    "-A",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
                &env,
                Some(&paths),
            )?;
        }
        let tree = git(workspace, &["write-tree"], &env)?;
        let message = format!("bridle checkpoint {scope} {name}");
        let mut commit = vec!["commit-tree", tree.as_str(), "-m", message.as_str()];
        if let Some(head) = &info.head {
            commit.extend(["-p", head.as_str()]);
        }
        let commit = git(workspace, &commit, &env)?;
        let reference = format!("{REF_ROOT}/{}/{}", safe_scope(scope), name);
        git(workspace, &["update-ref", &reference, &commit], &[])?;
        Ok(Checkpoint { reference, commit })
    })();
    let _ = fs::remove_file(&index);
    made
}

/// The checkpoints of a scope (an issue), oldest first.
pub fn list(workspace: &Path, scope: &str) -> Vec<String> {
    let prefix = format!("{REF_ROOT}/{}/", safe_scope(scope));
    let mut refs: Vec<String> = git(
        workspace,
        &["for-each-ref", "--format=%(refname)", &prefix],
        &[],
    )
    .map(|t| t.lines().map(str::to_owned).collect())
    .unwrap_or_default();
    refs.sort();
    refs
}

/// The scopes that have at least one before/after pair, so the window can offer an undo.
pub fn undoable_scopes(workspace: &Path) -> BTreeSet<String> {
    let all = git(
        workspace,
        &["for-each-ref", "--format=%(refname)", REF_ROOT],
        &[],
    )
    .unwrap_or_default();
    let mut found = BTreeSet::new();
    for line in all.lines() {
        if let Some(rest) = line.strip_prefix(&format!("{REF_ROOT}/"))
            && let Some((scope, name)) = rest.rsplit_once('/')
            && name.ends_with("-after")
        {
            found.insert(scope.to_owned());
        }
    }
    found
}

/// How a run changed the tree between two checkpoints: `(status, path)` with status `A`, `M` or `D`.
fn changes(workspace: &Path, before: &str, after: &str) -> Result<Vec<(char, String)>> {
    let text = git(
        workspace,
        &["diff", "--name-status", "--no-renames", "-z", before, after],
        &[],
    )?;
    let mut parts = text.split('\0').filter(|p| !p.is_empty());
    let mut out = Vec::new();
    while let (Some(status), Some(path)) = (parts.next(), parts.next()) {
        if let Some(kind) = status.chars().next() {
            out.push((kind, path.to_owned()));
        }
    }
    Ok(out)
}

/// The paths a run changed, between its two checkpoints.
pub fn changed_paths(workspace: &Path, before: &str, after: &str) -> Result<Vec<String>> {
    Ok(changes(workspace, before, after)?
        .into_iter()
        .map(|(_, path)| path)
        .collect())
}

/// What an undo did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UndoReport {
    pub restored: Vec<String>,
    pub removed: Vec<String>,
    /// Files left alone, with the reason (changed again since the run).
    pub skipped: Vec<(String, String)>,
}

fn working_hash(workspace: &Path, path: &str) -> Option<String> {
    if !workspace.join(path).is_file() {
        return None;
    }
    git(workspace, &["hash-object", "--", path], &[]).ok()
}

/// Puts back what a run changed, from the checkpoints taken before and after it. A file is touched only if it is still as the
/// run left it; one the person changed since is left and listed.
pub fn undo(workspace: &Path, before: &str, after: &str) -> Result<UndoReport> {
    let mut report = UndoReport::default();
    for (kind, path) in changes(workspace, before, after)? {
        let current = working_hash(workspace, &path);
        let run_left = git(
            workspace,
            &["rev-parse", "--verify", "-q", &format!("{after}:{path}")],
            &[],
        )
        .ok();
        match kind {
            'A' => match (&current, &run_left) {
                (None, _) => {}
                (Some(now), Some(then)) if now == then => {
                    fs::remove_file(workspace.join(&path))
                        .with_context(|| format!("remove {path}"))?;
                    report.removed.push(path);
                }
                _ => report
                    .skipped
                    .push((path, "it changed after the run".to_owned())),
            },
            'M' => {
                if current.is_some() && current == run_left {
                    git(
                        workspace,
                        &[
                            "restore",
                            &format!("--source={before}"),
                            "--worktree",
                            "--",
                            &format!(":(literal){path}"),
                        ],
                        &[],
                    )?;
                    report.restored.push(path);
                } else {
                    report
                        .skipped
                        .push((path, "it changed after the run".to_owned()));
                }
            }
            'D' => {
                if current.is_none() {
                    git(
                        workspace,
                        &[
                            "restore",
                            &format!("--source={before}"),
                            "--worktree",
                            "--",
                            &format!(":(literal){path}"),
                        ],
                        &[],
                    )?;
                    report.restored.push(path);
                } else {
                    report.skipped.push((path, "it exists again".to_owned()));
                }
            }
            _ => report
                .skipped
                .push((path, format!("a change of kind {kind} is not undone"))),
        }
    }
    Ok(report)
}

/// The latest run of a scope that has both a "before" and an "after" checkpoint: their refs.
pub fn latest_pair(workspace: &Path, scope: &str) -> Option<(String, String)> {
    let refs = list(workspace, scope);
    refs.iter()
        .rev()
        .filter_map(|r| r.strip_suffix("-after"))
        .find_map(|stem| {
            let before = format!("{stem}-before");
            refs.contains(&before)
                .then(|| (before, format!("{stem}-after")))
        })
}

// ---------------------------------------------------------------------------------------------

/// The right to run a write-run in a project: held while a member works. The system releases it if Bridle dies.
#[derive(Debug)]
pub struct RepoLock {
    file: File,
}

impl RepoLock {
    pub fn acquire(workspace: &Path) -> Result<Self, String> {
        let dir = identity::state_dir(workspace);
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("repo.lock"))
            .map_err(|e| format!("cannot open the repository lock: {e}"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(std::fs::TryLockError::WouldBlock) => Err("another Bridle is running a team in this project; wait for it, or close that window".to_owned()),
            Err(std::fs::TryLockError::Error(e)) => Err(format!("cannot take the repository lock: {e}")),
        }
    }
}

impl Drop for RepoLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn journal_path(workspace: &Path, scope: &str) -> PathBuf {
    identity::state_dir(workspace)
        .join("runs")
        .join(safe_scope(scope))
        .join("journal.jsonl")
}

/// Appends one line to a run's journal and flushes it to disk.
pub fn journal(workspace: &Path, scope: &str, kind: &str, detail: &Value) -> Result<()> {
    let path = journal_path(workspace, scope);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let line = json!({"time": humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string(), "kind": kind, "detail": detail});
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{line}")?;
    file.sync_all()?;
    Ok(())
}

fn last_kind(workspace: &Path, scope_dir: &Path) -> Option<String> {
    let _ = workspace;
    let text = fs::read_to_string(scope_dir.join("journal.jsonl")).ok()?;
    let last = text.lines().rev().find(|l| !l.trim().is_empty())?;
    serde_json::from_str::<Value>(last)
        .ok()?
        .get("kind")?
        .as_str()
        .map(str::to_owned)
}

/// A write-run in progress: the lock, and the checkpoint taken before it.
#[derive(Debug)]
pub struct RunGuard {
    _lock: RepoLock,
    workspace: PathBuf,
    scope: String,
    number: usize,
    repo: bool,
    pub before: Option<Checkpoint>,
}

/// What a finished run left behind.
#[derive(Debug, Clone, Default)]
pub struct Finished {
    pub before: Option<Checkpoint>,
    pub after: Option<Checkpoint>,
    pub note: Option<String>,
}

impl RunGuard {
    /// Takes the project's lock, checks the repository, and saves the "before" checkpoint. Refuses (with the reason) when another
    /// Bridle runs a team here, or when the repository is in the middle of a merge or rebase.
    pub fn begin(workspace: &Path, scope: &str) -> Result<Self, String> {
        let lock = RepoLock::acquire(workspace)?;
        let info = preflight(workspace);
        if let Some(problem) = info.problem {
            return Err(problem);
        }
        let number = list(workspace, scope)
            .iter()
            .filter(|r| r.ends_with("-before"))
            .count()
            + 1;
        let mut guard = Self {
            _lock: lock,
            workspace: workspace.to_path_buf(),
            scope: scope.to_owned(),
            number,
            repo: info.repo,
            before: None,
        };
        if info.repo {
            let name = format!("{number:04}-before");
            guard.before = Some(
                checkpoint(workspace, scope, &name)
                    .map_err(|e| format!("cannot save the state before the run: {e:#}"))?,
            );
        }
        let _ = journal(
            workspace,
            scope,
            "start",
            &json!({"repo": info.repo, "dirty": info.dirty, "before": guard.before.as_ref().map(|c| c.commit.clone())}),
        );
        Ok(guard)
    }

    /// Saves the "after" checkpoint and closes the journal. The lock is released when the guard is dropped.
    pub fn finish(self, outcome: &str) -> Finished {
        let mut done = Finished {
            before: self.before.clone(),
            ..Finished::default()
        };
        if self.repo {
            match checkpoint(
                &self.workspace,
                &self.scope,
                &format!("{:04}-after", self.number),
            ) {
                Ok(after) => done.after = Some(after),
                Err(error) => {
                    done.note = Some(format!("cannot save the state after the run: {error:#}"))
                }
            }
        }
        let _ = journal(
            &self.workspace,
            &self.scope,
            "end",
            &json!({"outcome": outcome, "after": done.after.as_ref().map(|c| c.commit.clone())}),
        );
        done
    }
}

/// At the start of a window: a run whose journal has no end was cut short by a crash or a close. It is marked interrupted and the
/// state of the tree is saved, so nothing it did is lost. Returns the scopes found.
pub fn recover(workspace: &Path) -> Vec<String> {
    let root = identity::state_dir(workspace).join("runs");
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let scope = entry.file_name().to_string_lossy().into_owned();
        if matches!(last_kind(workspace, &dir).as_deref(), Some("start")) {
            let number = list(workspace, &scope)
                .iter()
                .filter(|r| r.ends_with("-before"))
                .count();
            let saved = checkpoint(workspace, &scope, &format!("{number:04}-after")).ok();
            let _ = journal(
                workspace,
                &scope,
                "end",
                &json!({"outcome": "interrupted", "after": saved.map(|c| c.commit)}),
            );
            found.push(scope);
        }
    }
    found
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) -> String {
        git(
            dir,
            args,
            &[
                ("GIT_AUTHOR_NAME", "t"),
                ("GIT_AUTHOR_EMAIL", "t@example.com"),
                ("GIT_COMMITTER_NAME", "t"),
                ("GIT_COMMITTER_EMAIL", "t@example.com"),
            ],
        )
        .unwrap()
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q"]);
        run(dir.path(), &["config", "commit.gpgsign", "false"]);
        fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        fs::write(dir.path().join(".gitignore"), ".bridle/\nbuild/\n").unwrap();
        run(dir.path(), &["add", "-A"]);
        run(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn a_checkpoint_leaves_the_persons_branch_index_and_stash_alone() {
        let dir = repo();
        let p = dir.path();
        fs::write(p.join("a.txt"), "changed\n").unwrap();
        fs::write(p.join("new.txt"), "new\n").unwrap();
        run(p, &["add", "a.txt"]);
        fs::write(p.join("b.txt"), "unstaged\n").unwrap();
        let (head, status, stash) = (
            run(p, &["rev-parse", "HEAD"]),
            run(p, &["status", "--porcelain"]),
            run(p, &["stash", "list"]),
        );
        let c = checkpoint(p, "B-1", "0001-before").unwrap();
        assert_eq!(run(p, &["rev-parse", "HEAD"]), head, "HEAD is the same");
        assert_eq!(
            run(p, &["status", "--porcelain"]),
            status,
            "the index and the working tree are as they were"
        );
        assert_eq!(run(p, &["stash", "list"]), stash);
        assert!(c.reference.starts_with("refs/bridle/runs/B-1/"));
        // The checkpoint holds the working state, including the untracked file.
        assert_eq!(
            run(p, &["show", &format!("{}:new.txt", c.reference)]),
            "new"
        );
        assert_eq!(
            run(p, &["show", &format!("{}:b.txt", c.reference)]),
            "unstaged"
        );
        assert!(
            run(p, &["branch", "--contains", &c.commit]).is_empty(),
            "it is on no branch"
        );
        assert_eq!(
            run(p, &["log", "--oneline", "--branches"]).lines().count(),
            1,
            "no branch gained a commit"
        );
    }

    #[test]
    fn secret_files_ignored_files_and_the_harness_folder_are_left_out() {
        let dir = repo();
        let p = dir.path();
        fs::write(p.join(".env"), "TOKEN=abc\n").unwrap();
        fs::write(p.join("server.pem"), "key\n").unwrap();
        fs::create_dir_all(p.join("build")).unwrap();
        fs::write(p.join("build/out.o"), "x").unwrap();
        fs::create_dir_all(p.join(".bridle")).unwrap();
        fs::write(p.join(".bridle/board.txt"), "x").unwrap();
        fs::write(p.join("code.rs"), "fn main() {}\n").unwrap();
        let c = checkpoint(p, "B-2", "0001-before").unwrap();
        let files = run(p, &["ls-tree", "-r", "--name-only", &c.commit]);
        assert!(
            files.contains("code.rs") && files.contains("a.txt"),
            "{files}"
        );
        for hidden in [".env", "server.pem", "build/out.o", ".bridle/board.txt"] {
            assert!(
                !files.lines().any(|f| f == hidden),
                "{hidden} must not be saved: {files}"
            );
        }
    }

    #[test]
    fn a_repository_with_no_commit_yet_can_be_checkpointed() {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join("x.txt"), "x\n").unwrap();
        let info = preflight(dir.path());
        assert!(info.repo && info.head.is_none());
        let c = checkpoint(dir.path(), "B-3", "0001-before").unwrap();
        assert_eq!(
            run(dir.path(), &["show", &format!("{}:x.txt", c.reference)]),
            "x"
        );
    }

    #[test]
    fn undo_puts_back_what_the_run_changed_and_leaves_what_the_person_changed_since() {
        let dir = repo();
        let p = dir.path();
        let before = checkpoint(p, "B-4", "0001-before").unwrap();
        // The run: modifies a.txt, deletes b.txt, creates c.txt and d.txt.
        fs::write(p.join("a.txt"), "run edit\n").unwrap();
        fs::remove_file(p.join("b.txt")).unwrap();
        fs::write(p.join("c.txt"), "created\n").unwrap();
        fs::write(p.join("d.txt"), "created too\n").unwrap();
        let after = checkpoint(p, "B-4", "0001-after").unwrap();
        // The person then edits d.txt.
        fs::write(p.join("d.txt"), "my own edit\n").unwrap();
        let report = undo(p, &before.reference, &after.reference).unwrap();
        assert_eq!(
            fs::read_to_string(p.join("a.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "one\n"
        );
        assert_eq!(
            fs::read_to_string(p.join("b.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "two\n"
        );
        assert!(
            !p.join("c.txt").exists(),
            "a file the run created is removed"
        );
        assert_eq!(
            fs::read_to_string(p.join("d.txt")).unwrap(),
            "my own edit\n",
            "the person's later edit is kept"
        );
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].0, "d.txt");
        assert_eq!((report.restored.len(), report.removed.len()), (2, 1));
        assert_eq!(
            latest_pair(p, "B-4"),
            Some((before.reference, after.reference))
        );
        assert!(undoable_scopes(p).contains("B-4"));
    }

    #[test]
    fn preflight_refuses_a_merge_in_progress_and_lists_the_persons_changes() {
        let dir = repo();
        let p = dir.path();
        fs::write(p.join("a.txt"), "mine\n").unwrap();
        let info = preflight(p);
        assert!(info.repo && info.problem.is_none());
        assert_eq!(info.dirty, ["a.txt"]);
        let git_dir = PathBuf::from(run(p, &["rev-parse", "--absolute-git-dir"]));
        fs::write(git_dir.join("MERGE_HEAD"), run(p, &["rev-parse", "HEAD"])).unwrap();
        let info = preflight(p);
        assert!(
            info.problem
                .as_deref()
                .is_some_and(|m| m.contains("middle of a merge")),
            "{:?}",
            info.problem
        );
        assert!(RunGuard::begin(p, "B-5").unwrap_err().contains("merge"));
        fs::remove_file(git_dir.join("MERGE_HEAD")).unwrap();
        fs::create_dir_all(git_dir.join("rebase-merge")).unwrap();
        assert!(preflight(p).problem.unwrap().contains("rebase"));
    }

    #[test]
    fn a_folder_with_no_repository_runs_without_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let info = preflight(dir.path());
        assert!(!info.repo && info.problem.is_none());
        assert!(checkpoint(dir.path(), "B-6", "0001-before").is_err());
        let guard = RunGuard::begin(dir.path(), "B-6").unwrap();
        assert!(guard.before.is_none());
        let done = guard.finish("done");
        assert!(done.after.is_none() && done.note.is_none());
    }

    #[test]
    fn only_one_run_holds_the_lock_and_it_is_free_again_afterwards() {
        let dir = repo();
        let first = RunGuard::begin(dir.path(), "B-7").unwrap();
        let second = RunGuard::begin(dir.path(), "B-8").unwrap_err();
        assert!(second.contains("another Bridle"), "{second}");
        let done = first.finish("done");
        assert!(done.before.is_some() && done.after.is_some());
        let again = RunGuard::begin(dir.path(), "B-8").unwrap();
        drop(again);
        assert_eq!(list(dir.path(), "B-7").len(), 2);
    }

    #[test]
    fn a_run_cut_short_is_found_and_its_state_saved_at_the_next_start() {
        let dir = repo();
        let p = dir.path();
        let guard = RunGuard::begin(p, "B-9").unwrap();
        fs::write(p.join("a.txt"), "half done\n").unwrap();
        // The process dies: the guard is never finished (the lock goes with the handle).
        drop(guard);
        let found = recover(p);
        assert_eq!(found, ["B-9"]);
        let refs = list(p, "B-9");
        assert!(refs.iter().any(|r| r.ends_with("-after")), "{refs:?}");
        let (before, after) = latest_pair(p, "B-9").unwrap();
        let report = undo(p, &before, &after).unwrap();
        assert_eq!(
            report.restored,
            ["a.txt"],
            "the half-done edit can be undone"
        );
        assert!(
            recover(p).is_empty(),
            "a recovered run is not recovered twice"
        );
    }
}
