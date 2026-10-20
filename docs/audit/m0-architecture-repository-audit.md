# M0: Architecture & Repository Audit

| | |
|---|---|
| **Milestone** | M0 (audit). No substantive code is written in this milestone. |
| **Date** | 2026-10-21 |
| **Inputs** | [Master plan](../spec/master-development-plan.md) (verbatim brief), [traceability matrix](../spec/requirements-traceability.md) including PM directives D-1 to D-5, host inspection, upstream sources (§S) |
| **Supersedes** | The docs-only M0 at `D:\aiagentforself` (2026-10-09, TypeScript-based). Its upstream research was re-checked and reused here. |

This audit covers the 12 items required by §66. It then lists contradictions and challenges (§C), checks the
brief's upstream claims (§U), and records the open questions for the PM (§Q).

---

## 1. Current repository structure

`/home/kushagra/harnessforkush` (WSL2, ext4) was **empty and not a Git repository**. This is a greenfield
project with no code, abstractions or agent loop to reuse.

An earlier attempt, `D:\aiagentforself`, contains only M0 documents for a TypeScript design. Its verbatim
brief matches the current one word for word (checked by normalised word diff; the only differences are the
opening sentence and code-fence language tags). That means its **traceability matrix was reused**, then
amended for the Rust directive. No code exists there either.

M0 adds:

```
docs/spec/master-development-plan.md     verbatim brief (never edited)
docs/spec/requirements-traceability.md   one row per point + PM directives D-1..D-5
docs/audit/m0-architecture-repository-audit.md   this file
docs/adr/0001-rust-core-thin-host-adapter.md     Rust decision (D-1)
.gitignore  .gitattributes
```

**Location note.** The WSL virtual disk lives on **C:** (`%LOCALAPPDATA%\Packages\CanonicalGroupLimited.Ubuntu…`),
so the repo and its `target/` directory consume C: space. Model weights never go here (D-2).

## 2. Current dependencies

**Repository dependencies:** none.

**Host** (inspected 2026-10-21):

