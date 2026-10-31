# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The project has not been published, so the version below is for the repository only.

## [Unreleased] - 0.1.0 candidate

### Added

- Harness core: configuration with validation rules V1 to V36, a path guard, a command-risk classifier, a tool policy,
  output redaction, and a fail-closed hook for OpenCode 1.19.3 (M0 to M2).
- Deterministic MCP tools: git, code navigation (tree-sitter), dependency search, verification with failure parsing,
  and results (M2).
- Orchestrator: a task journal, loop rules with the rule named in each refusal, worktree tasks, and the `task`,
  `explain` and `trace` commands (M3).
- The micro model decides stalled loops, with a rules fallback and an A/B evaluation command (M4).
- The deep model on demand, with one GPU model at a time (M5).
- Context management: per-request compaction of tool results and the task state in the system prompt (M6).
- The Jev decision backend behind an endpoint policy, with TLS through the environment proxy (M7).
- A LiteLLM gateway as a model provider, with a hybrid profile (M8).
- Security review: an adversarial suite of 72 cases, fixes for short names, escapes, glob patterns and environment
  dumps, and `agent egress` (M9).
- Subagent tier: a small model reads a few bounded, redacted files and answers with checked citations (M4b).
- Repository memory: an `AGENTS.md` file (or the file `[memory]` names) is added to the system prompt, read through the
  tool policy, redacted and bounded (validation rule V41).
- Images in the chat: `/image <path> [question]` sends a PNG, JPEG, GIF or WebP from the workspace to a slot whose
  `capabilities.vision` is on; the file is read through the policy and limited to 4 MiB.
- Plugins (ADR-0013): manifests in the machine config declare tools that run a fixed program. Each tool's class is the
  stricter of its declared class and the command classifier's; only read-only and safe-execution tools are offered, and
  `agent plugins` lists them.
- Linux sandbox for commands (ADR-0016): Landlock on Linux, on by default there, refusing rather than running unconfined.
- Team run in the chat (`/team <request>`): a planner, a worker for each step, and a verifier, in sequence, with each
  role's end recorded in the task journal (ADR-0011).
- Routing: each chat request sent to the default slot is placed by rule, and the rest by the Laya decision model on this
  machine, with `main` as the fallback and `deep` used only when it is running. `agent route` shows the choice (V42 to
  V44; ADR-0010, ADR-0015).
- Benchmark runner and corpus, with a report (M10). The live results are not yet recorded.
- A checklist of every stage, checked against the code by `scripts/check_checklist.py`.
- Control Center: `agent tui`, a terminal application with a chat on the home screen and views of tasks, events,
  decisions, slots, diagnostics, configuration and destinations. The chat sends every tool call through the hook's policy
  path; the views are read-only and write nothing.
- Licence (Apache-2.0), NOTICE, third-party licence list, security and contributing guides, and CI (M11).

### Known limits

- The release runs on native Windows and is verified on one machine.
- Bridle is not a sandbox. See [security](docs/concepts/security.md).
- The live checks of the Jev service and the LiteLLM gateway are not part of the verified results.
- `security.require_worktree` and `verification.required_for_done` are read by the configuration checks only.
- OpenCode's model catalogue fetch and the model downloads are outside Bridle's control.
