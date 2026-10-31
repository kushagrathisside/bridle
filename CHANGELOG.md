# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The project has not been published, so the version below is for the repository only.

## [Unreleased] - 0.1.0 candidate

### Added

- A configuration loader with trust rules for repository files, and 40 validation rules (V1 to V40) reported by
  `agent config validate`. The shipped profiles are `local-only` and `development`, and `local-with-jev` and `hybrid`
  are for the machine configuration.
- A hook for OpenCode 1.19.3 that fails closed. Every tool call is checked against the policy, and tool output is
  redacted before the model sees it.
- A command classifier, a path guard, secret-file rules and output redaction. An adversarial suite of 72 cases runs in CI's
  test job.
- An MCP server with 20 default tools: git inspection, code navigation with tree-sitter, dependency search, verification
  with failure parsing, and inspection of artifacts and experiments. Configured adapters add commands as tools.
- A task journal, loop rules that name the rule in each refusal, worktree tasks, and the `agent task`, `agent explain`
  and `agent trace` commands.
- An optional micro model that decides stalled loops, with deterministic rules as the fallback.
- An optional deep model that loads on demand. Only one GPU model runs at a time.
- Context management: older tool results are shortened and then dropped as the context grows, and the task state is added
  to the system prompt.
- Optional decision and model backends behind the policy, all off by default: the Jev decision service, a LiteLLM gateway,
  and a subagent tool that reads a few bounded, redacted inputs and answers with citations.
- `agent egress`, which lists the outbound destinations that the configuration allows.
- A benchmark runner and a task corpus. The results are not yet summarised; see the [project status](docs/project/status.md) page.
- An Apache-2.0 licence, a NOTICE file, a third-party licence list, and a CI workflow.

### Known limits

- The release runs on native Windows and is verified on one machine.
- Bridle is not a sandbox. See [security](docs/concepts/security.md).
- The live checks of the Jev service and the LiteLLM gateway are not part of the verified results.
- `security.require_worktree` and `verification.required_for_done` are read by the configuration checks only.
- OpenCode's model catalogue fetch and the model downloads are outside Bridle's control.
