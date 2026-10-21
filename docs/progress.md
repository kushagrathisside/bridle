# Progress log

A running record. Newest entries first. Every "verified" line names the evidence.

## 2026-10-21 (later): docs, hook fixtures, MCP run

- MCP verified through OpenCode: `local-agent_git_status` was called, allowed as READ_ONLY, completed in 137 ms, and the model answered from its output (`run3.out`, `tool-events.jsonl`).
- Gap found: the after-hook saw 0 output bytes for the MCP call, while built-in tools showed real sizes. So the shim's redaction does not reach MCP output (security.md, residual risk 7). Needs a fix or a documented limit.
- Model accuracy: the status answer said "6 untracked files"; git reports 4. The branch and the modified file were correct. Tool output is deterministic, so prefer quoting the structured result over counting.
- Serve/prefill: the server processed requests of about 8,196 tokens (OpenCode's system prompt and tool schemas). Prefill cost matters for latency; measure before M3.

- Docs written: README, architecture, installation, configuration, security, models, troubleshooting.
- In progress: hook fixture suite (34 black-box cases, `crates/harness-cli/tests/hook_fixtures.rs`) by a Haiku subagent; MCP `git_status` run through OpenCode.
- The background task "Remove todo in command_risk.rs" in another session is stale: the `todo!()` was removed when the classifier merged. Stop it.
- Found: the Qwen server did not answer health probes while its one slot was generating. A long single generation (about 2,800 tokens) was in progress during the MCP run. Investigate reasoning length.

## 2026-10-21: end-to-end on Windows

Verified (evidence in `D:\e2e\`):

- OpenCode 1.19.3 loads the shim plugin from `.opencode/plugins/` (only with the proxy variables cleared).
- A headless run fixed the toy repo's bug: `add()` now adds, and `python -m unittest -q` reports 2 passed. Every tool call went through the hook and was logged (`run1.out`, `tool-events.jsonl`).
- A read of a planted secret `.env` was denied by the `secret_file` rule; the model said so. The secret value is absent from the event log and the run output (`run2.out`).
- The full health check passes in 2.5 s: streaming (98 ms to first token), tool call, structured output, context 16384 (`agent health main`).
- Quick health, serve and stop work through the harness with the proxy fix.

Fixed on the way (each has a regression test):
- Loopback HTTP clients ignored the environment proxy (`http.rs`).
- Speculative decoding detected as active for `"none"` (`health.rs`).
- Windows commands were lexed as POSIX, breaking `D:\...` paths (`command_risk.rs`, `host_default`).
- Shell arguments were not checked against secret files or the workspace (`tool_policy.rs`).
- Ask verdicts were allowed (`opencode_v1.rs`, decision D3).
- The launcher emitted `--no-mmap`, which b11756 lacks (`launcher.rs`).
- The default prompt cache (8 GB) would have starved the 16 GB host (profiles).

Gates at this point: `cargo test --workspace` passes on WSL and on native Windows; `cargo clippy --workspace --all-targets -- -D warnings` passes; `cargo fmt --check` passes; `rg 'todo!('` finds nothing; `npm run typecheck` in `opencode-plugin/` passes against `@opencode-ai/plugin` 1.19.3.

Known gaps:
- Hook payloads: 6 checked by hand, plus the runs. The 34-case suite is in progress.
- MCP tools: unit-tested; not yet called through OpenCode (MCP run in progress).
- `doctor` with a missing model: not separately tested.
- Verification timeouts leave grandchild processes running.
- `extra_safe` can downgrade a WRITE command (policy review needed).
- Recursive `grep` over the repo can print secret values into context; output redaction untested for this case.
- The subagent code was checked by gates and targeted tests, not read line by line. A full review pass is still owed (decision D-3).

## 2026-10-21: merges and decisions

- Merged the reviewed subagent branches: redactor (`e6359cb`), launcher argv (`a433689`), config profiles and validation (`c53f09b`), TypeScript shim (`919052c`), providers (`e413970`), MCP server (`b8df47a`), doctor/health/models (`700f142`), command classifier (`505ffd5`).
- Decision: release 1 is Windows-only; WSL moves to release 2 (ADR-0002 amendment).
- Decision: classifier default on Windows is the `cmd` lexer (`ShellKind::host_default`).
- Decision: profiles set an explicit load mode, one parallel slot and a 1 GiB prompt cache for the main model.

## 2026-10-21: Wave 0 and the POC

- uv cache removed (23 GB, approved); repo backed up with `git bundle` (verified); worktrees checked; uncommitted subagent work saved as patches.
- Windows build baseline: builds with MSVC in about 34 s. Offline build via vendored crates (cargo on Windows follows the git-config proxy).
- P1 (Windows POC) passed: Qwen healthy in 11 s, `READY`, correct tool call, VRAM 5.4 of 8.2 GB, host free RAM 382 MB while serving.

## 2026-10-21: planning

- Four read-only audits (core, OpenCode host, inference runtime, delivery) and an Opus review of the plan. Plan approved after the review and the decisions in its section 9.
- Plan and decisions are in `~/.ai/plans/make-a-plan-first-humble-robin.md` (outside the repo).

## 2026-10-21: baseline

- Brief saved verbatim (`docs/spec/master-development-plan.md`). Requirements matrix with directives D-1 to D-5 (`docs/spec/requirements-traceability.md`). M0 audit (`docs/audit/`). ADR-0001 (Rust core, thin shim). Git repo initialised on branch `m1-basic-integration`.
