# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The project has not been published, so the version below is for the repository only.

## [Unreleased] - 0.1.0 candidate

### Added

- The window shows a crew rail (an avatar for each member of the team), a board popup (issues by state, with the saved ideas as the
  first column, a detail panel and a runner), `@` tags for a member, a role or a team, and an ideas queue. The roster is `team.json`
  in the machine config folder; the board is `.bridle/board/events.jsonl`; ideas are `.bridle/ideas.json`. Members work through
  harness board tools (`board_ready`, `board_list`, `board_show`, `board_create`, `board_claim`, `board_comment`, `board_update`)
  whose use is limited by role, and classed by the policy as harness tools. See [the guide](docs/guide/team-and-board.md).
- Web research (ADR-0019), off by default and machine-config only: `[web]` settings (rule V48), the tools `web_search`, `web_open`,
  `web_download` and `document_inspect`, policy-checked inside each tool, an address guard that refuses this machine's own network at
  every redirect, SearXNG and Brave providers with reciprocal rank fusion, a streaming download manager (size limit, SHA-256, program
  and lying-name refusal), a source record with checksums, `bridle web` and the egress listing. No PDF, archive or crawler yet.
- The managed browser (ADR-0020), off by default and machine-config only: `[web.browser]` (rule V49), the tools `browser_navigate`,
  `browser_snapshot`, `browser_action` and `browser_close`, a pinned and checked Chromium through a Playwright worker that Bridle starts
  for each session, a closed JSON-line protocol with no scripts, selectors, paths or launch flags, Bridle's own egress proxy that judges
  every connection the browser makes (numbers in any spelling, local names, ports, one resolution per name, a byte limit per call),
  downloads and screenshots through the existing artifact code, source records with the Chromium and Playwright versions, event-log
  entries, `bridle web browser status / setup / test / remove`, and a `browser` line in `bridle doctor` and `bridle egress`.
- The web address guard also refuses `localhost.` (trailing dot) and the old spellings of IPv4 addresses (`2130706433`, `0x7f.1`, `127.1`).
- Browser: `web.browser.allowed_hosts` and `blocked_hosts`; behind the person's proxy the tunnel is now asked for at the address Bridle
  checked (`allow_unpinned_destinations` for names only the proxy can resolve); Chromium's own service endpoints are refused quietly;
  the whole Chromium folder is hash-pinned in the source and `chrome.dll` is re-checked before every start; session folders carry the
  owner's process id and those of dead processes are swept; a test shows Chromium exits when Bridle is ended without clean-up.
- The chat renders Markdown (tables, headings, lists, quotes, code blocks with Copy, links), with a Copy button on each answer and a
  jump-to-latest button. The Explorer has a project header with the branch, a fuzzy filter, an Open editors section, Git colours and
  letters, indent guides, remembered open folders, keyboard navigation and drag and drop to move.
- A member's turn is protected by a Git layer: a project lock, checkpoints before and after under `refs/bridle/runs/`, a journal, recovery
  of a cut-short run, and Undo changes on the issue. See [Runs, checkpoints and recovery](docs/reference/run-recovery.md).
- `board_plan` creates all the child issues of a plan in one call; a project manager who makes no plan is given the issue back twice.
- A slot whose server is still loading shows as `warming` and the chat does not send to it.
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
  dumps, and `bridle egress` (M9).
- Subagent tier: a small model reads a few bounded, redacted files and answers with checked citations (M4b).
- Repository memory: an `AGENTS.md` file (or the file `[memory]` names) is added to the system prompt, read through the
  tool policy, redacted and bounded (validation rule V41).
- Images in the chat: `/image <path> [question]` sends a PNG, JPEG, GIF or WebP from the workspace to a slot whose
  `capabilities.vision` is on; the file is read through the policy and limited to 4 MiB.
- Plugins (ADR-0013): manifests in the machine config declare tools that run a fixed program. Each tool's class is the
  stricter of its declared class and the command classifier's; only read-only and safe-execution tools are offered, and
  `bridle plugins` lists them.
- Linux sandbox for commands (ADR-0016): Landlock on Linux, on by default there, refusing rather than running unconfined.
- Team run in the chat (`/team <request>`): a planner, a worker for each step, and a verifier, in sequence, with each
  role's end recorded in the task journal (ADR-0011).
- Routing: each chat request sent to the default slot is placed by rule, and the rest by the Laya decision model on this
  machine, with `main` as the fallback and `deep` used only when it is running. `bridle route` shows the choice (V42 to
  V44; ADR-0010, ADR-0015).
- Benchmark runner and corpus, with a report (M10). The live results are not yet recorded.
- A checklist of every stage, checked against the code by `scripts/check_checklist.py`.
- Control Center: `bridle tui`, a terminal application with a chat on the home screen and views of tasks, events,
  decisions, slots, diagnostics, configuration and destinations. The chat sends every tool call through the hook's policy
  path; the views are read-only and write nothing.
- Licence (Apache-2.0), NOTICE, third-party licence list, security and contributing guides, and CI (M11).

- The Bridle desktop app (Windows): `bridle` opens a window on a folder with the chat, an editable Monaco code editor with
  tabs, side-by-side diffs, a file explorer, project search, a Changes list with per-file revert, Ctrl+P go to file, a status
  bar, and a real terminal (Git Bash, PowerShell, Command Prompt or WSL on demand). It runs the same agent loop, policy and
  event log as the terminal UI (ADR-0017). `bridle COMMAND` runs a command in the terminal; `bridle-cli` is the headless build.
- Conversation compaction: old turns are folded into a rolling summary written by the model, automatically or with `/compact`,
  with a context meter. A tool call cut off by the output limit no longer breaks the rest of the chat.
- Images: an Image button, `/image`, pasting and dropping. `launch.mmproj_file` and `launch.mmproj_offload` load a vision
  projector; validation rule V45.
- A VRAM guard: a GPU model server starts only if its weights, cache and projector fit with 768 MiB to spare, estimated from the
  model file's header, in every start path.
- A proxy setting in the app (`network.proxy_mode`, `network.proxy`, machine config only; validation rule V47) with a Test
  button, and `BRIDLE_PROXY` for an authenticated proxy.
- Windows tooling without WSL: `scripts/dev.cmd`, the end-to-end suite in `tools/e2e`, `scripts/install-bridle.cmd`, `CLAUDE.md`.

### Fixed

- Remote HTTPS requests used a bundled list of root certificates instead of the system store, so they failed behind a proxy that
  re-signs certificates and for sites with newer roots. They now use the system store, as the documentation said.
- The browser worker's own `--disable-features` replaced Playwright's list, which switched Chromium's time query and "AI mode" check
  back on; the lists are now merged. SearXNG was sent `time_range=week`, which it does not define; it is now `month`.

### Changed

- The product name is Bridle everywhere (ADR-0018): `bridle.toml`, `.bridle/`, `BRIDLE_*`, the `bridle` machine config folder
  and the `bridle_<tool>` names in OpenCode. The old `local-agent` names are read as a fallback and never written.
- The shipped `local-only` profile uses a 65,536-token context and loads the vision projector in system memory.

### Known limits

- The release runs on native Windows and is verified on one machine.
- Bridle is not a sandbox. See [security](docs/concepts/security.md).
- The live checks of the Jev service and the LiteLLM gateway are not part of the verified results.
- `security.require_worktree` and `verification.required_for_done` are read by the configuration checks only.
- OpenCode's model catalogue fetch and the model downloads are outside Bridle's control.