| Component | Found | Relevance |
|---|---|---|
| OS | Windows 11, build 10.0.26200 | Target host ✓ |
| CPU | Intel i7-14650HX, 24 logical processors | Good for CPU-resident LFM ✓ |
| RAM | 16 GB (15.7 GB usable) | Tight. WSL alone may take 11 GB (see K-11). |
| GPU | **RTX 4060 Laptop**, 8188 MiB, driver 592.82, CUDA 13.1. 0 MiB used at idle (Optimus). | Full 8 GB available for models ✓. Laptop power and thermal limits add benchmark variance (K-14). |
| Disks | C: **59.9 GB free** (91 % used), D: 137.8 GB free, E: 24 GB free | Weights and runtimes go to D: (D-2) |
| WSL | Ubuntu 24.04, WSL2. `.wslconfig`: `memory=11GB`, `swap=4GB`, `processors=16` | Explains the brief's "WSL eats host memory" observation (§6) |
| Rust (Windows) | rustup with toolchain **1.95.0-x86_64-pc-windows-msvc** installed, **no default set** | `rust-toolchain.toml` pins 1.95.0, so no `rustup default` change is needed ✓ |
| MSVC (Windows) | VS 2022 Community, MSVC 14.44.35207 (`link.exe`, `cl.exe`), Windows SDK 10.0.26100 | Native Windows builds work ✓ |
| Rust (WSL) | 1.95.0 and 1.99.0 stable (`~/.cargo/bin`, not on the non-interactive PATH) | Development and Linux CI parity ✓ |
| git | Windows 2.51.0, WSL 2.43.0 | ✓ |
| Python (Windows) | `E:\miniconda\python.exe` 3.13.5 (first on PATH); also Python 3.10 | Toy Python repo for the M1 exit test; research adapters later ✓ |
| Node.js | **Windows: none.** WSL: v18.19 (EOL). | Only needed to type-check the TS shim (CI). OpenCode bundles its own Bun runtime. |
| OpenCode | **missing** | Required in M1: portable `opencode-windows-x64.zip` v1.19.3 → `D:\runtimes\opencode\` |
| llama.cpp | **missing** → downloading b11756 `win-cuda-12.4-x64` + cudart 12.4 to `D:\runtimes\` | Qwen (GPU) and LFM (CPU) |
| PrismML llama.cpp | **missing** → downloading `prism-b10787-73bfe6c` `win-cuda-12.4-x64` to `D:\runtimes\` | Bonsai PTQ1_0 (M5) |
| Existing models | `D:\models\`: Qwen3-0.6B, Qwen2.5-0.5B, Llama-3.2-1B, LFM2.5-1.2B-**Instruct**, SmolLM2-360M, gemma-3-270m (all Q8_0 GGUF) | Not part of the stack. **Qwen3-0.6B is reused as a fast "smoke model"** for local provider integration tests. |
| ripgrep | WSL 14.1.1; not on Windows PATH | M2 uses the `ignore`/`grep` crates (ripgrep's own libraries), so no binary is needed |

**Session constraint (not a product constraint).** In this agent session, Windows-native `curl.exe` could not
reach the network (it went through an unreachable proxy). Downloads therefore run from WSL and write to
`/mnt/d`. Local inference and Windows processes are unaffected.

## 3. Existing OpenCode integration points

There are none in the repository. These are the upstream surfaces, **verified against the published
`@opencode-ai/plugin@1.19.3` and `@opencode-ai/sdk@1.19.3` type definitions**:

**Stable line: OpenCode v1.19.3** (2026-10-16, MIT). Windows x64 portable zip available.

| Surface | Use in this project |
|---|---|
| `opencode.json` `provider` with `@ai-sdk/openai-compatible` | Local llama-server endpoints |
| `opencode.json` `agent` (model, prompt, tools, permission per agent) and `.opencode/agents/*.md` | `build`, `explore`, `review`, `deep` |
| `opencode.json` `mcp` → `McpLocalConfig {type:"local", command[], environment, enabled, timeout}` | **All custom tools**, served by `agent mcp` (ADR-0001). Tools appear as `local-agent_<tool>`. |
| `permission` (allow/ask/deny, bash glob patterns) | Base permission layer. We extend it rather than replace it (§11). |
| Plugin hooks `tool.execute.before` (mutable `args`; **throw to block**), `tool.execute.after` (mutable `title/output/metadata`) | Policy, path guard, redaction, fingerprinting, event log (§34) |
| Plugin hook `permission.ask(Permission) → status: allow/deny/ask` | Our command-risk classifier decides permission requests |
| Plugin hook `shell.env` (mutable `env`) | Secret scrubbing for shell tools (§27) |
| Plugin hook `event` (`session.idle`, `session.error`, `file.edited`, `permission.replied`, …) | Telemetry and the outer controller trigger |
| `chat.params` (temperature, topP, topK, maxOutputTokens, options) | Per-model sampling from our config |
| `experimental.chat.system.transform`, `experimental.chat.messages.transform`, `experimental.session.compacting` | Request interception and compaction (§33, §23). **Experimental**: feature-detected and isolated in the shim. |
| `opencode run` / `opencode serve` + SDK | Headless runs for the autonomous mode and the evals |

**OpenCode 2.0** is a separate beta (`opencode2`, `@opencode/plugin`, npm tag `next`/`beta`). It is out of
scope until it is stable. The host-adapter seam is reserved.

## 4. Existing agent and tool abstractions

There are none in the repository. We reuse these from OpenCode:

| OpenCode capability | Plan mapping |
|---|---|
| Built-in agent loop (model ↔ tools) | The inner loop of IMPLEMENT/VERIFY (§17) |
| Agents and subagents (`task` tool) | Bounded subagent calls (§12–13) |
| Permission system | The base of the tool policy (§25) |
| Built-in read/edit/write/grep/glob/list/bash/LSP | Generic file and shell operations. We govern `bash` instead of duplicating it (C-14). |
| Session compaction | Fallback when our context manager is off (§23) |

## 5. Existing tests

None. M1 creates the test harness: `cargo test` unit tests per module, integration tests in `crates/*/tests/`,
and security tests in `crates/harness-core/tests/security_*.rs`. Real-model tests are gated behind
`LOCAL_AGENT_IT_ENDPOINT`.

## 6. Current configuration system

There is none yet. The design:

- **One harness config**, `local-agent.toml`, at the target repo root, with complete profiles under `configs/`
  (`local-only.toml`, `development.toml`, later `local-jev.toml`, `hybrid.toml`, `benchmark.toml`). Sections follow
  §35: `agent`, `models`, `runtimes`, `paths`, `routing`, `decisions`, `context`, `security`, `verification`,
  `telemetry`, `external`.
- Unknown keys are rejected (`deny_unknown_fields`). Secrets are referenced only by env-var name
  (`api_key_env`).
- **OpenCode config is generated from it** by `agent init` (provider, agents, MCP server, permission rules, plugin
  shim). This avoids two model registries drifting apart (C-11).

## 7. What can be reused

| Reuse | Purpose | Justification |
|---|---|---|
| **OpenCode v1.19.3** (pinned) | UX, sessions, inner loop, tools, permissions, subagents, LSP, headless mode | Strategic decision R-1.2 |
| **llama.cpp `llama-server` b11756** (prebuilt Windows CUDA 12.4) | Qwen3.5-9B on GPU, LFM2.5 on CPU | OpenAI-compatible; exposes `/health`, `/props`, `/v1/models`, `/metrics`, `/slots`; tool calls via `--jinja`; reasoning budget; MTP via `--spec-type draft-mtp` |
| **PrismML llama.cpp prism-b10787** (prebuilt Windows CUDA 12.4) | Bonsai 2 27B PTQ1_0 | The only runtime that loads PTQ1_0/PQ2_0 |
| `D:\models\Qwen3-0.6B` (already on disk) | Fast real-endpoint integration tests | Avoids loading the 5.7 GB main model in tests |
| Rust crates: `serde`, `serde_json`, `toml`, `thiserror`, `anyhow`, `clap`, `regex`, `sha2`, `ureq`, `sysinfo`, `dunce`, `tempfile` (dev) | See ADR-0001 and `docs/dependencies.md` | Each is justified in `docs/dependencies.md` (added in M1) |
| git CLI | Git subsystem (§49) | Already installed |
| *Candidate:* llama-swap | Model lifecycle and VRAM swapping (§45) | Decide in M5 against the in-house launcher (C-9) |
| *Optional:* LiteLLM ≥ v1.104.2 | Gateway, Jev, hybrid (M8) | Strategic: optional |

## 8. What must be built

| Area | Component | Milestone |
|---|---|---|
| Identity | `harness-core::identity`, the only place the temporary name lives | M1 |
| Config | TOML schema, loader, validation, profiles, env-var secret references | M1 |
| Models | Capability-based `ModelRegistry`; `ModelProvider` trait; `OpenAICompatibleProvider`, `LlamaCppProvider` (adds `/props` and `/health`), `MockProvider`; model manifest (sha256, revision, runtime build, driver) | M1 |
| Runtime launcher | `agent models serve <role>` starts llama-server with config-derived args and waits for `/health` | M1 |
| Health | Functional checks: health, completion, streaming, tool call, structured JSON, latency, context | M1 |
| CLI | `agent doctor`, `agent health`, `agent init`, `agent models {serve,manifest}`, `agent hook`, `agent mcp` | M1 |
| Host adapter | TS shim (`opencode-plugin/local-agent.ts`) plus Rust `host/opencode_v1.rs` | M1 |
| Security baseline | Path guard (traversal, absolute, symlink, junction, device names, ADS), secret redactor, command-risk classifier, `shell.env` scrubbing | M1 (hardened in M9) |
| Telemetry | Structured JSONL event log, local only | M1 |
| Tools (MCP) | M1: `git_status`, `git_diff`, `run_verification`. M2: repo_map, symbol_search, find_references, file_summary, dependency_search, git_history/log_range, run_tests/linter/typechecker, parse_test_failure, experiment adapter, cost classifier | M1, M2 |
| State | TaskState, plan, fingerprints, loop detector, state machine, worktrees, `agent explain/trace/run`, seed eval | M3 |
| Decisions | `DecisionEngine` trait, Rules → LFM → Jev | M3, M4, M7 |
| Deep | Deep agent (edit=deny), escalation, lifecycle manager | M5 |
| Context | ContextManager (rules first) and request-transform adapter | M6 |
| External | Jev engine (data-minimised DTO), LiteLLM provider | M7, M8 |
| Eval | Research Coding Eval corpus, matrix runner, reports | M10 |

## 9. Risks

| ID | Risk | L | I | Mitigation |
|---|---|---|---|---|
| K-1 | OpenCode V1 → V2 migration breaks plugin and server APIs | High | High | All logic is in Rust. OpenCode-specific code is limited to the TS shim and `host/opencode_v1.rs`. Tools use MCP, which is host-neutral. |
| K-2 | Request interception relies on `experimental.*` hooks | High | Med | Feature-detect and degrade to state injection through MCP tools and agent prompts. Document the requirement (R-32.3). |
| K-3 | OpenCode recommends WSL on Windows, so native Windows is less exercised | Med | Med | Keep native (R-6.1). Windows CI job, doctor checks, troubleshooting doc. |
| K-4 | Bonsai VRAM: 5.95 GB weights + KV at 24K + buffers ≈ 7.3–8 GB on an 8 GB laptop GPU | High | High | Measure in M5. Use q8_0 KV, no vision projector, a 16K cap if needed, a reasoning budget. |
| K-5 | Swap cost per escalation (Qwen ↔ Bonsai cannot coexist) | High | Med | Measure. Batch deep questions. Use a lifecycle manager (C-9). |
| K-6 | Two llama.cpp builds (upstream and Prism), with the fork lagging | Med | Med | The runtime binary is set per model in config. The manifest records both builds. |
| K-7 | LFM as a full OpenCode agent: large system prompt and tool schemas prefilled on CPU | High | Med | Main LFM role is an out-of-band DecisionEngine with compact JSON. Subagent role is an M4 A/B (C-5). |
| K-8 | Qwen3.5 thinking is on by default, which adds latency per turn | High | Med | Per-agent `enable_thinking`/reasoning budget. Benchmark both. |
| K-9 | Qwen3.5 tool-call template parsing at Q4 | Med | High | The M1 health check performs real tool calls. Pin the build. Track the malformed-call rate. |
| K-10 | MTP needs a GGUF with the MTP head. Whether the unsloth Q4_K_M has it is unknown. | Med | Low | Optional. Detected by `agent health` (R-2.7). |
| K-11 | RAM pressure: WSL may take 11 GB of 16 GB | High | High | Doctor reports WSL state and cap. Docs recommend `wsl --shutdown` or a lower `memory=` during heavy runs. LFM Q4_K_M is 0.73 GB. |
| K-12 | Jev is hosted, so decision state leaves the machine | Certain | Med | Opt-in. Whitelisted, redacted DTO with no source code. Outbound payloads logged locally (C-8). |
| K-13 | LFM licence (`lfm1.0`, HF "other") is not OSI-approved | Certain | Low | Never redistribute. Users download. Legal review in M11. |
| K-14 | Laptop GPU thermals make benchmark results vary | High | Med | Record power mode and temperature. Repeat runs. Report variance. |
| K-15 | OpenCode owns the inner loop | Certain | High | Two-level control (C-7) |
| K-16 | Windows path-escape surface: junctions, 8.3 names, UNC/`\\?\`, ADS, device names | Med | High | Canonicalise plus explicit checks. Adversarial tests from M1. |
| K-17 | Shell risk classification across bash and PowerShell (`-EncodedCommand`, `iex`, chaining) | High | High | Conservative: unrecognised commands → `ask`. Explicit allow-list for EXECUTE_SAFE. Compound commands are classified by their riskiest segment. |
| K-18 | **New (D-1):** hook bridge latency and availability (process spawn per hook) | Med | Med | Measure in M1. Fail closed for policy hooks. Daemon fallback behind the same JSON contract (ADR-0001). |
| K-19 | **New:** the repo and `target/` sit on the WSL disk, which is on C: (60 GB free) | Med | Low | Use `cargo clean` hygiene. Windows builds use `CARGO_TARGET_DIR=D:\cargo-target\harness`. No weights in WSL (D-2). |
| K-20 | **New:** subagent-written code quality (D-3) | Med | Med | Opus writes interfaces and acceptance tests first. Every change is reviewed and must pass fmt, clippy and tests before commit. |

## 10. Recommended implementation sequence

The plan's order is kept, with the same three implementation-level adjustments as the earlier audit
(C-3, C-4): baseline security moves into M1, worktrees and a seed eval move into M3.

| Milestone | Scope |
|---|---|
| **M0** | This audit, ADR-0001, repo initialised |
| **M1** | Cargo workspace and CI; identity; config and profiles; model registry, providers and manifest; health checks; launcher; `doctor/health/init/models/hook/mcp`; TS shim; baseline security; JSONL event log; `build` and `explore` agents on Qwen. **Exit:** OpenCode → Qwen → inspect → edit → run test → result, recorded in the event log, on native Windows. |
| **M2** | Deterministic tools, experiment-cost classifier, declarative verification |
| **M3** | TaskState, state machine, fingerprints, loop detection, RulesDecisionEngine, worktrees, `explain/trace/run`, seed eval (about 6 tasks) |
| **M4** | LFM LocalDecisionEngine, review agent, explore-on-LFM A/B |
| **M5** | Bonsai deep agent (edit=deny), escalation, lifecycle manager |
| **M6** | ContextManager and request-transform adapter |
| **M7** | Jev engine and A/B (rules vs LFM vs Jev) |
| **M8** | LiteLLM gateway, opt-in cloud provider |
| **M9** | Security hardening and adversarial suite |
| **M10** | Full Research Coding Eval, matrix A–F |
| **M11** | OSS release |

## 11. Exact files expected to change

**M0:** the five documents listed in §1, plus `.gitignore` and `.gitattributes`.

**M1 (planned; may shrink, must not grow without a note):**

```
Cargo.toml  Cargo.lock  rust-toolchain.toml  rustfmt.toml  .editorconfig
.github/workflows/ci.yml                         fmt, clippy, test (windows-latest + ubuntu-latest), weights guard, tsc

