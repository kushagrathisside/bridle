# Requirements Traceability Matrix

This file lists every point in the [Master Technical Development Plan](master-development-plan.md),
one row per point. Each point has a stable ID, a type, the milestone that delivers it, and a status.

- **Do not delete rows.** If a requirement changes, mark it `superseded`. Then link the ADR in
  `docs/adr/` that changed it.
- **Update the status in the same PR that delivers or verifies the requirement.**
- `§n` refers to section *n* of the master plan.

## Legend

| Type | Meaning | Change control |
|---|---|---|
| `STRAT` | Strategic decision | PM approval plus an ADR (Decision / Evidence / Risk / Alternative) |
| `REQ` | Functional or non-functional requirement | Normal review |
| `CON` | Constraint or prohibition ("do not …") | PM approval |
| `DEF` | Initial tunable default. Benchmark it, don't treat it as fixed. | Change with benchmark evidence |
| `ASM` | Documented assumption | Confirm or refute, then record it |
| `CLAIM` | Upstream fact asserted by the plan. Checked in the M0 audit. | Re-verify when upstream changes |

| Status | Meaning |
|---|---|
| `open` | Not started |
| `in-progress` | Partially delivered |
| `done` | Delivered and covered by tests or docs |
| `verified` | `CLAIM` confirmed against an upstream source (see the M0 audit, §U) |
| `corrected` | `CLAIM` is partly wrong. The audit records the correction. |
| `superseded` | Replaced. See the linked ADR. |


## PM directives received after the brief

The project lead gave these directives after the brief. They have the same authority as the brief.
Where a directive changes a brief requirement, the affected row is marked `superseded` or annotated, and
the ADR is linked.

| ID | Date | Type | Directive | Effect | Status |
|---|---|---|---|---|---|
| D-1 | 2026-10-21 | STRAT | **Rust is the primary implementation language.** | [ADR-0001](../adr/0001-rust-core-thin-host-adapter.md): Rust workspace (`harness-core`, `harness-runtime`, `harness-cli` → binary `agent`), tools over MCP, and a thin TS shim only where OpenCode requires JS. Supersedes R-50.2. Narrows R-31.1 and R-50.3. | active |
| D-2 | 2026-10-21 | CON | **Download models to the D: drive only.** C: has about 60 GB free, and WSL's virtual disk also lives on C:. | Weights go to `D:\models\<model>\` (existing convention on this machine). llama.cpp runtimes go to `D:\runtimes\`. `paths.models_dir` in config defaults to `D:\models` on this host, and `agent doctor` warns when the models dir is on the system drive. Weights are never written under the repo or the WSL filesystem. | active |
| D-3 | 2026-10-21 | process | **Use Haiku 5.5 subagents to implement low-level coding tasks.** | Opus designs interfaces, writes the specs and acceptance tests, reviews and integrates. Haiku subagents implement well-specified modules against those specs. Every subagent change is reviewed and must pass `fmt`, `clippy` and tests before it is committed. | active |
| D-4 | 2026-10-21 | process | Keep working until the current usage quota is exhausted. **Do not restart automatically** after the quota ends, even if an automatic or system prompt arrives. | Session-level operating rule for the implementing agent. | active |
| D-5 | 2026-10-21 | process | Save the brief verbatim and track every point in a markdown list. | [`master-development-plan.md`](master-development-plan.md) (verbatim) and this matrix. | done |

---

## §0 Mission

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-0.1 | STRAT | Open-source, local-first coding-agent system for a constrained workstation and demanding software/research workflows | all | open |
| R-0.2 | REQ | Target environment: Windows host, RTX 4060 8 GB VRAM, 16 GB RAM, WSL only when a repo/toolchain requires Linux | M1 (doctor) | open |
| R-0.3 | REQ | Support Git projects in Python, C++, and Rust | M2 | open |
| R-0.4 | REQ | Support research workloads: robotics, simulation, RL, numerical computation, experiment execution and analysis | M2, M10 | open |
| R-0.5 | REQ | Optimise for: unfamiliar-repo comprehension, multi-file implementation, research-code modification, debugging, test/execute/observe/repair loops, simulation/experiment execution, architecture reasoning, numerical correctness, context management, efficient local inference, safe autonomous execution, escalation | all (measured M10) | open |
| R-0.6 | STRAT | Not a chat-with-files app. Build a model-agnostic, policy-driven, local-first harness that improves a small model through orchestration, deterministic tools, verification, context management, and selective escalation. | all | open |
| R-0.7 | ASM | No permanent product name. Use a temporary internal identifier, isolated so a rename needs no architectural change. | M1 | done (`harness_core::identity`: every user-visible name in one file; rename = that file, the `[[bin]]` name and the shim package) |

## §1 Strategic design decision

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-1.1 | STRAT | Do not implement a new coding-agent framework from scratch | all | open |
| R-1.2 | STRAT | Do not fork OpenCode. Treat it as an upstream runtime dependency and host. | all | open |
| R-1.3 | STRAT | Deliver the differentiated work as: OpenCode config, OpenCode agents, custom tools, plugin(s), an optional model gateway, a replaceable decision/orchestration layer, deterministic tooling, persistent local state, and eval/observability infrastructure | M1–M10 | open |
| R-1.4 | STRAT | Project intelligence (routing, micro-decisions, context control, verification, failure classification, escalation, research tools, state, telemetry) lives outside the OpenCode core | all | open |
| R-1.5 | CLAIM | OpenCode is MIT-licensed and has build/plan agents, subagents, project plugins, and custom tools under `.opencode/tools/` | M0 | verified |
| R-1.6 | CLAIM | "The current V2 plugin API can intercept model requests and tool execution" | M0 | corrected: V2 is OpenCode 2.0 **beta**. Stable V1 has equivalent hooks, some `experimental.*` (audit §U). |

