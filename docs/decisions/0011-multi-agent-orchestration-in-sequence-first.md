# ADR-0011: Multi-agent orchestration: planner, worker and verifier in sequence first, on one task journal

- **Status:** Accepted (2026-10-31); the two open questions are decided below
- **Date:** 2026-10-31
- **Deciders:** Kushagra Srivastava

## Context

Claude Code and Codex run subagents: a sub-conversation with its own context and tool limits that the main agent calls.
Bridle has the parts for a multi-agent run but not the arrangement: the orchestrator's task journal (M3), the loop rules,
the subagent tier (M4b, ADR-0009, a bounded reader that never edits), and the slots (ADR-0010).

Two decisions already in place limit the design:

- **ADR-0004:** the orchestrator observes OpenCode's loop and steers it through hooks. It does not run the loop itself.
- **ADR-0009:** a subagent never edits, never runs a tool, and never sees the parent's transcript.

A multi-agent run in which a worker edits files changes both. That needs a decision, not an implementation.

On one 8 GB GPU only one model is loaded at a time. Two models working in parallel means one on the CPU (`micro`, slow) or
serialised swaps (about 20 s each). So the first version runs in sequence.

## Decision (proposed)

1. **Roles, not processes.** A run has three roles: `planner` (writes a short plan into the task state), `worker` (makes
   the edits for one plan step), and `verifier` (runs the checks and reports a verdict). Each role is a bounded sub-run
   with its own prompt, its own tool list, and its own iteration limit.
2. **One journal.** Every sub-run writes to the same task journal, with the role in each record. `agent trace` shows the
   whole run in order. No second state store.
3. **Sequence first.** Planner, then worker, then verifier, one at a time. Parallel workers wait for the sandbox
   decision (ADR-0012) and for measured need.
4. **The worker's tools go through the same policy.** A worker that edits is not a reader, so ADR-0009's rule does not
   cover it. Its calls go through the policy hook and the loop rules exactly as the main agent's do. No role gets a
   verdict the policy would not give.
5. **The verifier decides nothing.** It runs the configured verification steps and records the result. Whether the task
   is done stays with the orchestrator's rules.
6. **Where it runs.** In the chat first, which already calls the hook path. In OpenCode only after the chat version is
   measured, since ADR-0004 keeps OpenCode's own loop in charge there.

## Consequences

- The planner and verifier can be the micro slot or the `main` slot, chosen by ADR-0010's rules.
- Each sub-run adds a model call. The 30-call limit per turn becomes a limit per role.
- A worker that loops is stopped by the same loop rules; a worker cannot restart the run on its own.

## Decided (2026-10-31, by the owner)

- **Workers may edit files, through the policy.** Every worker call goes through the policy hook and the loop rules,
  the same as the main agent's calls. ADR-0009's reader-only rule stays for the `subagent` tool; a worker is a new role
  with its own decision record.
- **The verifier informs only.** It runs the configured checks and records the result. The orchestrator's rules decide
  whether the task is done.
