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
| The folder is not a Git repository | Bridle makes a **shadow repository**, `.bridle/shadow.git`, and keeps the same checkpoints in it (see below). No `.git` appears in your folder. If git itself is missing the run goes ahead without checkpoints. |
| A repository with no commit yet | Works. The checkpoint has no parent. |

### A folder with no Git repository: the shadow repository

The first run in such a folder creates `.bridle/shadow.git` (an ordinary Git directory, used with `GIT_DIR` and `GIT_WORK_TREE`, so your
folder gets no `.git`). Checkpoints, Undo and recovery then work as in a repository. Its own ignore list leaves out `.bridle/`,
`node_modules/`, `target/`, `.venv/`, `venv/`, `__pycache__/`, `dist/`, `build/` and `*.log`, and a `.gitignore` in your folder is honoured too.
A file over **5 MB** is listed in the journal and not copied. A folder holding more than **200 MB** of files stops the run before it starts,
with the reason: put the project under Git or add a `.gitignore`. If you later run `git init` there, the real repository is used from then
on and the shadow is left alone (inspect it with `git --git-dir=.bridle/shadow.git --work-tree=. ...`, delete it by deleting the folder).
Worktrees (parallel engineers) are not available without a real repository.

### Leases and the stale-write check

While a run is active, the file tools (`write_file`, `edit_file`) claim a **lease** on each file they write, in `.bridle/leases.json`
(changed under `.bridle/leases.lock`, atomically). One run holds a path at a time; a path is the same however it is spelled (`Src\Lib.rs` and
`src/lib.rs`); a lease ends when the run ends, or after 15 minutes if the run died. A write to a path another run holds is refused with who holds
it. Members work one at a time today, so this protects against a lease left by a run that died and is what a second writer needs.

Separately, `read_file` shows a hash of what it read (`(sha256 ...)`), the file tools remember it, and a later `write_file` or `edit_file` of
that file is **refused when it has changed since** (you typed in the editor, a formatter ran): "changed since you read it; read it again".
`write_file` and `edit_file` also take `expected_sha256` (at least 8 hex digits of the hash shown). A file that was never read can still be written.

### Repair

`bridle doctor --repair` puts right what a crash leaves: it ends leases (only when no run is alive), closes runs cut short (saving the state of
the files as a checkpoint), removes `bridle-index-*` files from the Git directory, `.part-*` files of unfinished downloads and screenshots, old
`*.tmp-*` files in `.bridle`, browser session folders of processes that are gone, and background commands whose Bridle is gone (stopped after checking that the process id still belongs to that program). It says what it did, does nothing to a live run (it skips
those steps when another Bridle holds the project lock), and is safe to repeat.

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

Worktrees for parallel engineers, the integration step (`merge-tree`, Apply), the user's unsaved editor buffers as leases, and the
small-model scheduler are planned, not built. Members work **one at a time** in your working copy, so two members never edit at once; the
lock stops a second Bridle from starting a run. See [the plan](../spec/team-subagents-ideas-plan.md).