## §2 Core model stack: Qwen3.5-9B (main)

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-2.1 | STRAT | Heterogeneous model hierarchy, not one universal model | all | open |
| R-2.2 | STRAT | Main engineering model: Qwen3.5-9B, Q4-class GGUF (Q4_K_M preferred), llama.cpp CUDA, RTX 4060 8 GB | M1 | done (Qwen3.5-9B Q4_K_M on llama.cpp b11756 CUDA, RTX 4060: P1 2026-10-21, 5.4 of 8.2 GB VRAM) |
| R-2.3 | CLAIM | Qwen3.5-9B: 9B multimodal, 262K native context, MTP training, LiveCodeBench v6 65.6, BFCL-V4 66.1, DeepPlanning 18.0 | M0 | verified |
| R-2.4 | REQ | Main model duties: implementation, substantial debugging, moderate architecture reasoning, test interpretation, code review, repo modification, tool calling, multi-step implementation | M1 | in-progress (implementation, debugging, tests and tool calls shown on the toy repo; architecture reasoning and review are measured in M10) |
| R-2.5 | STRAT | Optimisation target is capability × iteration speed × verification, not parameter count | all | open |
| R-2.6 | REQ | The serving adapter exposes: OpenAI-compatible chat, model identity, streaming, tool calls, usage metadata, prompt/completion token counts, latency, health checks, and configurable context and generation parameters | M1 | done (`providers`: OpenAI-compatible chat, streaming, tool calls, usage, timings; `agent health` checks; docs/modules/runtime-providers.md) |
| R-2.7 | REQ | MTP/speculative decoding is an optional capability, detected at install/health-check time and exposed via config. It is never a hard dependency. | M1 (detect), M5 (bench) | open |

## §3 Deep reasoning model: Ternary Bonsai 2 27B

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-3.1 | STRAT | Deep/escalation model: Ternary Bonsai 2 27B, PTQ1_0, PrismML llama.cpp fork, CUDA. It is not the default model. | M5 | open |
| R-3.2 | CLAIM | PTQ1_0 is 5.95 GB and PQ2_0 is 7.21 GB. PTQ1_0 decodes faster on Ada. The PrismML fork is required. | M0 | verified (PrismML artifact table also lists 5.93 / 7.25 GB) |
| R-3.3 | DEF | PTQ1_0 is the default on 8 GB cards. Do not default to PQ2_0. | M5 | open |
| R-3.4 | CON | Do not use the full 262K context by default | M5 | open |
| R-3.5 | DEF | Operating modes: normal deep 16K–24K, large deep 24K–32K, very large repo = retrieval + summaries. Benchmark them. | M5, M10 | open |
| R-3.6 | REQ | Invoke Bonsai for: persistent debugging failure, hard architecture, research methodology, hard algorithms, cross-module causal analysis, high-impact refactoring, repeated local-model failure, user-marked "deep", policy-predicted worth-the-latency | M5 | open |
| R-3.7 | CON | Bonsai must not automatically replace Qwen for every task | M5 | open |

## §4 Micro-model: LFM2.5-1.2B-Thinking

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-4.1 | STRAT | Micro-model: LFM2.5-1.2B-Thinking, CPU-resident, GGUF/llama.cpp | M4 | open |
| R-4.2 | CLAIM | 1.17B params, 32,768 ctx, llama.cpp GGUF, recommended for agentic/extraction/RAG, *not* for knowledge-intensive tasks or programming, native function calling, BFCLv3 56.97 | M0 | verified |
| R-4.3 | CON | LFM is never assigned primary implementation responsibility | M4 | open |
| R-4.4 | REQ | LFM bounded tasks: repo exploration, file triage, failure classification, context relevance, tool routing, subagent routing, escalation decisions, loop detection, completion classification, log summarisation, experiment-artifact classification, cheap structured decisions | M4 | open |
| R-4.5 | DEF | Baseline sampling: temperature 0.05, top_k 50, repetition_penalty 1.05. Benchmark before changing. | M4 | open |

## §5 Model role hierarchy

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-5.1 | STRAT | Roles: Qwen3.5-9B is the main engineer. LFM2.5-1.2B is the micro-orchestrator, explorer, classifier, summariser, and decision model. Bonsai 2 27B is the deep reasoner and escalation model. Optional Qwen3.5-2B/4B handles secondary review. | M1–M5 | open |
| R-5.2 | CON | 2B/4B models are optional, not mandatory dependencies for the first release | M11 | open |
| R-5.3 | REQ | LFM is the first subagent/micro-model to test | M4 | open |

## §6 Native Windows execution

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-6.1 | STRAT | Inference runs natively on Windows. WSL is not the default runtime. | M1 | done (release 1 runs inference natively on Windows; ADR-0002 records the WSL decision for release 2) |
| R-6.2 | REQ | Fit the model runtime, OpenCode, compiler, Git, LSPs, tests, repo, simulation, and experiment scripts in 16 GB RAM simultaneously. Measure it. | M5, M10 | open |
| R-6.3 | REQ | WSL is allowed as an escape hatch for repos that genuinely need Linux-only dependencies | M9 (docs) | open |
| R-6.4 | CON | Do not move the inference stack into WSL because of Linux-tooling preference | all | open |

## §7 Inference-layer abstraction

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-7.1 | CON | Never hard-code the harness against one model server | all | open |
| R-7.2 | REQ | `ModelProvider` with adapters: OpenAICompatible, LlamaCpp, PrismBonsai, LiteLLM, JevDecision, Mock | M1 (OpenAICompatible, LlamaCpp, Mock), M5, M7, M8 | in-progress (OpenAI-compatible and llama.cpp done; PrismBonsai, LiteLLM, Jev, Mock pending) |
| R-7.3 | REQ | Primary LLM calls flow ModelRegistry → ModelRouter → ModelProvider | M1, M4 | open |
| R-7.4 | REQ | Declarative model-registry config. The format is negotiable; the separation is not. | M1 | done (TOML, ADR-0001; layered machine/repo/explicit config with repository trust rules; docs/configuration.md) |
| R-7.5 | REQ | Per-model config: name, endpoint, API-key *source*, context, temperature, max output, reasoning budget, MTP, KV-cache mode, timeout, retries, role, capabilities, hardware profile | M1 | done (`ModelConfig`: endpoint, `api_key_env`, context, sampling, max output, reasoning budget, MTP, KV cache, timeout, retries, role, capabilities, hardware; V1-V23) |

