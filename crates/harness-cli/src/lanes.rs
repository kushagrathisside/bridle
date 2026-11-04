//! Lanes: separate working copies for members who work at the same time, merged by the person afterwards (ADR-0023).
//!
//! This is the part of Infinity Code that matters here: parallel edits happen in **separate sandboxes**, and each result comes back as
//! a **diff for review**, not as writes into the project. A lane is a Git worktree under `.bridle/lanes/<name>/`, started from a snapshot
//! of the project's working tree (uncommitted changes included, ignored and secret files left out, exactly like a checkpoint). A member
//! works in a lane by being given that folder as its workspace (`bridle --workspace .bridle/lanes/<name> ...`): it has its own leases,
//! its own run lock and its own `.bridle/`, so two members in two lanes never meet.
//!
//! * `diff` shows what a lane changed against the snapshot it started from.
//! * `integrate` merges lanes **three ways** (`git merge-tree`, which touches nothing) onto a fresh snapshot of the project, in the
//!   order given. If any lane conflicts, nothing is written (or, with `--skip-conflicts`, that lane is left out). Only then is the merged
//!   state written into the working tree, with the same rule as Undo: a file you changed since the snapshot is left alone and listed.
//!   The project is checkpointed before and after, so `lanes undo` puts it back.
//! * A lane can be started from another lane (`--from`), which is how one member builds on another's unfinished work.
//!
//! The model never integrates: this is the person's step (a command, and later a button). A folder that is not a real Git repository
//! (a shadow repository cannot have worktrees) cannot have lanes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use harness_core::identity;
use harness_runtime::process::hide_console;
use serde::{Deserialize, Serialize};

use crate::gitlayer::{self, UndoReport, git};

const AUTHOR_ENV: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Bridle"),
    ("GIT_AUTHOR_EMAIL", "bridle@localhost"),
    ("GIT_COMMITTER_NAME", "Bridle"),
    ("GIT_COMMITTER_EMAIL", "bridle@localhost"),
];

/// One lane's record, kept in `.bridle/lanes/<name>.lane.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    pub name: String,
    /// The snapshot commit the lane started from; its changes are measured against this.
    pub base: String,
    /// The lane this one was started from, if any.
    pub parent: Option<String>,
    pub created_unix_s: u64,
}

fn lanes_dir(workspace: &Path) -> PathBuf {
    identity::state_dir(workspace).join("lanes")
}

/// The folder a lane works in: give it to a member as its workspace.
pub fn lane_dir(workspace: &Path, name: &str) -> PathBuf {
    lanes_dir(workspace).join(name)
}

fn meta_path(workspace: &Path, name: &str) -> PathBuf {
    lanes_dir(workspace).join(format!("{name}.lane.json"))
}

/// A lane name: one to 32 characters, lowercase letters, digits, `-` and `_`, starting with a letter or digit.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn require_real_repo(workspace: &Path) -> Result<()> {
    let info = gitlayer::preflight(workspace);
    if !info.repo || info.shadow {
        bail!(
            "lanes need a Git repository of the project's own (a folder without Git uses a shadow repository, which cannot have worktrees); run `git init` first"
        );
    }
    Ok(())
}

pub fn load(workspace: &Path, name: &str) -> Result<Lane> {
    if !valid_name(name) {
        bail!("`{name}` is not a lane name (lowercase letters, digits, - and _; at most 32)");
    }
    let text = fs::read_to_string(meta_path(workspace, name))
        .map_err(|_| anyhow!("there is no lane named `{name}`; `bridle lanes list` shows them"))?;
    serde_json::from_str(&text).with_context(|| format!("the record of lane `{name}` is damaged"))
}

/// The lanes of the project, by name.
pub fn list(workspace: &Path) -> Vec<Lane> {
    let mut lanes: Vec<Lane> = fs::read_dir(lanes_dir(workspace))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let name = name.strip_suffix(".lane.json")?.to_owned();
            load(workspace, &name).ok()
        })
        .collect();
    lanes.sort_by(|a, b| a.name.cmp(&b.name));
    lanes
}

