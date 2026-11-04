# Lanes: members working in parallel

A lane is a separate working copy of your project for one member. Two members in two lanes cannot step on each other while they work;
you see what each did as a diff and merge them yourself. The design, and where the idea comes from (AWS's Infinity Code), is in
[ADR-0023](../decisions/0023-notices-background-commands-and-lanes-from-infinity.md).

Lanes need the project to be a real Git repository. A folder without Git uses Bridle's shadow repository, which cannot have worktrees;
run `git init` first.

## The steps

```text
bridle lanes new engineer-1                 make a lane: a snapshot of the project as it is now, checked out in its own folder
bridle lanes new qa --from engineer-1       or start from another lane's unfinished work
bridle --workspace .bridle\lanes\engineer-1 ...     give the folder to a member (a chat, a run) as its workspace
bridle lanes diff engineer-1                the review: files, summary and patch against the snapshot the lane started from
bridle lanes integrate engineer-1 qa --dry-run      what merging them would change; nothing is written
bridle lanes integrate engineer-1 qa        merge them into the project, in this order
bridle lanes undo                           put the project back as it was before the last integration
bridle lanes remove engineer-1              delete a lane (folder, record, checkpoints)
bridle lanes list | path NAME
```

## What `new` makes

A snapshot of the working tree, **uncommitted changes included**, with Git-ignored files and secret-looking files left out (the same
rule as a checkpoint), checked out as a Git worktree in `.bridle\lanes\NAME\`. A lane therefore does not contain build folders,
`node_modules` or `.env`. It has its own `.bridle\` state, its own leases and its own run lock, so a member in a lane works as it would
in any project.

## What `integrate` does

1. Takes the project's run lock (so no team run writes at the same time) and snapshots **the project as it is now** (so work you did meanwhile is part of the left side).
2. Merges each lane onto that snapshot, in the order given, three ways, with `git merge-tree`, which writes only objects.
3. If any lane conflicts, **nothing is written**: it prints the lane and the files, and exits with an error. `--skip-conflicts`
   leaves that lane out and merges the rest.
4. Otherwise writes the merged state into the working tree, with the rule Undo uses: a file you changed after the snapshot is **left
   alone and listed**.
5. Checkpoints the project before and after, so `bridle lanes undo` can return it. `--remove` also deletes the lanes that merged.

The model cannot run any of this. It is the person's step.

## Things to know

- Two lanes that change different parts of one file merge; two that change the same lines conflict at the merge, not while they work.
- Line endings follow your Git settings: on Windows with `core.autocrlf` the merged files are written with CRLF.
- Git ignores `.bridle\`, which `bridle init` sets up; in a repository where it is not ignored, lane folders appear as untracked.
- The board runner does not give members lanes yet, and the window has no button for this; today it is these commands.

## What was and was not shown

Tests on real temporary Git repositories cover two lanes merging (including an uncommitted change at the start, a new file and a
deletion), a dry run, undo, a conflict that writes nothing, `--skip-conflicts`, work done in the project after the lane started, a
stacked lane and removal. The same flow was run through the built command line. **Not shown:** two real models working in two lanes at
once; a very large repository (a snapshot reads every tracked and untracked file); a repository with submodules.

## Related

- [Background commands](background-commands.md) · [Runs, checkpoints and recovery](../reference/run-recovery.md) · [Team and board](team-and-board.md)