## §8 LiteLLM is optional

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-8.1 | CON | LiteLLM is optional infrastructure and is not embedded in the agent loop | all | open |
| R-8.2 | REQ | Minimal mode: OpenCode → direct local model endpoints | M1 | done (OpenCode to Qwen directly; end-to-end runs 2026-10-21 and 2026-10-22) |
| R-8.3 | REQ | Advanced mode: OpenCode → LiteLLM → local/cloud/decision providers | M8 | open |
| R-8.4 | CLAIM | LiteLLM supports OpenCode as a harness, model groups, and TypeSafe/Jev (`/v1/decisions`, `/v1/systemone`, Jev routing, Jev compaction) | M0 | verified (decision routes need LiteLLM ≥ v1.104.2 / ≥ v1.105.0-rc.3) |

## §9 Jev integration

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-9.1 | REQ | Jev is an *optional* `DecisionEngine` backend | M7 | open |
| R-9.2 | REQ | Backends: RulesDecisionEngine, LFMDecisionEngine, JevDecisionEngine, plus a FutureLearnedDecisionEngine extension point | M3, M4, M7 | open |
| R-9.3 | REQ | The rest of the system does not know which backend is active | M3 | open |
| R-9.4 | REQ | `decide(state, schema) → DecisionResult { decision, confidence, reason_code, provider, latency_ms }` | M3 | open |
| R-9.5 | REQ | Jev preserves probabilistic output. LFM output is strictly structured and validated. Rules give deterministic confidence or `null`. | M3, M4, M7 | open |
| R-9.6 | STRAT | The host software, not the model, is the authority | all | open |

## §10 Decision types

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-10.1 | REQ | Controlled vocabulary: NEXT_ACTION, MODEL_ROUTE, TOOL_ROUTE, SUBAGENT_ROUTE, CONTEXT_RETAIN, CONTEXT_DROP, RETRY, ESCALATE, TASK_COMPLETE, TASK_BLOCKED, FAILURE_CLASS, USER_INTERVENTION_REQUIRED, COMMAND_RISK | M3 | open |
| R-10.2 | REQ | `MODEL_ROUTE ∈ {MAIN, DEEP, MICRO, EXTERNAL}`; `NEXT_ACTION ∈ {EXPLORE, IMPLEMENT, TEST, REVIEW, DIAGNOSE, SUMMARIZE, ASK_USER, ESCALATE, DONE}` | M3 | open |
| R-10.3 | CON | No free-form model-generated workflow control where an enum suffices | all | open |

## §11 OpenCode agent configuration

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-11.1 | REQ | Initial agents: `build`, `explore`, `review`, `deep` | M1, M4, M5 | open |
| R-11.2 | REQ | `build`: Qwen3.5-9B. read=allow, edit=allow, bash=ask/policy-mediated. Covers implementation, debugging, tests, Git, controlled autonomous loops. | M1 | done (generated `build` agent: edit allow, read rules with secrets denied, bash `ask` plus allow list, every call through the `tool-before` policy) |
| R-11.3 | REQ | `explore`: LFM2.5-1.2B. read=allow, edit=deny, bash=restricted. Covers repo mapping, dependency tracing, symbol discovery, targeted inspection, concise evidence. | M1 (on Qwen, see audit C-2), M4 (LFM A/B) | open |
| R-11.4 | REQ | `review`: LFM2.5-1.2B or optional Qwen 2B/4B. Covers inspecting diffs, classifying failures, finding regressions and missing tests, challenging the implementation. | M4 | open |
| R-11.5 | REQ | `deep`: Bonsai 2 27B. Covers hard architecture, persistent failure, research reasoning, escalation, senior review. | M5 | open |
| R-11.6 | CON | Build on OpenCode's allow/ask/deny permission system. Do not invent a parallel one. | M1 | done (permissions are generated into `opencode.json`; the plugin hooks only answer allow or deny inside OpenCode's own flow) |

## §12 No multi-agent swarm

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-12.1 | CON | No free-running, recursive role swarm (architect/researcher/coder/tester/…/manager) | all | open |
| R-12.2 | REQ | Hierarchical design: main Qwen → (LFM cheap evidence \| deterministic tools \| review subagent \| Bonsai when justified) | M4, M5 | open |
| R-12.3 | REQ | Subagents return evidence or bounded analysis, not autonomous branches | M4 | open |

## §13 Subagent output policy

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-13.1 | REQ | Subagent result schema: `{summary, findings[], evidence[], confidence, recommendation, files[], commands[], unresolved[]}` | M4 | open |
| R-13.2 | REQ | The parent receives the summary, evidence, file/symbol refs, and unresolved items. It never receives the full transcript by default. | M4, M6 | open |

## §14 Deterministic tools

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-14.1 | CON | Do not use an LLM for anything that can be answered deterministically | all | open |
| R-14.2 | REQ | Initial tools: repo_map, symbol_search, find_references, file_summary, dependency_search, git_status, git_diff, git_history, git_log_range, run_tests, run_linter, run_typechecker, parse_test_failure, run_command, inspect_artifact, compare_experiments, summarize_experiment, inspect_config | M2 | in-progress (M2: git_status, git_diff, git_history, git_log_range, run_tests, run_linter, run_typechecker, parse_test_failure done; others planned in docs/milestones/m2-tools.md) |
| R-14.3 | REQ | Extensible research tool adapters: run_experiment, inspect_training_log, compare_runs, extract_metrics, inspect_mujoco_xml, inspect_jax_trace, inspect_tensor_shapes | M2 (adapter API), later | open |
| R-14.4 | CON | Do not hard-code the user's current research repository into the core. Use adapters. | all | open |

## §15 Repository intelligence

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-15.1 | CON | No vector database as the first solution | M2–M6 | open |
| R-15.2 | REQ | Deterministic retrieval first: ripgrep, AST, Tree-sitter, LSP, Git, symbol index, dependency graph | M2 | open |
| R-15.3 | REQ | Use semantic retrieval only for conceptual queries, behind a future `ContextRetriever`. Not in the MVP. | post-M6 | open |

## §16 Persistent state

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-16.1 | REQ | Local state, separate from OpenCode's, under `.local-agent/`: state.json, task.json, plan.json, repo-map.json, decisions.jsonl, failures.jsonl, tool-events.jsonl, summaries/, benchmarks/ | M3 | open |
| R-16.2 | REQ | `.local-agent/` is gitignored by default | M1 | done (`.local-agent/` added by `agent init`; checked in toy repo) |
| R-16.3 | REQ | State can be reconstructed from Git and logs where possible | M3 | open |
| R-16.4 | REQ | `TaskState` holds task_id, repository, branch/worktree, original_prompt, current_objective, phase, iteration, active_model, last_action, last_test, failure_class, failure_count, escalation_count, context_version, status | M3 | open |