crates/harness-core/{Cargo.toml, src/lib.rs}
crates/harness-core/src/identity.rs              the ONLY place the temporary name lives
crates/harness-core/src/config/{mod,schema,load}.rs
crates/harness-core/src/models/{mod,capabilities,registry}.rs
crates/harness-core/src/security/{mod,path_guard,redact,command_risk,tool_policy}.rs
crates/harness-core/src/telemetry.rs
crates/harness-core/tests/{security_paths,security_commands,config}.rs

crates/harness-runtime/{Cargo.toml, src/lib.rs}
crates/harness-runtime/src/providers/{mod,openai_compat,llama_cpp,mock}.rs
crates/harness-runtime/src/{health,manifest,launcher,event_log,sysprobe}.rs
crates/harness-runtime/src/mcp/{mod,protocol,tools}.rs
crates/harness-runtime/tests/{provider_mock_server,mcp_protocol}.rs

crates/harness-cli/{Cargo.toml, src/main.rs}
crates/harness-cli/src/commands/{doctor,health,init,models,hook,mcp}.rs
crates/harness-cli/src/host/opencode_v1.rs       the ONLY Rust module that knows OpenCode payload shapes

opencode-plugin/{local-agent.ts, package.json, tsconfig.json}
configs/{local-only.toml, development.toml}
examples/toy-python-repo/**                      M1 exit-criterion target
docs/{architecture,installation,configuration,models,security,troubleshooting,dependencies}.md
docs/adr/0002-two-level-control-loop.md
README.md  LICENSE (pending Q1)
```

## 12. Upstream API and version constraints

| Component | Constraint | Source |
|---|---|---|
| OpenCode | Pin **v1.19.3**. Provisional supported range `>=1.19.0 <2.0.0`, set by contract tests. V2 out of scope. | GitHub releases; plugin type definitions |
| `@opencode-ai/plugin` | Type-only import in the shim, version-matched (1.19.3). The shim uses only `node:child_process`, which Bun supports. | npm registry |
| llama.cpp | **b11756** (2026-10-20). Windows CUDA **12.4** build chosen over 13.4: driver 592.82 reports CUDA 13.1, so a 13.4 build would rely on minor-version compatibility. 12.4 is unambiguous. | GitHub releases; `nvidia-smi` |
| PrismML fork | **prism-b10787-73bfe6c** (2026-10-19), CUDA 12.4. `--jinja` required for tools. | GitHub releases |
| Qwen3.5-9B | `unsloth/Qwen3.5-9B-GGUF` @ `3885219b…`, `Qwen3.5-9B-Q4_K_M.gguf`, 5.68 GB, sha256 `03b74727…52b7e8`. Apache-2.0. There is no official `Qwen/Qwen3.5-9B-GGUF` repo (HTTP 401). Thinking on by default. Coding sampling T=0.6, top_p=0.95, top_k=20. | HF API, model card |
| Ternary Bonsai 2 27B | `prism-ml/Ternary-Bonsai-2-27B-gguf` @ `b072e1d3…`, `PTQ1_0`, 5.95 GB, sha256 `53107f53…fe33ee3`. Apache-2.0. | HF API |
| LFM2.5-1.2B-Thinking | `LiquidAI/LFM2.5-1.2B-Thinking-GGUF` @ `9584426e…`, `Q4_K_M`, 0.73 GB, sha256 `7223a220…12e1231b`. Licence `lfm1.0` (HF: "other"). Sampling T=0.05, top_k=50, rep 1.05. | HF API |
| LiteLLM | `/v1/decisions` and `/v1/systemone` need ≥ v1.104.2 (or ≥ v1.105.0-rc.3) | LiteLLM docs (earlier audit) |
| Jev | Hosted only: `typesafe/jev-latest`, `TYPESAFE_API_KEY` | LiteLLM docs (earlier audit) |
| Rust | **1.95.0** via `rust-toolchain.toml` (MSRV = pinned) | Installed on both hosts |

---

## §C. Contradictions, challenges, and proposed resolutions

Items marked **[impl]** are implementation details; I proceed with them unless the PM objects. **[strategic]**
items use the Decision / Evidence / Risk / Alternative format (R-62.3).

**C-1 [impl] Package count and layout.** §31 asks for one plugin package; §51 lists nine packages; D-1
requires Rust. → Use three crates (`harness-core`, `harness-runtime`, `harness-cli`) plus the TS shim. Modules
mirror §31. Split further only when a module gains an independent consumer.

**C-2 [impl] `explore` model in M1.** §11 assigns `explore` to LFM, but LFM arrives in Phase 4. → `explore`
runs on Qwen (read-only) in M1–M3. The model is configurable. M4 measures LFM.

**C-3 [impl] Measurement before the eval framework.** → A minimal eval runner and seed tasks in M3.

**C-4 [impl] Security too late.** → Baseline guardrails in M1, worktrees in M3, hardening in M9.

**C-5 [strategic] LFM's primary role.**
- *Decision (plan):* LFM is the `explore`/`review` OpenCode subagent and the micro-decision model.
- *Evidence:* OpenCode agent requests carry OpenCode's system prompt and every tool schema (thousands of
  tokens), prefilled on **CPU** in an 8K context. Liquid's model card says LFM is not for programming.
  §46 itself warns that CPU LFM may lose to GPU Qwen.
- *Risk:* An LFM subagent could be slower than a Qwen subagent and could overflow its context.
- *Recommended alternative:* LFM's primary role is the out-of-band `LocalDecisionEngine` and summariser
  (compact JSON in, a validated enum out). Its subagent role becomes an M4 A/B experiment. This stays within
  the agreed direction.

**C-6 [impl] "V2 plugin API".** The stable line is V1 (1.19.x) and V2 is a separate beta. → Target V1 behind
the adapter.

**C-7 [impl, architecture-shaping] Two-level control loop.** OpenCode owns the inner loop, so the §17 state
machine is an outer controller in Rust. *Interactive:* the shim forwards `session.idle`. Rust runs declarative
verification, fingerprints failures, consults the DecisionEngine and continues the session through the SDK,
within the iteration bounds. *Autonomous/benchmark:* `agent run` creates a worktree, starts `opencode serve`
and drives sessions over HTTP. Recorded as ADR-0002 in M1/M3.

**C-8 [impl] Jev egress.** External decision engines receive only a whitelisted, redacted DTO containing enums,
counts and fingerprint hashes, never source code. Each outbound payload is logged locally.

**C-9 [impl, decide in M5] Lifecycle manager.** llama-swap versus the in-house launcher, behind a single
`ModelLifecycle` trait.

**C-10 [impl] Where tools live.** `.opencode/tools/` only loads for the repo that contains it. → Tools ship in
the `agent` binary over MCP, and `agent init` wires a target repo in one step.

**C-11 [impl] Single source of truth.** `opencode.json` is generated from `local-agent.toml`.

**C-12 [needs PM] Project licence.** → Apache-2.0 recommended (patent grant; matches Qwen and Bonsai; compatible
with MIT OpenCode).

**C-13 [done] No Git repo.** Initialised in M0.

**C-14 [impl] `run_command` duplicates OpenCode's `bash`.** → Govern `bash`. Build structured wrappers only where
they return parsed results (`run_tests`, `run_verification`, `run_experiment`).

**C-15 [info] WSL memory.** WSL may use up to 11 GB of 16 GB. `agent doctor` reports this. User config is
not modified.

**C-16 [impl, D-1] Rust and TypeScript boundary.** See ADR-0001. OpenCode only loads JS plugins, so a TS shim is
unavoidable. It is kept logic-free and fails closed for policy hooks.

**C-17 [impl, D-2] Storage policy.** Model and runtime paths are explicit config (`paths.models_dir`,
`paths.runtimes_dir`), never derived from the repo location. `agent doctor` warns when weights would land on
the system drive.

## §U. Upstream fact check of the brief's claims

| Brief claim | Result |
|---|---|
| OpenCode is MIT, with build/plan agents, subagents, project plugins, and `.opencode/tools/` | ✓ verified |
| "Current V2 plugin API intercepts requests and tool execution" | ◐ V2 is beta. Stable V1 (1.19.3) has the equivalent hooks, verified in its type definitions. |
| Qwen3.5-9B: 262K, MTP-trained, LCB v6 65.6, BFCL-V4 66.1, DeepPlanning 18.0, Apache-2.0 | ✓ (earlier audit, model card) |
| Bonsai 2 27B PTQ1_0 5.95 GB / PQ2_0 7.21 GB; fork required; Apache-2.0 | ✓ re-verified via the HF API today (5.95 / 7.21 GB, apache-2.0) |
| LFM2.5-1.2B-Thinking: 1.17B, 32K, GGUF, not for programming, BFCLv3 56.97, `lfm1.0` | ✓ (earlier audit). GGUF Q4_K_M is 0.73 GB (HF API today). |
| LFM sampling T=0.05, top_k=50, rep=1.05 | ✓ |
| LiteLLM supports OpenCode harness and TypeSafe/Jev | ✓ with version floors (§12) |

## §Q. Questions for the PM

1. **Project licence:** Apache-2.0 (recommended) or MIT? (C-12). Until you decide, no `LICENSE` file is
   added.
2. **Temporary identifier:** `local-agent` (CLI `agent`, state dir `.local-agent/`), confined to
   `harness-core::identity`. I am proceeding with this unless you object.
3. **C-5 framing** of LFM's role. I am proceeding with it unless you object.

Downloads and installs needed for M1 are covered by D-2. Everything goes to D:\, nothing is installed
system-wide, and the OpenCode zip is portable.

## §S. Sources

- OpenCode plugins: https://opencode.ai/docs/plugins/ · MCP servers: https://opencode.ai/docs/mcp-servers/
- `@opencode-ai/plugin@1.19.3`, `@opencode-ai/sdk@1.19.3` (npm registry tarballs, type definitions)
- OpenCode releases: https://github.com/anomalyco/opencode/releases
- llama.cpp releases: https://github.com/ggml-org/llama.cpp/releases (b11756)
- PrismML llama.cpp releases: https://github.com/PrismML-Eng/llama.cpp/releases (prism-b10787-73bfe6c)
- HF API: `unsloth/Qwen3.5-9B-GGUF`, `LiquidAI/LFM2.5-1.2B-Thinking-GGUF`, `prism-ml/Ternary-Bonsai-2-27B-gguf`
- Earlier audit sources (Qwen card, Prism docs, Liquid card, LiteLLM decisions/TypeSafe docs): `D:\aiagentforself\docs\audit\m0-architecture-repository-audit.md` §S
