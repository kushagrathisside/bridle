# The small-model fleet

A **fleet** is a small group of modest models that are loaded onto the graphics processor together, while the large model is unloaded, so
that they can perform bounded tasks in parallel: reading files and answering a question about them, or condensing a long log. The fleet is
**off by default** and may be configured **only in the machine configuration**. The design, the measurements and the limits of the evidence
are set out in
[ADR-0022](../decisions/0022-the-fleet-is-small-models-loaded-together-and-a-deterministic-scheduler-swaps-them.md).

The reasoning is as follows. A graphics card with 8 GiB of memory cannot hold the 9B model and a fleet at the same time, so Bridle must
exchange one for the other. A deterministic scheduler, which is ordinary code and not a model, performs the exchange. It checks that the
fleet will fit before anything is stopped, counts the exchanges against a budget, and keeps a record, so that a crash cannot leave you
without a model.

## Configuration

Define the small models under `[models]`, each with a `[launch]` section so that Bridle can start it, and then add:

```toml
[fleet]
enabled = true
slots = ["manager", "worker"]   # one to four slots that run together
worker = "worker"               # one server with several task slots (launch.parallel)
manager = "manager"             # optional; it is loaded, but has no task yet
swap_budget = 2                 # exchanges a run may make (stop the large model, restore it)
parallel = 4                    # tasks at one time; launch.parallel of the worker should be at least this
```

`bridle config validate` checks these settings (rule V51). The pair that was tested is Granite-4.0-H-Micro as the manager and
Granite-4.0-H-1B, with four server slots, as the worker.

## Commands

```text
bridle fleet status       the slots, whether each is running, the video memory each needs, whether they fit, and any unfinished swap
bridle fleet up           record the swap, stop the GPU slots outside the fleet, and start the fleet
bridle fleet down         stop the fleet and start again whatever `up` stopped
bridle fleet recover      complete a swap that a crash interrupted (the same restoration as `down`)
bridle fleet run TASKS.json [--parallel N] [--out RESULTS.json] [--keep-up]
```

`fleet run` brings the fleet up, sends the tasks to the worker `parallel` at a time, prints each answer together with the speed-up, and
then restores the large model. The option `--keep-up` leaves the fleet loaded instead.

### A tasks file

```json
[
  {"id": "t1", "role": "explore", "task": "What does this module do? One sentence.", "paths": ["crates/harness-runtime/src/scheduler.rs"]},
  {"id": "t2", "role": "summarise", "task": "Condense this.", "text": "a long log or a note"}
]
```

The `role` is either `explore` or `summarise`. Each path is read through the same checks as the file tools: it must lie inside the
workspace, must not be a secret file, and is limited in size. An answer must be JSON of a fixed shape. Bridle validates it and prints it
labelled as **a small model's reading, not a verified fact**. A reference to an input that does not exist, or to a line that the input does
not have, is discarded and counted. Nothing that a small model returns is ever executed.

## How a swap is kept safe

1. **The fleet must fit.** Its estimated memory, plus 230 MiB for the desktop and a reserve of 768 MiB, must not exceed the card's memory.
   If it would, nothing is stopped.
2. **The record is written first.** The file `.bridle/swap.json` exists before the first process is stopped.
3. **Swaps are counted.** A swap beyond `swap_budget` is refused, with the reason given.
4. **A second `up` is refused** while a record exists, and the message names the command to run (`bridle fleet down`).
5. **Restoration is idempotent.** A slot that is already running is not started twice; if a slot cannot be started, the record is kept so
   that the restoration can be retried.
6. **A failed start is reversed.** If a slot of the fleet does not start, `up` restores what it stopped.

## Measurements

Eight tasks over this repository's own files completed in 7.1 s of wall-clock time for 26.0 s of work, four at a time, which is about 3.7
times faster than running them one after another; the large model was healthy again after 7.6 s. On the real models, a swap record existed
during the swap, a second `up` was refused, and `recover` restored the large model. The decision record gives the figures and states what
was **not** shown: the small models' answers were not graded, and one cited a line that does not exist (a citation of that kind is now
discarded).

## Limits

The scheduler is not yet used by the desktop board runner, and no manager model yet plans work. Separate working copies for parallel
members exist as [lanes](lanes.md). The fleet's answers have not been compared in quality with the large model's on real tasks.

## Related

- [Models](models.md) · [Configuration](configuration.md) · [Team and board](team-and-board.md) · [Lanes](lanes.md)
- [The plan, items F1 and F0.2](../spec/team-subagents-ideas-plan.md)