## §17 Agent state machine

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-17.1 | REQ | Bounded explicit workflow: START → DISCOVER → PLAN → IMPLEMENT → VERIFY → (PASS → REVIEW → DONE \| FAIL → DIAGNOSE → recoverable: IMPLEMENT / unclear: EXPLORE / repeated: ESCALATE) | M3 | open |
| R-17.2 | REQ | Maximum iteration counts are configurable | M3 | open |
| R-17.3 | DEF | Implementation retries 3, same-failure retries 2, deep escalations 1, automatic command retries 1. Tune from trajectories. | M3, M10 | open |

## §18 Escalation policy

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-18.1 | REQ | Default Qwen → Qwen → Qwen. On stall, LFM classifies the state. Persistent or high-complexity failure goes to Bonsai. | M4, M5 | open |
| R-18.2 | REQ | Triggers: same normalised failure ≥ 2, different patches with the same failure, high architecture uncertainty, cross-module behaviour, research-methodology question, explicit user request, high-impact change | M5 | open |
| R-18.3 | REQ | Bonsai analyses and proposes. Qwen implements and tests. | M5 | open |
| R-18.4 | CON | Bonsai must not mutate code automatically in the first implementation stage | M5 | open |

## §19 Failure normalisation

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-19.1 | REQ | Recognise semantically similar failures, not only exact-string repeats | M3 | open |
| R-19.2 | REQ | Normalise by exception type, command, test id, source location, key message tokens, stack-trace signature, exit code | M3 | open |
| R-19.3 | REQ | `FailureFingerprint` with stable hashing | M3 | open |

## §20 Loop detection

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-20.1 | REQ | Detect repeated commands, repeated patches, repeated failures, the same file rewritten repeatedly, no metric progress, the same test failing across attempts | M3, M4 | open |
| R-20.2 | REQ | On a loop, stop and invoke the decision engine. `STALLED` leads to ESCALATE, REPLAN, or ASK_USER. | M4 | open |

## §21 Verification

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-21.1 | CON | "The model says it is fixed" is never evidence | all | open |
| R-21.2 | REQ | Verify via git diff, syntax/build, targeted tests, broader tests, lint/typecheck where configured | M2, M3 | open |
| R-21.3 | REQ | Research code: experiment smoke test, metric extraction, regression comparison, artifact validity | M2, M10 | open |
| R-21.4 | REQ | Robotics: simulation startup, environment reset, rollout execution, metric computation, optional trajectory sanity | adapters (M2+) | open |
| R-21.5 | REQ | Declarative verification adapters (a list of commands in config) | M2 | done (declared `[[verification.steps]]` with `kind` and `targets`; run by `run_verification`, `run_tests`, `run_linter`, `run_typechecker`) |

## §22 Research workflow support

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-22.1 | REQ | Research task mode with hypothesis, expected_behavior, experiment_command, parameters, metrics, baseline, current_result, interpretation, next_action | M3 (schema), M10 | open |
| R-22.2 | REQ | Prefer structured metrics over natural-language summaries | M2+ | open |

## §23 Context engineering

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-23.1 | REQ | `ContextManager`: history policy, tool-output compaction, repo retrieval, state injection, summary generation, context budget, relevance filtering | M6 | open |
| R-23.2 | REQ | Retain the current task, plan, diff, recent failures, relevant files, and current test result. Drop or compact unrelated old tool results, repeated logs, stale listings, and old attempts. | M6 | open |
| R-23.3 | REQ | Keep semantic state separate from the raw transcript. The transcript is not long-term memory. | M3, M6 | open |

## §24 Optional Jev context compaction

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-24.1 | REQ | Tool result → ContextManager → DecisionEngine → KEEP / COMPACT / DROP | M6 (rules), M7 (Jev) | open |
| R-24.2 | CON | Not mandatory for the MVP. Start with deterministic age/type rules, then LFM/Jev for ambiguous cases. | M6 | open |

## §25 Tool policy

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-25.1 | REQ | Tool classes: READ_ONLY, WRITE, EXECUTE_SAFE, EXECUTE_RISKY, NETWORK, DESTRUCTIVE, PRIVILEGED | M1 (baseline), M9 | open |
| R-25.2 | REQ | Examples: read_file/git_diff READ_ONLY. pytest/ruff EXECUTE_SAFE. python script SAFE or RISKY. pip install NETWORK/RISKY. `git reset --hard` and `rm -rf` DESTRUCTIVE. | M1, M9 | open |
| R-25.3 | CON | The model cannot bypass policy through generated arguments. The policy inspects the actual arguments. | M1, M9 | open |

## §26 Sandbox and worktree safety

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-26.1 | REQ | Autonomous work happens in a Git worktree (`.worktrees/agent-task-NNN/`) | M3 (see audit C-4), M9 | open |
| R-26.2 | REQ | The agent never needs permission to destroy the user's primary working tree | M3, M9 | open |
| R-26.3 | REQ | Flow: create worktree → agent works → tests → review → user accepts/merges | M3, M9 | open |
| R-26.4 | REQ | Interactive tasks may explicitly allow direct modification. Autonomous tasks default to isolated worktrees. | M3 | open |

## §27 Security requirements

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-27.1 | REQ | Path protection: confine operations to the authorised workspace. Block `../` traversal, absolute-path escape, symlink escape, and junction/reparse-point escape. | M1 (baseline), M9 | done (path guard and shell argument checks; tests in harness-core) |
| R-27.2 | REQ | Never inject API keys, SSH private keys, `.env` secrets, cloud credentials, browser cookies, or Windows credential stores into model context | M1, M9 | in-progress (redactor tests pass; hook and event log redact; secret read denied end to end) |
| R-27.3 | REQ | Redact API_KEY, TOKEN, PASSWORD, SECRET, AUTHORIZATION, and PRIVATE KEY patterns from telemetry | M1, M9 | open |
| R-27.4 | REQ | Local-first mode never sends repo content to the network unexpectedly. External providers require explicit configuration. | M1 | done (endpoints must be loopback, V4; a repository config cannot add remote endpoints; webfetch and websearch follow `security.network`, denied in release 1). OpenCode's own catalog fetch: R-36.2 |
| R-27.5 | ASM | Optional external/cloud models are supported. The default install is local-only and transmits no repo content. | M1, M8 | open |

