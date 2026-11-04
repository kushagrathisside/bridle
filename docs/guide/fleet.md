# The small-model fleet

A fleet is a few **small** models that run together on the GPU while the big one is unloaded, doing bounded work in parallel: reading
files and answering a question about them, or condensing a long log. It is **off by default** and **machine-configuration only**. The
design, the measurements and what is not shown are in
[ADR-0022](../decisions/0022-the-fleet-is-small-models-loaded-together-and-a-deterministic-scheduler-swaps-them.md).

The idea in one line: an 8 GiB card cannot hold the 9B model and a fleet, so Bridle swaps, and a deterministic scheduler (code, not a
model) does the swapping, counts it, checks that things fit first, and keeps a record so that a crash cannot leave you with no model.

## Turn it on

In the machine configuration, define the small models under `[models]` (each with a `[launch]` section so Bridle can start it), then:

```toml
[fleet]
enabled = true
slots = ["manager", "worker"]   # one to four slots that run together
worker = "worker"               # one server with several task slots (launch.parallel)
manager = "manager"             # optional; loaded, but it has no job yet
swap_budget = 2                 # swaps a run may make (stop the big model, put it back)
parallel = 4                    # tasks at once; the worker's launch.parallel should be at least this
```

`bridle config validate` checks it (rule V51). The tested pair was Granite-4.0-H-Micro (manager) and Granite-4.0-H-1B with four slots
(worker).

## Commands

```text
bridle fleet status       the slots, whether each runs, what they need in video memory, whether they fit, any unfinished swap
bridle fleet up           record the swap, stop the GPU slots that are not in the fleet, start the fleet
bridle fleet down         stop the fleet and start again what `up` stopped
bridle fleet recover      finish a swap that a crash cut short (the same restore as `down`)
bridle fleet run TASKS.json [--parallel N] [--out RESULTS.json] [--keep-up]
```

`fleet run` brings the fleet up, sends the tasks to the worker `parallel` at a time, prints each answer and the speedup, and puts the big
model back. `--keep-up` leaves the fleet loaded.

### A tasks file

```json
[
  {"id": "t1", "role": "explore", "task": "What does this module do? One sentence.", "paths": ["crates/harness-runtime/src/scheduler.rs"]},
  {"id": "t2", "role": "summarise", "task": "Condense this.", "text": "a long log or a note"}
]
```

`role` is `explore` or `summarise`. Paths are read through the same checks as the file tools (inside the workspace, no secret files,
bounded size). An answer is JSON in a fixed shape; Bridle checks it and prints it labelled **a small model's reading, not a verified
fact**. A reference to an input that is not there, or to a line the input does not have, is dropped and counted. Nothing a small model
returns is ever run.

## How a swap stays safe

1. **It must fit.** The fleet's estimates plus 230 MiB for the desktop plus a 768 MiB reserve must be within the GPU. If not, nothing is stopped.
2. **The record comes first.** `.bridle/swap.json` is written before the first process is stopped.
3. **Counted.** More than `swap_budget` swaps is refused with the reason.
4. **A second `up` is refused** while a record exists; it tells you to run `bridle fleet down`.
5. **Restore is idempotent.** A slot that already runs is not started twice; if a slot cannot be started the record is kept for a retry.
6. **A failed start puts things back.** If a fleet slot does not start, `up` restores what it stopped.

## What was measured

Eight tasks over this repository's files: 7.1 s of wall time for 26.0 s of work, four at once, about 3.7 times faster than one at a
time; the big model was back in 7.6 s. On the real models, a swap record existed during the swap, a second `up` was refused, and `recover`
restored the big model. See the ADR for the numbers and for what was **not** shown: the answers were not graded, and one cited a line
that does not exist (that kind of citation is now dropped).

## Not built

Worktrees for parallel engineers and an integration step, the scheduler inside the desktop board runner, a manager that plans work, and
a quality comparison against the big model on real tasks.

## Related

- [Models](models.md) · [Configuration](configuration.md) · [Team and board](team-and-board.md)
- [Plan, F1 and F0.2](../spec/team-subagents-ideas-plan.md)