/// Starts a lane: a snapshot of the project (or of lane `from`), checked out as a worktree.
pub fn new_lane(workspace: &Path, name: &str, from: Option<&str>) -> Result<Lane> {
    if !valid_name(name) {
        bail!("`{name}` is not a lane name (lowercase letters, digits, - and _; at most 32)");
    }
    require_real_repo(workspace)?;
    let dir = lane_dir(workspace, name);
    if meta_path(workspace, name).exists() || dir.exists() {
        bail!("a lane named `{name}` exists already; remove it first or pick another name");
    }
    let base = match from {
        Some(parent) => {
            let parent = load(workspace, parent)?;
            let parent_dir = lane_dir(workspace, &parent.name);
            gitlayer::checkpoint(
                &parent_dir,
                &format!("lane-{}", parent.name),
                &format!("stack-{name}"),
            )?
            .commit
        }
        None => gitlayer::checkpoint(workspace, "lanes", &format!("{name}-base"))?.commit,
    };
    fs::create_dir_all(lanes_dir(workspace)).context("create the lanes folder")?;
    git(
        workspace,
        &[
            "worktree",
            "add",
            "--detach",
            "-q",
            &dir.display().to_string(),
            &base,
        ],
        &[],
    )?;
    let lane = Lane {
        name: name.to_owned(),
        base,
        parent: from.map(str::to_owned),
        created_unix_s: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    };
    let text = serde_json::to_string_pretty(&lane).context("write the lane record")?;
    fs::write(meta_path(workspace, name), text).context("write the lane record")?;
    Ok(lane)
}

/// The lane's state now, as a commit (a checkpoint of its folder; Git ignored and secret files left out).
pub fn result_commit(workspace: &Path, lane: &Lane) -> Result<String> {
    let dir = lane_dir(workspace, &lane.name);
    if !dir.is_dir() {
        bail!(
            "the folder of lane `{}` is missing ({}); remove the lane with `bridle lanes remove {}`",
            lane.name,
            dir.display(),
            lane.name
        );
    }
    Ok(gitlayer::checkpoint(&dir, &format!("lane-{}", lane.name), "result")?.commit)
}

/// What a lane changed against the snapshot it started from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneDiff {
    /// `(status, path)`, status `A`, `M` or `D`.
    pub files: Vec<(char, String)>,
    pub stat: String,
    pub patch: String,
}

pub fn diff(workspace: &Path, lane: &Lane) -> Result<LaneDiff> {
    let result = result_commit(workspace, lane)?;
    Ok(LaneDiff {
        files: gitlayer::changes(workspace, &lane.base, &result)?,
        stat: git(
            workspace,
            &["diff", "--stat", "--no-renames", &lane.base, &result],
            &[],
        )?,
        patch: git(
            workspace,
            &["diff", "--no-renames", &lane.base, &result],
            &[],
        )?,
    })
}

enum Merge {
    Clean(String),
    Conflict(Vec<String>),
}

/// A three-way merge of two commits that writes only objects (nothing in the working tree, the index or any branch).
fn merge_tree(workspace: &Path, base: &str, ours: &str, theirs: &str) -> Result<Merge> {
    let mut command = Command::new("git");
    command
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "-z",
            &format!("--merge-base={base}"),
            ours,
            theirs,
        ])
        .current_dir(workspace)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);
    let output = command.output().context("run git merge-tree")?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut parts = text.split('\0');
    let tree = parts.next().unwrap_or("").trim().to_owned();
    match output.status.code() {
        Some(0) if !tree.is_empty() => Ok(Merge::Clean(tree)),
        Some(1) => Ok(Merge::Conflict(
            parts
                .take_while(|p| !p.is_empty())
                .map(str::to_owned)
                .collect(),
        )),
        _ => bail!(
            "git merge-tree failed (it needs Git 2.38 or newer): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

/// What `integrate` did, or would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrateReport {
    /// Lanes that merged cleanly, in order.
    pub merged: Vec<String>,
    /// Lanes that conflict, with the conflicting files.
    pub conflicted: Vec<(String, Vec<String>)>,
    /// What the merge changes in the project: `(status, path)`.
    pub files: Vec<(char, String)>,
    /// What was written to the working tree (empty for a dry run); `skipped` are files you changed since the snapshot.
    pub applied: UndoReport,
    pub dry_run: bool,
    /// Why nothing was written, when that is so.
    pub note: Option<String>,
    /// The project's checkpoints around the integration (`lanes undo` uses them); none for a dry run.
    pub before_ref: Option<String>,
    pub after_ref: Option<String>,
}

fn drop_ref(workspace: &Path, reference: &str) {
    let _ = git(workspace, &["update-ref", "-d", reference], &[]);
}

fn next_stamp(workspace: &Path) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let taken = |stamp: &str| {
        git(
            workspace,
            &[
                "rev-parse",
                "--verify",
                "-q",
                &format!("{}/lanes/{stamp}-before", gitlayer::REF_ROOT),
            ],
            &[],
        )
        .is_ok()
    };
    let mut stamp = format!("int-{secs:012}");
    let mut n = 2;
    while taken(&stamp) {
        stamp = format!("int-{secs:012}-{n}");
        n += 1;
    }
    stamp
}