## §28 Observability

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-28.1 | REQ | One structured event per agent action (timestamp, task_id, phase, actor, model, tool, duration_ms, status, failure_fingerprint, tokens_in/out) | M1 (basic), M3 | open |
| R-28.2 | REQ | Track task success, time to completion, turns, model used, escalations, subagent use, tool errors, test failures, same-failure loops, human interventions, prompt/completion tokens, latency, GPU VRAM, RAM, CPU | M3, M10 | open |
| R-28.3 | CON | Telemetry is local by default. No analytics endpoint by default. | M1 | done (telemetry has one destination, `local`: JSONL under `.local-agent/`) |

## §29 Model selection policy

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-29.1 | REQ | Deterministic first-pass routing: explicit user model → micro for cheap classification → main for standard coding → main + decision review after failures → deep for architecture/research/high complexity → else main | M3, M4 | open |
| R-29.2 | REQ | The routing policy is configurable and swappable: rules → LFM → Jev → learned router | M3–M7 | open |

## §30 LFM as a local Jev-like policy

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-30.1 | CON | Do not claim LFM replaces Jev | all | open |
| R-30.2 | REQ | `LocalDecisionEngine` (LFM) with schemas RouteDecision, EscalationDecision, ToolSelectionDecision, CompletionDecision, FailureClassification, ContextRetentionDecision | M4 | open |
| R-30.3 | REQ | LFM receives a compact state, never the whole repository | M4 | open |
| R-30.4 | REQ | Validate strictly against the schema. Invalid output falls back to the deterministic policy. | M4 | open |
| R-30.5 | CON | Never execute arbitrary model output as control logic | all | open |

## §31 OpenCode plugin architecture

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-31.1 | REQ | One primary plugin package with modules for routing, decisions, context, state, verification, security, tools, and adapters. Names may change as implementation dictates. | M1 | open. Under D-1 the modules live in the Rust crates. The "plugin package" is the thin shim in `opencode-plugin/`. See ADR-0001. |
| R-31.2 | CON | Do not prematurely create dozens of packages | all | open |

## §32 Stable OpenCode APIs

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-32.1 | CLAIM | The V2 plugin API is beta and may change | M0 | verified |
| R-32.2 | REQ | Stable config and custom tools are the baseline. V2-only hooks sit behind a small adapter. Pin and test against a supported OpenCode release, degrade gracefully, document the supported version range, and never depend on undocumented internals. | M1 | done for OpenCode 1.19.3 (adapter `opencode_v1.rs` pinned; `doctor` checks the version; 34 hook fixtures; shell selection read from the 1.19.3 binary) |
| R-32.3 | REQ | If request interception needs a V2 hook, document the requirement clearly | M1, M6 | open |

## §33 Request interception

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-33.1 | REQ | Request pipeline: inspect → apply context policy → remove irrelevant tools → inject structured state → model-specific guidance → send | M6 | open |
| R-33.2 | CON | The rest of the codebase must not depend directly on OpenCode internals | all | open |

## §34 Tool-execution interception

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-34.1 | REQ | Before execution: policy check → path check → risk classification → approval if required → execute | M1, M9 | done (hook decides before execution; Ask denied; verified by end-to-end runs) |
| R-34.2 | REQ | After execution: redaction → normalisation → failure classification → event log → context manager | M1, M3 | in-progress (redaction and event log done; failure classification and context manager pending) |

## §35 Configuration design

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-35.1 | REQ | One top-level project config with sections: agent, models, routing, decisions, context, security, verification, telemetry, external | M1 | done (config schema, validation V1-V21, profiles) |
| R-35.2 | CON | No API secrets in the config file. Reference env vars or key sources instead. | M1 | done (`api_key_env` must be a variable name, V8; a repository config may not set it; keys never logged) |

## §36 Environment profiles

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-36.1 | REQ | Profiles: local-only, local-with-jev, hybrid, development, benchmark | M1 (local-only, development), M7, M8, M10 | open |
| R-36.2 | REQ | local-only has no external network dependency except model download during installation | M1 | in-progress (harness makes no external calls; OpenCode fetches its model catalog unless disabled) |
| R-36.3 | ASM | The user may accept an external Jev dependency while the coding models stay local | M7 | open |

## §37 Optional cloud frontier escalation

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-37.1 | REQ | The provider interface can expose a cloud frontier model. It is explicitly opt-in. Possible policy: routine → Qwen, hard → Bonsai, very hard → cloud. | M8 | open |
| R-37.2 | CON | Never silently upload source code | all | open |

## §38 Model lifecycle and manifest

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-38.1 | REQ | Distinguish model identity, version, quantization, runtime version, runtime build, hardware, configuration | M1 | done (`agent models manifest`: model id, revision, quantisation, sha256, runtime kind/build/commit, GPU, driver, CUDA, OS) |
| R-38.2 | REQ | Persist a model manifest `{model, quantization, source, revision, sha256, runtime, runtime_revision, cuda, driver}` for Qwen, LFM, and Bonsai | M1, M4, M5 | in-progress (manifest code and tests exist; not run end to end) |

## §39 System benchmark

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-39.1 | REQ | Benchmark suite from real workload categories: 5 tasks each for repo understanding, implementation, debugging, research/simulation, refactoring, and experiment analysis | M10 (seed in M3) | open |
| R-39.2 | ASM | Task counts are an initial proposal, not the final size | M10 | open |
| R-39.3 | REQ | Compare A: Qwen only, B: + deterministic tools, C: + LFM, D: + LFM + Bonsai, E: + Jev, F: + LFM + Bonsai + Jev | M10 | open |
| R-39.4 | REQ | Measure success rate, time to success, human interventions, iterations, failed tool calls, test pass rate, first-pass success, escalation rate, context size, prompt/output tokens, VRAM, RAM, CPU | M10 | open |
| R-39.5 | REQ | Select the winning configuration empirically | M10 | open |

## §40 Beyond SWE-bench

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-40.1 | CON | Do not optimise for SWE-bench alone | M10 | open |
| R-40.2 | REQ | Local "Research Coding Eval" covering repo comprehension, algorithm implementation, debugging, test repair, simulation debugging, experiment analysis, refactoring, cross-language changes. Do not tie the public name to it. | M10 | open |

