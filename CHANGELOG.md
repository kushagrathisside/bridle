# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The project has not been published; versions are for the repository only.

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
- Benchmark runner and corpus, with a report (M10). The live results are not yet recorded.
- A checklist of every stage, checked against the code by `scripts/check_checklist.py`.
- Control Center: `agent tui`, a terminal application with a chat on the home screen and views of tasks, events,
  decisions, slots, diagnostics, configuration and destinations. The chat sends every tool call through the hook's policy
  path; the views are read-only and write nothing.
- Licence (Apache-2.0), NOTICE, third-party licence list, security and contributing guides, and CI (M11).

### Known limits

- The live checks of Jev and of the LiteLLM gateway were not run: no Jev service was available, and the gateway
  check waits for approval.
- A recursive search can read a secret file without naming it (security.md, residual risk 1).
- OpenCode's model-catalog fetch and the model downloads are outside the harness's control.