/// Merges the named lanes, in order, into the project. See the module docs.
pub fn integrate(
    workspace: &Path,
    names: &[String],
    skip_conflicts: bool,
    dry_run: bool,
) -> Result<IntegrateReport> {
    if names.is_empty() {
        bail!("name at least one lane to integrate");
    }
    require_real_repo(workspace)?;
    let lanes = names
        .iter()
        .map(|n| load(workspace, n))
        .collect::<Result<Vec<_>>>()?;
    let _lock = gitlayer::RepoLock::acquire(workspace).map_err(|e| anyhow!(e))?;
    let stamp = next_stamp(workspace);
    // The project as it is right now: both the left side of every merge and the point Undo returns to.
    let before = gitlayer::checkpoint(workspace, "lanes", &format!("{stamp}-before"))?;
    let mut current = before.commit.clone();
    let mut report = IntegrateReport {
        dry_run,
        ..IntegrateReport::default()
    };
    for lane in &lanes {
        let result = result_commit(workspace, lane)?;
        match merge_tree(workspace, &lane.base, &current, &result)? {
            Merge::Clean(tree) => {
                current = git(
                    workspace,
                    &[
                        "commit-tree",
                        &tree,
                        "-p",
                        &current,
                        "-m",
                        &format!("bridle lanes: merge {}", lane.name),
                    ],
                    &AUTHOR_ENV,
                )?;
                report.merged.push(lane.name.clone());
            }
            Merge::Conflict(files) => report.conflicted.push((lane.name.clone(), files)),
        }
    }
    let blocked = !report.conflicted.is_empty() && !skip_conflicts;
    if blocked {
        report.merged.clear();
        report.note = Some("some lanes conflict, so nothing was written; fix them in their lanes, or integrate the others with --skip-conflicts".to_owned());
    } else if report.merged.is_empty() {
        report.note = Some("there was nothing to merge".to_owned());
    } else {
        report.files = gitlayer::changes(workspace, &before.commit, &current)?;
        if report.files.is_empty() {
            report.note = Some("the lanes changed nothing".to_owned());
        }
    }
    if dry_run || blocked || report.merged.is_empty() || report.files.is_empty() {
        drop_ref(workspace, &before.reference);
        return Ok(report);
    }
    // The merged state replaces the snapshot, file by file, except where the project moved on since the snapshot.
    let mut applied = gitlayer::undo(workspace, &current, &before.commit)?;
    for (_, why) in &mut applied.skipped {
        *why = why.replace("it changed after the run", "you changed it in the project after the snapshot, so the lanes' version was not written");
    }
    report.applied = applied;
    let after = gitlayer::checkpoint(workspace, "lanes", &format!("{stamp}-after"))?;
    report.before_ref = Some(before.reference);
    report.after_ref = Some(after.reference);
    Ok(report)
}

/// Puts the project back as it was before the latest integration (files you changed since are left and listed).
pub fn undo_last(workspace: &Path) -> Result<UndoReport> {
    require_real_repo(workspace)?;
    let _lock = gitlayer::RepoLock::acquire(workspace).map_err(|e| anyhow!(e))?;
    let (before, after) = gitlayer::latest_pair(workspace, "lanes")
        .ok_or_else(|| anyhow!("there is no integration to undo"))?;
    gitlayer::undo(workspace, &before, &after)
}