## §41 Evaluation reproducibility

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-41.1 | REQ | Each run stores task ID, git commit, model versions, runtime versions, configuration, prompt, decision policy, hardware, duration, result, test outcome, human intervention | M3 (seed), M10 | open |
| R-41.2 | REQ | Pin the OpenCode version, plugin version, model revision, quantization, runtime build, OS, and CUDA/driver | M1, M10 | open |
| R-41.3 | REQ | Report throughput split into cold start, model load, prompt prefill, generation, tool execution, and total wall-clock | M5, M10 | open |

## §42 Installation architecture

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-42.1 | CON | Do not ship model weights. Installer commands fetch them. | all | open |
| R-42.2 | REQ | Install flow: install → detect platform/GPU/CUDA → install OpenCode integration → configure endpoints → health checks → benchmark micro and main → optionally install Bonsai | M1 (partial), M11 | open |
| R-42.3 | CON | No unconditional multi-gigabyte download | all | open |

## §43 `agent doctor`

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-43.1 | REQ | `agent doctor` reports OS, CPU, RAM, GPU, VRAM, NVIDIA driver, CUDA compatibility, OpenCode version, llama.cpp version, model endpoints, model health, configured context, tool support, MTP support, Bonsai runtime support, with ✓/⚠ output | M1 | done (`agent doctor`: OS, CPU, RAM, GPU, VRAM, driver, CUDA, OpenCode version, llama.cpp builds, model files, endpoint quick health, busy or reserved ports, config issues; tool, context and MTP support via `agent health`) |
| R-43.2 | REQ | The system still functions when optional components are unavailable | all | open |

## §44 Health checks

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-44.1 | REQ | Each provider has `GET /health` or equivalent, plus functional tests: simple completion, tool capability, streaming, context, structured output, latency | M1 | done (`agent health`: health, models, completion, streaming, tool call, structured output, context, MTP; all pass for Qwen on Windows) |
| R-44.2 | REQ | Model-specific checks: Qwen tool call. LFM classification and structured JSON. Bonsai reasoning and tool call. | M1, M4, M5 | open |
| R-44.3 | CON | HTTP 200 does not imply compatibility | M1 | done (health runs functional checks; HTTP 200 alone never passes) |

## §45 Model unloading and memory

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-45.1 | CON | Do not assume Qwen 9B and Bonsai 27B coexist in VRAM | M5 | open |
| R-45.2 | REQ | Lifecycle states: loaded, warming, ready, idle, unloading, unloaded, error | M5 | open |
| R-45.3 | REQ | Configurable lifecycle (keep Qwen loaded, LFM CPU-resident, Bonsai on demand; unload Qwen before Bonsai if needed) | M5 | open |

## §46 CPU-resident LFM

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-46.1 | REQ | GPU runs Qwen or Bonsai. CPU runs LFM. Benchmark it. | M4 | open |
| R-46.2 | CON | Do not assume CPU LFM beats Qwen. LFM earns its place only by reducing end-to-end wall-clock. | M4 | open |

## §47 Latency-aware policy

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-47.1 | REQ | The router eventually weighs expected accuracy gain against latency cost and failure cost. Routing is never purely parameter-count based. | M10+ | open |

## §48 Experiment control

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-48.1 | REQ | Experiment classes SMOKE, SMALL, MEDIUM, LARGE, EXPENSIVE, each with limits on time, GPU memory, CPU, number of envs, simulation steps, and disk output | M2 (classifier), M9 | open |
| R-48.2 | CON | Never auto-start expensive experiments without policy. For example, `python train.py --steps 10000000` is flagged as potentially expensive and may require approval. | M2, M9 | open |

## §49 Git integration

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-49.1 | REQ | Record branch, commit before/after the task, diff, files changed, tests before/after | M3 | open |
| R-49.2 | REQ | Capture `git status` before autonomous mutation | M3 | open |
| R-49.3 | REQ | Create a checkpoint before broad refactoring when safe | M3, M9 | open |
| R-49.4 | CON | Never reset or rewrite history without explicit permission | all | open |

## §50 Code quality

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-50.1 | REQ | Typed, modular, documented, linted, tested, CI-validated, cross-platform where practical | all | open |
| R-50.2 | REQ | TypeScript strict mode, ESLint, Prettier, Vitest (or equivalent modern tooling) | M1 | **superseded** by D-1 / [ADR-0001](../adr/0001-rust-core-thin-host-adapter.md): rustfmt, clippy `-D warnings`, `cargo test`, cargo-deny (M11). The TS shim is checked with `tsc --strict --noEmit`. |
| R-50.3 | CON | No second language runtime without clear value. Isolate Python research adapters behind a subprocess/API boundary. | all | open. Under D-1, TypeScript is the second language. It is confined to the OpenCode shim, because OpenCode only loads JS plugins. |

## §51 Repository layout

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-51.1 | REQ | Follow the suggested layout (.github, .opencode, packages, configs, docs, evals, examples, scripts, tests, root docs). Adjust only with evidence. | M1+ | open. Adjusted by ADR-0001: `crates/` replaces `packages/`, `Cargo.toml` replaces `package.json`, `configs/*.toml` replaces `*.yaml`, `opencode-plugin/` holds the TS shim (audit C-1). |

## §52 OSS licensing

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-52.1 | REQ | Before publishing: audit OpenCode, model, and dependency licences. Generate third-party notices. Do not redistribute weights. Document model download. Keep the project licence separate from model licences. | M11 | open |
| R-52.2 | CLAIM | OpenCode MIT, Qwen3.5-9B Apache-2.0, Bonsai 2 27B GGUF Apache-2.0, LFM2.5-1.2B-Thinking `lfm1.0` | M0 | verified (`lfm1.0` terms still need legal review) |
| R-52.3 | CON | Never claim blanket "all Apache/MIT compatible" without verifying every component | M11 | open |

