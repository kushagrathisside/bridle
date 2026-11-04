# Lanes: members working in parallel

A **lane** is a separate working copy of your project, given to one member. Two members who work in two lanes cannot interfere with each
other while they work. You review what each of them did as a diff, and you merge the results yourself. The design, and the source of the
idea (the coding agent in AWS's Infinity), are described in
[ADR-0023](../decisions/0023-notices-background-commands-and-lanes-from-infinity.md).

Lanes require the project to be a real Git repository. A folder without Git uses Bridle's shadow repository, which cannot have worktrees;
run `git init` in the folder first.

## The procedure

```text
bridle lanes new engineer-1                 make a lane: a snapshot of the project as it is now, checked out in its own folder
bridle lanes new qa --from engineer-1       or start from another lane's unfinished work
bridle --workspace .bridle\lanes\engineer-1 ...     give the folder to a member (a chat or a run) as its workspace
bridle lanes diff engineer-1                the review: files, a summary and the patch, against the snapshot the lane began from
bridle lanes integrate engineer-1 qa --dry-run      show what merging them would change; nothing is written
bridle lanes integrate engineer-1 qa        merge them into the project, in this order
bridle lanes undo                           restore the project to its state before the last integration
bridle lanes remove engineer-1              delete a lane (its folder, record and checkpoints)
bridle lanes list | path NAME
```

## What `new` creates

A snapshot of the working tree is taken, **including uncommitted changes**, but excluding files that Git ignores and files that look like
secrets (the same rule as a checkpoint). It is checked out as a Git worktree in `.bridle\lanes\NAME\`. A lane therefore does not contain
build folders, `node_modules` or `.env`. It has its own `.bridle\` state, its own leases and its own run lock, so a member works in a lane
exactly as in any other project.

## What `integrate` does

1. It takes the project's run lock, so that no team run is writing at the same time, and then snapshots **the project as it is now**. Work
   that you have done in the meantime therefore forms part of the left-hand side of the merge.
2. It merges each lane onto that snapshot, in the order given, by a three-way merge (`git merge-tree`), which writes objects only.
3. If any lane conflicts, **nothing is written**. The command prints the lane and the files concerned and exits with an error. With
   `--skip-conflicts`, the conflicting lane is omitted and the remainder are merged.
4. Otherwise it writes the merged state into the working tree, under the rule that Undo uses: a file that you changed after the snapshot is
   **left alone and listed**.
5. It checkpoints the project before and after, so that `bridle lanes undo` can restore it. The option `--remove` also deletes the lanes
   that were merged.

The model cannot run any of these commands. Integration is a step that only the person takes.

## Points to note

- Two lanes that change different parts of one file merge cleanly. Two that change the same lines conflict at the merge, and not while the
  members are working.
- Line endings follow your Git settings: on Windows with `core.autocrlf`, merged files are written with CRLF line endings.
- Git ignores `.bridle\`, which `bridle init` arranges. In a repository where it is not ignored, the lane folders appear as untracked.
- The board runner does not yet give members lanes, and the window has no button for integration. At present the facility is available
  through these commands.

## Evidence

Tests on real temporary Git repositories cover two lanes merging (including an uncommitted change at the start, a new file and a deletion),
a dry run, undo, a conflict that writes nothing, `--skip-conflicts`, work done in the project after a lane began, a stacked lane, and
removal. The same sequence was run through the built command line. Two real model turns (Qwen3.5-9B: one asked to add a function to
`calc.py`, the other a line to `README.md`) were run at the same time in two lanes with `bridle eval-agent lanes`; each touched only its own
file, and `integrate` merged them without a conflict. One model server answered the two turns in succession, so this demonstrates isolation
and merging, not a gain in speed. It has **not** been shown on a very large repository (a snapshot reads every tracked and untracked file),
or on a repository with submodules.

## Related

- [Background commands](background-commands.md) · [Runs, checkpoints and recovery](../reference/run-recovery.md) · [Team and board](team-and-board.md)