/// Removes a lane: its worktree, its record and its checkpoints. Other lanes started from it keep their own snapshots.
pub fn remove(workspace: &Path, name: &str) -> Result<()> {
    let lane = load(workspace, name)?;
    let dir = lane_dir(workspace, name);
    if dir.exists() {
        git(
            workspace,
            &["worktree", "remove", "--force", &dir.display().to_string()],
            &[],
        )
        .or_else(|_| {
            fs::remove_dir_all(&dir)
                .map(|()| String::new())
                .map_err(anyhow::Error::from)
        })
        .with_context(|| format!("remove {}", dir.display()))?;
    }
    let _ = git(workspace, &["worktree", "prune"], &[]);
    for reference in gitlayer::list(workspace, &format!("lane-{name}")) {
        drop_ref(workspace, &reference);
    }
    drop_ref(
        workspace,
        &format!("{}/lanes/{name}-base", gitlayer::REF_ROOT),
    );
    if let Some(parent) = &lane.parent {
        drop_ref(
            workspace,
            &format!("{}/lane-{parent}/stack-{name}", gitlayer::REF_ROOT),
        );
    }
    fs::remove_file(meta_path(workspace, name)).context("remove the lane record")?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A file's text with Windows line endings read as `\n`: Git writes CRLF into a working tree on Windows (`core.autocrlf`), and what
    /// these tests check is the merge, not the line ending.
    fn read(path: impl AsRef<Path>) -> std::io::Result<String> {
        fs::read_to_string(path).map(|text| text.replace("\r\n", "\n"))
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        git(dir, args, &AUTHOR_ENV).unwrap()
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q"]);
        run(dir.path(), &["config", "commit.gpgsign", "false"]);
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
        fs::write(dir.path().join("b.txt"), "bee\n").unwrap();
        fs::write(dir.path().join("keep.txt"), "keep\n").unwrap();
        fs::write(dir.path().join(".gitignore"), ".bridle/\n").unwrap();
        run(dir.path(), &["add", "-A"]);
        run(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    fn files(report: &IntegrateReport) -> Vec<String> {
        let mut v: Vec<String> = report
            .files
            .iter()
            .map(|(k, p)| format!("{k} {p}"))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn names_are_checked() {
        for good in ["a", "x1", "engineer-2", "qa_1"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "-a", "A", "a b", "a/b", "..", &"x".repeat(33), "ü"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn two_lanes_edit_different_parts_and_both_come_back_merged_with_a_diff_to_review_first() {
        let dir = repo();
        let p = dir.path();
        // The project has an uncommitted change when the lanes start: both lanes begin from it.
        fs::write(p.join("keep.txt"), "keep, edited before the lanes\n").unwrap();
        let one = new_lane(p, "one", None).unwrap();
        new_lane(p, "two", None).unwrap();
        assert_eq!(
            read(lane_dir(p, "one").join("keep.txt")).unwrap(),
            "keep, edited before the lanes\n"
        );

        // Lane one edits the top of a.txt and adds a file; lane two edits the bottom of a.txt and deletes b.txt.
        fs::write(
            lane_dir(p, "one").join("a.txt"),
            "ONE\ntwo\nthree\nfour\nfive\n",
        )
        .unwrap();
        fs::write(lane_dir(p, "one").join("new.txt"), "new\n").unwrap();
        fs::write(
            lane_dir(p, "two").join("a.txt"),
            "one\ntwo\nthree\nfour\nFIVE\n",
        )
        .unwrap();
        fs::remove_file(lane_dir(p, "two").join("b.txt")).unwrap();

        // The review: each lane's diff, nothing in the project yet.
        let d = diff(p, &one).unwrap();
        assert_eq!(
            d.files,
            vec![('M', "a.txt".to_owned()), ('A', "new.txt".to_owned())]
        );
        assert!(
            d.patch.contains("-one") && d.patch.contains("+ONE"),
            "{}",
            d.patch
        );
        assert!(d.stat.contains("a.txt"), "{}", d.stat);
        assert_eq!(
            read(p.join("a.txt")).unwrap(),
            "one\ntwo\nthree\nfour\nfive\n"
        );

        // A dry run says what would happen and writes nothing.
        let dry = integrate(p, &["one".to_owned(), "two".to_owned()], false, true).unwrap();
        assert_eq!(dry.merged, ["one", "two"]);
        assert_eq!(files(&dry), ["A new.txt", "D b.txt", "M a.txt"]);
        assert!(dry.before_ref.is_none() && dry.applied.restored.is_empty());
        assert_eq!(
            read(p.join("a.txt")).unwrap(),
            "one\ntwo\nthree\nfour\nfive\n"
        );

        let done = integrate(p, &["one".to_owned(), "two".to_owned()], false, false).unwrap();
        assert!(
            done.conflicted.is_empty() && done.applied.skipped.is_empty(),
            "{done:?}"
        );
        assert_eq!(
            read(p.join("a.txt")).unwrap(),
            "ONE\ntwo\nthree\nfour\nFIVE\n",
            "both edits are in one file"
        );
        assert_eq!(read(p.join("new.txt")).unwrap(), "new\n");
        assert!(!p.join("b.txt").exists(), "lane two's deletion");
        assert_eq!(
            read(p.join("keep.txt")).unwrap(),
            "keep, edited before the lanes\n"
        );
        assert!(done.before_ref.is_some() && done.after_ref.is_some());

        // Undo puts the project back as it was just before the integration.
        let undone = undo_last(p).unwrap();
        assert!(undone.skipped.is_empty(), "{undone:?}");
        assert_eq!(
            read(p.join("a.txt")).unwrap(),
            "one\ntwo\nthree\nfour\nfive\n"
        );
        assert!(!p.join("new.txt").exists());
        assert_eq!(read(p.join("b.txt")).unwrap(), "bee\n");
    }

    #[test]
    fn lanes_that_change_the_same_lines_conflict_and_nothing_is_written_unless_the_conflict_is_skipped()
     {
        let dir = repo();
        let p = dir.path();
        new_lane(p, "x", None).unwrap();
        new_lane(p, "y", None).unwrap();
        new_lane(p, "z", None).unwrap();
        fs::write(
            lane_dir(p, "x").join("a.txt"),
            "X\ntwo\nthree\nfour\nfive\n",
        )
        .unwrap();
        fs::write(
            lane_dir(p, "y").join("a.txt"),
            "Y\ntwo\nthree\nfour\nfive\n",
        )
        .unwrap();
        fs::write(lane_dir(p, "z").join("b.txt"), "zee\n").unwrap();
        let names = ["x", "y", "z"].map(str::to_owned);

        let blocked = integrate(p, &names, false, false).unwrap();
        assert_eq!(blocked.conflicted.len(), 1, "{blocked:?}");
        assert_eq!(
            blocked.conflicted[0],
            ("y".to_owned(), vec!["a.txt".to_owned()])
        );
        assert!(
            blocked.merged.is_empty()
                && blocked
                    .note
                    .as_deref()
                    .is_some_and(|n| n.contains("nothing was written")),
            "{blocked:?}"
        );
        assert_eq!(
            read(p.join("a.txt")).unwrap(),
            "one\ntwo\nthree\nfour\nfive\n"
        );
        assert_eq!(read(p.join("b.txt")).unwrap(), "bee\n");
        assert!(
            gitlayer::latest_pair(p, "lanes").is_none(),
            "a blocked integration leaves no checkpoint pair"
        );

        let partial = integrate(p, &names, true, false).unwrap();
        assert_eq!(partial.merged, ["x", "z"]);
        assert_eq!(partial.conflicted[0].0, "y");
        assert_eq!(
            read(p.join("a.txt")).unwrap(),
            "X\ntwo\nthree\nfour\nfive\n"
        );
        assert_eq!(read(p.join("b.txt")).unwrap(), "zee\n");
    }

    #[test]
    fn work_the_person_did_in_the_project_after_a_lane_started_is_kept_in_the_merge() {
        let dir = repo();
        let p = dir.path();
        new_lane(p, "w", None).unwrap();
        fs::write(
            lane_dir(p, "w").join("a.txt"),
            "W\ntwo\nthree\nfour\nfive\n",
        )
        .unwrap();
        fs::write(lane_dir(p, "w").join("b.txt"), "from the lane\n").unwrap();
        // While the lane worked, the person edited a.txt in the project. The integration snapshots the project first, so this edit is
        // part of the left side: the merge keeps both (different lines), and nothing is lost.
        fs::write(
            p.join("a.txt"),
            "one\ntwo\nthree\nfour\nfive, by the person\n",
        )
        .unwrap();
        let done = integrate(p, &["w".to_owned()], false, false).unwrap();
        assert!(done.applied.skipped.is_empty(), "{done:?}");
        assert_eq!(
            read(p.join("a.txt")).unwrap(),
            "W\ntwo\nthree\nfour\nfive, by the person\n"
        );
        assert_eq!(read(p.join("b.txt")).unwrap(), "from the lane\n");
    }

    #[test]
    fn a_lane_can_build_on_another_and_removing_lanes_cleans_up() {
        let dir = repo();
        let p = dir.path();
        new_lane(p, "base-work", None).unwrap();
        fs::write(
            lane_dir(p, "base-work").join("a.txt"),
            "ONE\ntwo\nthree\nfour\nfive\n",
        )
        .unwrap();
        let stacked = new_lane(p, "on-top", Some("base-work")).unwrap();
        assert_eq!(stacked.parent.as_deref(), Some("base-work"));
        assert_eq!(
            read(lane_dir(p, "on-top").join("a.txt")).unwrap(),
            "ONE\ntwo\nthree\nfour\nfive\n",
            "it starts from the other lane's work"
        );
        fs::write(
            lane_dir(p, "on-top").join("a.txt"),
            "ONE\ntwo\nthree\nfour\nFIVE\n",
        )
        .unwrap();
        // Only the stacked lane's own change shows in its diff.
        let d = diff(p, &stacked).unwrap();
        assert!(
            d.patch.contains("+FIVE") && !d.patch.contains("+ONE"),
            "{}",
            d.patch
        );
        // Merged in order, the two come to the final state.
        let done = integrate(
            p,
            &["base-work".to_owned(), "on-top".to_owned()],
            false,
            false,
        )
        .unwrap();
        assert!(done.conflicted.is_empty(), "{done:?}");
        assert_eq!(
            read(p.join("a.txt")).unwrap(),
            "ONE\ntwo\nthree\nfour\nFIVE\n"
        );

        assert_eq!(
            list(p).iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["base-work", "on-top"]
        );
        remove(p, "on-top").unwrap();
        remove(p, "base-work").unwrap();
        assert!(list(p).is_empty());
        assert!(!lane_dir(p, "on-top").exists() && !lane_dir(p, "base-work").exists());
        assert!(
            gitlayer::list(p, "lane-base-work").is_empty()
                && gitlayer::list(p, "lane-on-top").is_empty()
        );
        assert!(load(p, "on-top").is_err());
        // The project's own files were not touched by removing lanes.
        assert_eq!(
            read(p.join("a.txt")).unwrap(),
            "ONE\ntwo\nthree\nfour\nFIVE\n"
        );
    }

    #[test]
    fn a_lane_needs_a_real_repository_a_new_name_and_a_good_name() {
        let plain = tempfile::tempdir().unwrap();
        let error = new_lane(plain.path(), "a", None).unwrap_err().to_string();
        assert!(error.contains("Git repository"), "{error}");
        let dir = repo();
        let p = dir.path();
        assert!(new_lane(p, "Bad Name", None).is_err());
        new_lane(p, "dup", None).unwrap();
        assert!(
            new_lane(p, "dup", None)
                .unwrap_err()
                .to_string()
                .contains("exists already")
        );
        assert!(new_lane(p, "child", Some("missing")).is_err());
        assert!(integrate(p, &[], false, true).is_err());
        assert!(integrate(p, &["missing".to_owned()], false, true).is_err());
        assert!(undo_last(p).is_err(), "nothing was integrated");
    }
}
