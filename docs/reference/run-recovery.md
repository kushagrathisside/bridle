# Runs, checkpoints and recovery

When a team member works on an issue, Bridle protects your files with a **Git layer**: a lock, a checkpoint before the run and one
after it, and a journal. This page says exactly what it does to your repository, and how to inspect, undo and clean up by hand with
plain Git if Bridle itself is broken. The design and the cases it covers are in
[the plan](../spec/team-subagents-ideas-plan.md#36-what-can-fail-and-what-happens).

## What Bridle does to your repository

| When | What | Touches your branch, index or stash? |
|---|---|---|
| A member's turn on an issue starts | Takes the project lock (`.bridle/repo.lock`), checks the repository, saves a **checkpoint** `refs/bridle/runs/<issue>/NNNN-before`, and writes `start` to `.bridle/runs/<issue>/journal.jsonl` | No |
| The turn ends (done, stopped, failed or out of steps) | Saves `refs/bridle/runs/<issue>/NNNN-after`, writes `end` to the journal, adds a comment to the issue naming the files that changed, and releases the lock | No |
| The window opens a project | A journal with a `start` and no `end` was cut short (a crash, or a close): the files are saved as an `-after` checkpoint and the journal is closed as `interrupted`. The issue goes back to ready with a comment | No |
| You press **Undo changes** on an issue | Puts back the files the latest run changed, from its two checkpoints | Only the working files the run changed |

Bridle does **not** commit, switch branches, reset, stash or push on your branch. A checkpoint is a commit object that no branch points to:
it does not appear in `git log`, and `git push` does not send it (only `git push --mirror` would). It is kept alive by its ref, so `git gc`
leaves it. It holds the working tree as it was, including untracked files; files that Git ignores, secret-looking files (`.env`,
`*.pem`, `*.key`, `id_rsa*`, `.netrc`, `.npmrc`) and the `.bridle`, `.local-agent` and `.worktrees` folders are left out. The author is
`Bridle <bridle@localhost>`.

### When a run will not start

| Situation | Behaviour |
|---|---|
| Another Bridle window (or the command line) holds `.bridle/repo.lock` | The run is refused: "another Bridle is running a team in this project". The lock is released by the system if that program dies. |
| The repository is in the middle of a merge, rebase, cherry-pick, revert or bisect, or has paths in conflict | The run is refused, with the reason. Finish or abort the operation first. |
| You have uncommitted changes | The run goes ahead. They are in the "before" checkpoint, so nothing is lost. |
| The folder is not a Git repository | The run goes ahead **without checkpoints** (so Undo is not available). The journal is still written. |
| A repository with no commit yet | Works. The checkpoint has no parent. |

### What Undo does

For each file that differs between the "before" and "after" checkpoints:

| The run | Undo | If you changed the file after the run |
|---|---|---|
| Modified it | Restores the "before" content | Left alone, and listed |
| Created it | Deletes it | Left alone, and listed |
| Deleted it | Restores it | Left alone if it exists again, and listed |

Undo takes the project lock, so it will not run while a turn is working. It adds a comment to the issue saying what it did. It covers
changes made through Bridle's file tools **and** changes made by commands the member ran (the comparison is of the whole tree). It does
not cover files over the ignore rules or the secret-looking names above, because those are never saved.

## By hand

Everything above can be inspected and undone with Git. Replace `B-a3f8.1` with the issue id and `0001` with the run number.

| To | Run |
|---|---|
| List what Bridle saved | `git for-each-ref refs/bridle/` |
| List the checkpoints of one issue | `git for-each-ref refs/bridle/runs/B-a3f8.1/` |
| See what a run changed | `git diff --stat refs/bridle/runs/B-a3f8.1/0001-before refs/bridle/runs/B-a3f8.1/0001-after` |
| See the change in one file | `git diff refs/bridle/runs/B-a3f8.1/0001-before refs/bridle/runs/B-a3f8.1/0001-after -- path/to/file` |
| Compare a checkpoint with your working copy | `git diff refs/bridle/runs/B-a3f8.1/0001-after -- path/to/file` |
| Restore one file as it was before the run | `git restore --source=refs/bridle/runs/B-a3f8.1/0001-before --worktree -- path/to/file` |
| Print a file as it was in a checkpoint | `git show refs/bridle/runs/B-a3f8.1/0001-before:path/to/file` |
| Drop one checkpoint | `git update-ref -d refs/bridle/runs/B-a3f8.1/0001-before` |
| Drop all of an issue's checkpoints | `git for-each-ref --format="%(refname)" refs/bridle/runs/B-a3f8.1/` and `git update-ref -d` for each |
| See a run's journal | `type .bridle\runs\B-a3f8.1\journal.jsonl` |
| Free a stuck project lock | Close every Bridle window. The lock is an operating-system lock and goes with the program; the file `.bridle/repo.lock` itself is harmless and can stay |
| Take a stale `index.lock` away | First check that no Git process runs (`tasklist /FI "IMAGENAME eq git.exe"`), then delete `.git\index.lock`. Bridle never deletes it for you |

A checkpoint is made like this (Bridle does the same, with a temporary index so yours is not touched):

```bat
set GIT_INDEX_FILE=%TEMP%\my-index
git read-tree HEAD
git add -A
for /f %t in ('git write-tree') do git commit-tree %t -p HEAD -m "my checkpoint"
```

## Not covered yet

Leases on files (two members editing one file at once), worktrees for parallel engineers, a stale-read check on writes, and the
small-model scheduler are planned, not built. Members work **one at a time** in your working copy, so two members never edit at once; the
lock stops a second Bridle from starting a run. See [the plan](../spec/team-subagents-ideas-plan.md).
