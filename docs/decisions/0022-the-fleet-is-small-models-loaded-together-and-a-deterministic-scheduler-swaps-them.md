# ADR-0022: The fleet is a few small models loaded together while the big one is unloaded; a deterministic scheduler swaps them

- **Status:** Accepted (2026-11-04); the scheduler, the swap record and a parallel worker are built and were run on the real models
- **Date:** 2026-11-04
- **Deciders:** Kushagra Srivastava

## Context

The plan ([team-subagents-ideas-plan.md](../spec/team-subagents-ideas-plan.md), F1 and F0.2) asks for small models that do bounded work
in parallel. The tested GPU has 8,188 MiB and the main model (Qwen3.5-9B) uses about 6.5 GiB, so the big model and a fleet cannot be
loaded together. A fleet therefore means **swapping**: stop the big model, load the small ones, work, load the big one again. Two
things make that risky if done by hand or by asking a model: a crash between "stopped" and "started again" leaves the person with no
model at all, and a model that decides when to swap can swap in a loop.

## Decisions

### 1. The scheduler is code, not a model

`harness_runtime::scheduler` holds the rules and starts and stops nothing, which keeps them testable:

- `plan_up(running, fleet)`: stop every running GPU slot that is not in the fleet, then start every fleet slot that is not running, in
  the fleet's order. A fleet slot that already runs is left alone.
- `plan_down(running, fleet, restore)`: stop the fleet's slots that run, then start the slots that were stopped, unless they already run
  (so a crash after the restore does not start anything twice).
- `Budget`: a run may make `fleet.swap_budget` swaps (default 2: one to stop the big model, one to put it back). A swap over the budget
  is refused with the reason.
- `fit`: the fleet's estimates, a desktop of 230 MiB and a reserve of 768 MiB must be within the GPU's memory, checked **before
  anything is stopped**. If it does not fit, nothing is touched and the message says by how much.

No model decides what to swap. A manager model may later *propose* work; it never starts or stops a process.

### 2. A write-ahead swap record

`.bridle/swap.json` (`id`, start time, `phase` up or restoring, the fleet's slots, the slots that were stopped) is written atomically
**before the first process is stopped** and removed only when the stopped slots run again. A second `fleet up` while a record exists
is refused, naming what to run. `bridle fleet down` and `bridle fleet recover` finish the restore from the record, and if the restore
cannot start a slot the record is kept so it can be retried. If a slot of the fleet fails to start, `up` puts things back itself.

### 3. One worker server with several slots

The worker is one `llama-server` started with `-np N` slots: one copy of the weights, N tasks at once, each with its own context
(`context / N`). This is cheaper than N servers, and the first measurement (progress.md, 2026-11-04) showed parallel slots scale well
on this GPU. The worker's `launch.parallel` should be at least `fleet.parallel`; validation warns when it is not (rule V51).

### 4. The existing sub-agent contract is reused

Fleet tasks are the `explore` and `summarise` roles that already exist (`harness_core::subagent`): the inputs are bounded, the answer
must be JSON in a fixed schema, references must name an input, and the parent reads the answer labelled as *a small model's reading, not a verified fact*.
Nothing a small model returns is executed. `bridle fleet run TASKS.json` sends a file of tasks to the worker `fleet.parallel` at a time
and reports each result and the speedup.

### 5. Machine configuration only

`[fleet]` (`enabled`, `slots`, `manager`, `worker`, `swap_budget`, `parallel`) is refused in a repository file (`config::trust`), and is
validated by rule V51: one to four slots that exist and have a `[launch]` section, a worker and (optionally) a manager among them, at
most 10 swaps, 1 to 16 tasks at once. The video-memory check in `models serve` lets slots of an enabled fleet run together, and
the estimator now reads per-layer key/value head counts, so a hybrid model (Granite, with few attention layers) is estimated from the
layers that keep a cache.

## Results on the tested machine

Manager Granite-4.0-H-Micro Q4_K_M (estimated 2,378 MiB), worker Granite-4.0-H-1B Q4_K_M with 4 slots (estimated 1,534 MiB), Qwen3.5-9B as
the big model, an RTX 4060 Laptop with 8,188 MiB:

- `fleet status`: both fit with about 3,278 MiB to spare.
- `fleet run` with eight tasks over this repository's own files: main stopped, manager and worker healthy after 2.0 and 1.5 s, eight tasks
  answered, none failed, **7.1 s of wall time for 26.0 s of work, four at once: about 3.7 times**, both fleet slots stopped, main
  healthy again after 7.6 s, record gone. After stopping main the GPU showed 0 MiB used.
- Crash recovery, run on the real models: after `fleet up` the record existed and `fleet status` named it; a second `fleet up` was
  refused; `fleet recover` stopped the fleet, started main again and cleared the record.

## What was not shown

- **The answers were not graded.** All eight were well-formed, but they were shallow, and one cited a line that does not exist
  (`leases.rs:4105` in a file of 526 lines). The reference check only verified the *name* of an input; it now also drops a line number the
  input does not have (counted and reported). It cannot check that an existing line says what the model claims. The inputs are not
  shown to the small model with line numbers, so it has to count; numbering them is the obvious next change and has not been tried.
- **No crash was forced mid-swap.** The record's presence during a swap and the recovery were run; killing Bridle between the two stops
  was not. The write-before-stop order is tested in code (`the_swap_record_is_written_before_and_survives_until_the_restore_is_done`).
- **The manager is started but has no job yet.** There is no model-based planning loop; the manager slot is loaded and measured only.
- **Not built:** worktrees for parallel engineers and an integration step, scheduling inside the desktop board runner, the larger
  Qwen3.5-4B manager (its download was not complete), and any quality comparison between the fleet and the 9B model on real tasks.
- **Downloaded models are slow here** (about 30 KB/s at the end of a download), which is why only the Granite pair was used.

## Related

- [The fleet (guide)](../guide/fleet.md)
- [Plan, section 13](../spec/team-subagents-ideas-plan.md#13-build-status)
- [ADR-0020](0020-managed-browser-is-a-policy-gated-playwright-worker.md) and [ADR-0021](0021-window-screenshots-start-the-program-and-capture-only-its-window.md) use the same machine-config-only rule.