## §53 Documentation

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-53.1 | REQ | Usable by a competent developer who has never seen the source | all | open |
| R-53.2 | REQ | README covers what it is, why it exists, architecture, supported models, supported hardware, quickstart, local-only setup, optional Jev setup, optional cloud setup, security model, limitations, benchmark methodology, contributing, licence | M1 → M11 | open |
| R-53.3 | REQ | Architecture diagrams | M1+ | open |
| R-53.4 | REQ | Examples: normal coding task, debugging task, research task, deep escalation, subagent exploration, context compaction, Jev routing | M11 | open |
| R-53.5 | REQ | Troubleshooting for CUDA, VRAM, model loading, Windows paths, PowerShell, WSL interop, OpenCode plugin loading, tool permissions, Bonsai runtime, context overflow | M1 → M11 | open |

## §54 No hidden magic

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-54.1 | REQ | Every routing decision is inspectable | M3+ | open |
| R-54.2 | REQ | `agent explain` shows the task class, main model, micro decision, decision, reason, confidence | M3, M4 | open |
| R-54.3 | REQ | `agent trace` shows the ordered actor/tool/decision trace | M3 | open |

## §55 Capabilities over names

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-55.1 | REQ | Components are specified by capabilities (tools, vision, reasoning, structured_output, streaming, long_context, local, gpu, context). Routing operates on capabilities. | M1 | done for the registry (`Capabilities` per model, `ModelRegistry::find`); routing on capabilities arrives with the decision engine (M3/M4) |
| R-55.2 | REQ | Replacing Qwen with a better 7B/9B needs no architectural change | all | open |

## §56 Future learned orchestrator

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-56.1 | CON | Do not implement a learned orchestrator now | — | open |
| R-56.2 | REQ | Design and document the extension point (Rules → LFM → Jev → Learned) without changing the agent loop. Log trajectories suitable for training. | M3 (docs) | open |

## §57 Development phases / milestones

| ID | Milestone | Deliverables | Success condition | Status |
|---|---|---|---|---|
| R-57.0 | M0 Audit | Repo inspection report: architecture, reuse, risks, dependency constraints, order | Report delivered; no code | **done** (see `docs/audit/m0-architecture-repository-audit.md`) |
| R-57.1 | M1 OpenCode integration | OpenCode config, build agent, explore agent, custom tools, local model connectivity, health check, basic logging. No Jev, no Bonsai, no complex routing. | OpenCode → Qwen → inspect repo → edit file → run test → result | open |
| R-57.2 | M2 Tools | repo_map, symbol_search, git tools, test runner, failure parser, experiment adapter | Main model uses deterministic tools instead of reconstructing repo info | open |
| R-57.3 | M3 State | Task state, plan, event log, failure fingerprints | Session resumes after interruption without transcript reconstruction | open |
| R-57.4 | M4 LFM | Failure classification, loop detection, next-action decision, context relevance | Measured reduction in latency/tokens/failed actions (not assumed) | open |
| R-57.5 | M5 Bonsai | Stalled-task escalation, deep reasoning, senior review (PTQ1_0) | Benchmark of Qwen alone vs Qwen + Bonsai escalation | open |
| R-57.6 | M6 Context | History filtering, tool-result compaction, repo retrieval, state injection, token budgeting | Measured task success, prompt tokens, latency, memory | open |
| R-57.7 | M7 Jev | JevDecisionEngine behind the same interface. No orchestration rewrite. | A/B rules vs LFM vs Jev. Jev on by default only with evidence. | open |
| R-57.8 | M8 LiteLLM | Optional gateway: routing, external providers, Jev, logging, aliases, hybrid | Not required for local-only users | open |
| R-57.9 | M9 Security | Worktrees, path sandbox, secret redaction, risk policy, network policy, destructive-command policy | Adversarial tests pass | open |
| R-57.10 | M10 Evaluation | Benchmark runner, task corpus, experiment matrix, metrics, reports | Full local suite run | open |
| R-57.11 | M11 OSS release | README, docs, licence, security policy, contributing, examples, release workflow, publication, versioning, changelog, benchmark report | Docs and security posture match implementation quality | open |

## §58 Testing strategy

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-58.1 | REQ | Unit tests: routing, state transitions, failure fingerprints, context filtering, policy decisions, parsers, redaction, configuration | M1+ | open |
| R-58.2 | REQ | Integration tests: OpenCode → plugin → local model → tool → state | M1+ | open |
| R-58.3 | REQ | Security tests: path traversal, symlink escape, destructive commands, secret leakage, unauthorised network, workspace escape, malformed model output | M1 (baseline), M9 | open |
| R-58.4 | REQ | End-to-end tests: real repo, real model, real tests, real failure, real recovery | M5, M10 | open |

## §59 Model-output robustness

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-59.1 | REQ | Assume models return invalid JSON, omit fields, malform tool args, hallucinate files, invent commands, repeat, exceed context, and falsely claim success | all | open |
| R-59.2 | REQ | Validate model output at every boundary | all | open |
| R-59.3 | CON | Never trust free-form model claims for permissions, routing, command execution, completion, or state transitions | all | open |

## §60 Critical acceptance criteria

| ID | Area | Criterion | Milestone | Status |
|---|---|---|---|---|
| AC-B1 | Basic | OpenCode loads the extension | M1 | done (OpenCode 1.19.3 loads the shim and the MCP server; every tool call is logged by the hooks; Windows run 2026-10-22) |
| AC-B2 | Basic | Qwen works locally | M1 | done (Qwen3.5-9B serves locally on the RTX 4060; `agent health` passes; Windows runs 2026-10-21 and 2026-10-22) |
| AC-B3 | Basic | LFM works locally | M4 | open |
| AC-B4 | Basic | Bonsai works locally | M5 | open |
| AC-B5 | Basic | Models are independently testable | M1, M4, M5 | open |
| AC-B6 | Basic | Configuration is declarative | M1 | done (TOML config with validation V1-V23 and `agent config validate`) |
| AC-A1 | Agent | Main coding loop works | M1 | done (toy repo: read, edit, test, verify through OpenCode with the policy on every call; Windows run 2026-10-22) |
| AC-A2 | Agent | Subagent invocation works | M1, M4 | open |
| AC-A3 | Agent | Subagent results can be summarised | M4 | open |
| AC-A4 | Agent | Tests are automatically executed where configured | M2, M3 | open |
| AC-A5 | Agent | Failures are parsed | M2 | done (`harness_core::failures`: 14 output formats; failed steps return parsed failures; `parse_test_failure` tool; M2 step 3) |
| AC-A6 | Agent | Repeated failures are detected | M3 | open |
| AC-O1 | Orchestration | Model routing works | M4 | open |
| AC-O2 | Orchestration | Escalation works | M5 | open |
| AC-O3 | Orchestration | Deterministic decision fallback works | M3, M4 | open |
| AC-O4 | Orchestration | LFM decision backend works | M4 | open |
| AC-O5 | Orchestration | Jev backend works when configured | M7 | open |
| AC-O6 | Orchestration | No external service required for local-only mode | M1+ | open |
| AC-S1 | Safety | Dangerous commands are governed | M1 (baseline), M9 | open |
| AC-S2 | Safety | Worktree mode works | M3, M9 | open |
| AC-S3 | Safety | Path escapes are blocked | M1, M9 | open |
| AC-S4 | Safety | Secrets are not logged | M1, M9 | open |
| AC-E1 | Evaluation | Benchmark suite can run | M10 | open |
| AC-E2 | Evaluation | Metrics can be collected | M3, M10 | open |
| AC-E3 | Evaluation | Model versions are recorded | M1, M10 | open |
| AC-E4 | Evaluation | Runs are reproducible | M10 | open |
| AC-R1 | OSS | Docs are complete | M11 | open |
| AC-R2 | OSS | Licence audit is complete | M11 | open |
| AC-R3 | OSS | Third-party licences are documented | M11 | open |
| AC-R4 | OSS | No weights accidentally committed (enforced by `.gitignore` and a CI check) | M1, M11 | open |
| AC-R5 | OSS | CI passes | M1+ | open |

## §61 Prohibitions (what AI must NOT do)

| ID | Prohibition | Enforcement |
|---|---|---|
| P-1 | Fork OpenCode without a demonstrated concrete blocker | ADR required |
| P-2 | Rewrite OpenCode's agent system | Review |
| P-3 | Add every possible framework just because it supports agents | Dependency-justification section in each PR |
| P-4 | Create ten subagents before measuring whether one is useful | Agent count tracked in this file |
| P-5 | Introduce a vector DB before deterministic retrieval is exhausted | Review |
| P-6 | Make Jev mandatory | Test: local-only profile runs with Jev disabled |
| P-7 | Make LiteLLM mandatory | Test: local-only profile runs without LiteLLM |
| P-8 | Make cloud models mandatory | Test: local-only profile has no external endpoints |
| P-9 | Bundle model weights | `.gitignore` + CI check for `*.gguf`/large files |
| P-10 | Expose unrestricted shell access | Tool policy + security tests |
| P-11 | Silently send repository data externally | Network policy + egress tests |
| P-12 | Claim performance improvements without benchmark evidence | Claims in docs must cite an eval run ID |
| P-13 | Optimise token/s while ignoring end-to-end task success | Eval reports lead with task success |
| P-14 | Treat benchmark scores as equivalent across benchmark versions | Eval records benchmark version |
| P-15 | Hard-code assumptions about model availability | Capability-based registry, health checks |
| P-16 | Use unstable OpenCode APIs without isolating them | Host adapter boundary, lint rule banning OpenCode imports in core |
| P-17 | Add abstractions not justified by actual extension points | Review; each interface must have ≥ 2 real implementations or a named planned one |

## §62 Working style

| ID | Type | Requirement | Status |
|---|---|---|---|
| R-62.1 | REQ | The human is PM/research lead; AI is the implementation engineer | active |
| R-62.2 | REQ | Before major implementation: inspect, report grounded findings, identify contradictions, challenge assumptions, suggest changes with evidence, then implement | active |
| R-62.3 | REQ | Distinguish strategic decisions from implementation details. Never silently change strategic decisions. State disagreements as Decision / Evidence / Risk / Recommended alternative, then continue as agreed unless blocked. | active |
| R-62.4 | REQ | Do not ask for confirmation on ordinary implementation decisions | active |
| R-62.5 | ASM | Work happens inside a Git repository AI can inspect and modify | corrected: the working directory `/home/kushagra/harnessforkush` (WSL) was empty and not a Git repo at M0 (2026-10-21). Repo initialised in M0. An earlier, docs-only M0 attempt exists at `D:\aiagentforself` (TypeScript-based, superseded by D-1). |

## §63 Definition of success

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-63.1 | STRAT | Success means maximising the probability of task completion under a fixed local compute budget, not parity with frontier models | M10 | open |
| R-63.2 | REQ | Validate experimentally: (9B + harness) > 9B alone, and whether (9B + micro-policy + verification + deep escalation) beats a larger model used inefficiently | M10 | open |

## §64–§66 Target architecture, philosophy, immediate instruction

| ID | Type | Requirement | Milestone | Status |
|---|---|---|---|---|
| R-64.1 | STRAT | Target architecture: User → OpenCode → extension/orchestrator → {Context Manager, Decision Engine (Rules/LFM/Jev), Tool Policy} → Model Router → {Qwen, Bonsai, optional cloud} → loop IMPLEMENT/TEST/DIAGNOSE → DONE or RETRY/ESCALATE | all | open |
| R-65.1 | STRAT | Principle 1: models are replaceable | all | active |
| R-65.2 | STRAT | Principle 2: the harness owns state | all | active |
| R-65.3 | STRAT | Principle 3: deterministic systems own truth | all | active |
| R-65.4 | STRAT | Principle 4: cheap intelligence handles cheap decisions | all | active |
| R-65.5 | STRAT | Principle 5: expensive intelligence is reserved for high-value reasoning | all | active |
| R-66.1 | REQ | First deliverable is the Architecture & Repository Audit (12 items) | M0 | **done**: [M0 audit](../audit/m0-architecture-repository-audit.md) (2026-10-21) |
| R-66.2 | REQ | Implement incrementally in milestones M0–M11. No giant commit. | all | active |
| R-66.3 | REQ | Every milestone leaves the project runnable, has tests, updates docs, justifies every external dependency, and documents every strategic assumption | all | active |
| R-66.4 | REQ | A real open-source project, not a pile of scripts | all | active |
| R-66.5 | STRAT | Core abstraction: TASK → STATE + EVIDENCE → DECISION ENGINE → {TOOL, MODEL, ESCALATE} → VERIFY → {DONE, REPAIR → DECISION ENGINE}. The design survives replacement of Qwen, Bonsai, LFM, Jev, OpenCode, or LiteLLM. | all | open |
