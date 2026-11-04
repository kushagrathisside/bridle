# Plan: sub-agents, the team, the ideas queue, search and polish

- **Status:** Proposal for the owner's review, revision 2, 2026-11-04. Nothing in it is built. It needs no approval to be read;
  the decisions taken by default are in [section 11](#11-decisions-taken-by-default).
- **Scope:** sub-agents on small models that run on the GPU with the 9B model unloaded, teams of named members the user can
  address with `@`, safe sharing of files and Git between agents with recovery from crashes, an ideas queue, web search for
  literature review, and window polish.
- **What changed in revision 2** (after the owner's feedback of 2026-11-04):
  1. Agent sharing now includes the Git level: checkpoints, worktrees, locks, recovery, a failure matrix, a workflow matrix and a
     manual runbook ([section 3.5 to 3.7](#35-the-same-file-and-the-same-resources-on-top-of-git)).
  2. Sub-agents are a fleet of small models on the GPU with Qwen3.5-9B unloaded, and the candidates for a small manager model
     are compared ([section 3.4](#34-small-models-on-the-gpu-the-fleet-and-its-manager)). This is separate from roles.
  3. The six roles became **six teams**; each team has roles; each role has any number of named **members** with their own prompt
     and speciality; the user can tag a team, a role or one member ([section 5](#5-feature-f2-teams-roles-and-members)).
  4. Memory (what agents remember and share, and what the machine can hold) has its own section
     ([3.8](#38-memory)).
  5. The ideas queue is unchanged, with an explicit list of checks that prove it works ([section 6](#6-feature-f3-the-ideas-queue)).
- **What changed in revision 3** (the owner's feedback of 2026-11-04, later): (1) a survey of small models that can code, and a
  worker rig with more than one model family ([3.4](#34-small-models-on-the-gpu-the-fleet-and-its-manager)); (2) a **crew rail**, a
  strip of member avatars on the right of the window, to tag and monitor members ([5.6](#56-the-crew-rail)); (3) a **board** where
  members have their own issues, tracks and dependencies, so a project manager can split one issue into one for engineering, one for
  QA and one for product ([5.7](#57-the-board)); (4) the open questions are now **decisions taken by default**
  ([section 11](#11-decisions-taken-by-default)) because the owner asked not to be asked, and building has started
  ([section 13](#13-build-status)).
- **Method:** the existing code and decision records were read first ([ADR-0004](../decisions/0004-orchestrator-observes-and-steers.md),
  [0005](../decisions/0005-micro-model-decides-stalled-loops-only.md), [0009](../decisions/0009-subagents-are-bounded-readers.md),
  [0010](../decisions/0010-routing-between-slots-by-rules-and-a-small-classifier.md),
  [0011](../decisions/0011-multi-agent-orchestration-in-sequence-first.md),
  [0015](../decisions/0015-laya-is-the-decision-backend-in-place-of-jev.md)), then other tools and papers were surveyed
  ([section 2](#2-what-other-tools-do)), and the memory arithmetic was worked out from the model files on this machine. Many
  sources are secondary write-ups or vendor claims; each one is labelled where it matters.
- **What this plan can and cannot promise.** It cannot prove that "all outcomes" work: nobody can. It enumerates the failures and
  workflows that can be predicted, gives each a detection, a recovery and a test that forces it, and commits to one rule for
  the rest: **an unexpected failure ends in a safe, inspectable state, and nothing is deleted without a copy.**

## 1. Feature list

| ID | Feature | Size | Needs | Why |
|---|---|---|---|---|
| F0.1 | **Slot readiness.** A slot is `warming` until its server answers, and only then `running`. | small | nothing | Found on 2026-11-04: the window showed `running` while the server answered HTTP 503 "Loading model" (`server_state` only checks that the process is alive). |
| F0.2 | **Model scheduler.** One component owns every start, stop and swap of a model server, with a queue, a reason for each swap, and a guaranteed restore. | medium | F0.1 | Swaps are in `workflow.rs` (deep escalation) and the window; the fleet would add more callers. |
| F0.3 | **Run and agent identity.** Every event and journal record carries `run`, `agent`, `role` and `parent`. | small | nothing | A multi-agent trace cannot be read without it. |
| F0.4 | **Git layer.** Checkpoints, worktrees, a repository lock, a write-ahead log, preflight checks, undo and apply. | large | F0.3 | The first answer to "how do agents share files" ([3.5](#35-the-same-file-and-the-same-resources-on-top-of-git)). |
| F0.5 | **Leases and write checks.** A claim table for paths and tool resources, and a base-version check on every write. | medium | F0.3 | The second answer ([3.5](#35-the-same-file-and-the-same-resources-on-top-of-git)). |
| F0.6 | **Recovery and `bridle doctor --repair`.** Start-up reconciliation of runs, leases, worktrees, locks and the swap record; a documented manual runbook. | medium | F0.2, F0.4, F0.5 | "If something crashes we should be able to work on it" ([3.6](#36-what-can-fail-and-what-happens)). |
| F0.7 | **Handoff envelope and artifact store.** Typed messages between agents and documents in a run folder that a harness tool writes. | medium | F0.3 | How agents share results ([3.9](#39-how-agents-pass-work-to-each-other)). |
| F1 | **Sub-agents on a small-model fleet.** Several small-model agents run as parallel slots of one GPU server, managed by a small manager model, with Qwen3.5-9B unloaded while they work. | large | F0.1 to F0.7 | The first queued feature. |
| F2 | **Teams, roles and members.** Six teams, any number of named members per role, tagged with `@`, assigned by rules or by the user. | large | F1 | The second queued feature. |
| F2.1 | **Crew rail.** Avatars of all members on the right of the window with status and badges; click to tag, hold to monitor. | medium | F2 data | Requested 2026-11-04. |
| F2.2 | **Board.** Issues with owners, tracks, dependencies, a ready queue, comments and Git links; members create and work them through harness tools. | large | F2 data, F0.3 | Requested 2026-11-04: a project manager splits an issue into issues for other teams. |
| F3 | **Ideas queue.** Notes saved while a run is working, shown top right, sent when the run ends. | small | nothing | Independent; can ship first. |
| F4 | **Web search for literature review.** Opt-in, policy-gated search through the proxy setting. | medium | policy for network tools | Queued earlier; gives research members their tools. |
| F5 | **Window polish and the new panels** (agents, team, ideas, runs). | medium | F1 to F3 | Queued earlier. |
| F6 | **Server-side context save and restore** around swaps (llama.cpp slot save and restore). | small | F0.2 and a measurement | Optimisation only. |

Not planned yet, and said so: an agent-to-agent network protocol, the LFM3 vision, audio and omni models, and writers on separate
machines.

## 2. What other tools do

### Agents, roles and orchestration

| Source | What it says | What we take from it |
|---|---|---|
| [Claude Code subagents](https://code.claude.com/docs/en/sub-agents) (primary docs) | A subagent is a Markdown file with `name`, `description`, `tools`, `model`, `permissionMode`, `maxTurns` and `isolation: worktree`. It has its own context and returns a summary. `@agent-name` forces one, but the parent still writes the task. Nesting to three layers; 20 at once by default. | The shape of a role, the `@` interaction, depth and concurrency caps, worktree isolation. |
| [Anthropic, multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system) (primary) | Orchestrator-worker. A delegation needs an objective, an output format, tool guidance and boundaries. Results can go to storage and come back as a reference. About 4 times the tokens of chat for an agent and 15 times for multi-agent. Most coding is a poor fit. | A four-part delegation brief; results by reference; budgets; sequential by default for code. |
| [Cognition, Don't Build Multi-Agents](https://cognition.com/blog/dont-build-multi-agents) and [What's Actually Working](https://cognition.com/blog/multi-agents-working) (primary) | Actions carry implicit decisions, so parallel writers conflict. What works: single writer, a clean-context reviewer, a stronger model consulted as a tool, a manager that splits work and coordinates children. | Single writer by default; reviewer with a clean context; deep model as a consult; a manager. |
| [MetaGPT](https://arxiv.org/pdf/2308.00352.pdf) | Roles (product manager, architect, project manager, engineer, QA) follow procedures and pass documents, not chat. | Roles with fixed outputs; handoffs as documents. |
| [LLM multi-agent systems for software engineering](https://arxiv.org/pdf/2404.04834), [Liu et al.](https://arxiv.org/pdf/2409.02977) | Common roles: orchestrator, programmer, reviewer, tester, retriever. Topologies: centralised, decentralised, hierarchical, nested. | A hierarchy is a recognised pattern. |
| [CrewAI, LangGraph, AutoGen compared](https://www.jetbrains.com/pages/ai-agents/building/agent-framework-comparison/) | CrewAI's hierarchical process has a manager that assigns and can reject work; LangGraph builds the supervisor as an explicit graph. | An explicit graph of allowed handoffs, not free conversation. |
| [Scrum Guide](https://scrumguides.org/scrum-guide.html) | Product Owner accountable for value and the backlog; Developers make a done increment and uphold the Definition of Done. | The product owner, the engineers and the "done" gate. |
| [NVIDIA ToolOrchestra](https://research.nvidia.com/labs/lpr/ToolOrchestra) | An 8B model trained with reinforcement learning to coordinate tools and other models. Based on Qwen3-8B. NVIDIA License, research and development only. | A design reference. Not a candidate: it is the size of the model it would replace and its licence is restrictive. |
| [Small language models are the future of agentic AI](https://arxiv.org/abs/2506.02153) (via summaries) | Small models by default, large on demand, with a router. | Matches ADR-0010 and the fleet design below. |

### Git, files and recovery

| Source | What it says | What we take from it |
|---|---|---|
| [Cline checkpoints](https://docs.cline.bot/features/checkpoints) | A separate "shadow" Git repository, so the project's history is untouched. A snapshot after each tool use, including untracked files; restore files only, or files and conversation. | Checkpoint after every step, outside the user's branch; undo is a first-class action. |
| Aider (secondary blog only) | Makes each edit a real commit and exposes `/undo`. | The opposite choice; we keep checkpoints private and commits for the user. Not verified from a primary source. |
| [Jujutsu operation log](https://martinvonz.github.io/jj/v0.10.0/operation-log/) | Every repository-changing operation is recorded with the state it produced and can be undone; the working copy is snapshotted at the start of each command; conflicts can be committed and resolved later. | A write-ahead operation log and undo by operation id. We stay on plain Git so the user's tools keep working. |
| [MCP Agent Mail](https://github.com/Dicklesworthstone/mcp_agent_mail) | A coordination layer for coding agents: identities, mailboxes, **advisory file reservations with expiry**, Git-backed archives, and a pre-commit guard that blocks commits touching files reserved by another agent. | Leases with a time to live and renewal; a mailbox on disk; a Git-level guard. Its reservations are advisory, ours are enforced by the file tools. |
| [Git worktrees for parallel agents](https://augmentcode.com/guides/git-worktrees-parallel-ai-agent-execution) (secondary) | Each agent gets its own working directory and index and shares one object store. Worktrees do not catch semantic conflicts; shared toolchains still contend. | Worktrees for parallel writers; a verification after every merge; leases for build tools. |
| [git-worktree-runner issue 180](https://github.com/coderabbitai/git-worktree-runner/issues/180) (a user report, closed as fixed) | Claude Code's agent worktrees were left **locked** after an abrupt end; `git worktree prune` skips locked entries; the fix is unlock, prune, delete the branch. | Never lock worktrees; record every worktree in our own manifest; reconcile at start-up. |
| Git behaviour (git documentation; guides) | A stale `index.lock` after a crash is removed only when no Git process is running; removing one in use can corrupt the index. | Our own Git calls go through one gate with retry; locks are never deleted automatically. |

### Memory

| Source | What it says | What we take from it |
|---|---|---|
| [Letta shared memory blocks](https://docs.letta.com/tutorials/shared-memory-blocks) | Agents can attach the same memory block and read and write it, next to their private blocks. Every memory operation costs model tokens. | Private and shared tiers; avoid making a small model manage its own memory. |
| Mem0 and blackboard comparisons (vendor and blog) | Three patterns: a blackboard (shared state, needs locking), message passing (no shared state, must ask), and shared vector stores. One design adds provenance (writer, time, prompt hash) to each entry. | A run blackboard with provenance, written through a harness tool; no vector store in version 1. |

### Models

| Source | What it says | What we take from it |
|---|---|---|
| [llama.cpp server](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md), [llama-swap](https://github.com/mostlygeek/llama-swap) | Parallel slots (`-np`) share one set of weights and one KV buffer; router mode loads models on request; llama-swap swaps by groups with exclusive and persistent flags; slot save and restore needs matching settings. | The fleet is **one server with several slots**, not several servers; Bridle keeps its own scheduler because it has the VRAM guard. |
| [LFM2.5-1.2B-Instruct card](https://huggingface.co/LiquidAI/LFM2.5-1.2B-Instruct) (primary) | 1.17B, 16 layers (10 convolution, 6 attention), 32K context. Recommended for agentic tasks, data extraction and RAG; **not recommended for knowledge-intensive tasks or programming.** Python-style tool calls by default. IFEval 86.23, BFCLv3 49.12 (Liquid's own handler). | Good bounded worker; weak at code. |
| [Qwen3.5-4B card](https://huggingface.co/Qwen/Qwen3.5-4B) and [GGUF sizes](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF) (primary) | 4B, 32 layers, **8 attention layers with 4 KV heads of 256** (the same cache shape as the 9B), 262K context, Apache-2.0. BFCL-V4 50.3, TAU2-Bench 79.9. Q4_K_M file 2.74 GB. | The leading manager candidate. |
| [Qwen3.5-2B card](https://huggingface.co/Qwen/Qwen3.5-2B) (primary) | 2B, 24 layers, 6 attention layers with 2 KV heads of 256, Apache-2.0. BFCL-V4 43.6, TAU2-Bench 48.8 (thinking mode). | A fallback if memory is short; probably too weak to plan. |
| [LFM3 family](https://www.layer3labs.io/guides/lfm3-explained) (secondary; **all figures are Liquid's own, unreplicated**) | LFM3-1.7B: 256K context; BFCL V4 71.39 against 62.83 for Qwen3.5-9B, ToolSandbox 80.36 against 76.41, SWE-bench Verified (OpenCode) 43.40 against 51.20. Two "decision" variants (d1-2B) answer with typed, scored answers (Decision Bench 85.2). Needs a newer llama.cpp. | LFM3-1.7B as a possible manager and worker in one server; d1-2B as a possible router. Both must be measured here first (ADR-0010 found LFM3-1.7B answers `ASK_USER` to every stalled loop). |
| [Granite-4.0-H-Micro](https://huggingface.co/ibm-granite/granite-4.0-h-micro) and [H-1B](https://huggingface.co/ibm-granite/granite-4.0-h-1b) (primary cards) | Apache-2.0, 128K context, tool calling, **hybrid: 4 attention layers and 36 Mamba2 layers**, so the cache per token is small. Micro (3B): HumanEval 81, HumanEval+ 75, MBPP 73, BigCodeBench 37.9, BFCL v3 57.56. H-1B: HumanEval 73, MBPP 69, BigCodeBench 29.12, BFCL v3 50.21. GGUF Q4_K_M 1.94 GB and 901 MB. | The best-documented small coders found. See 3.4. |
| [Qwen2.5-Coder-1.5B-Instruct](https://huggingface.co/Qwen/Qwen2.5-Coder-1.5B-Instruct) (primary) | Apache-2.0, 1.54B, 28 layers, 2 KV heads, 32K context. The card gives no numbers and does not mention tool calling; it names code agents as a target. (The 3B is under the Qwen research licence, not Apache.) | A code-specialised fallback for patch-style edits; not for tool loops without a test. |
| [SmolLM3-3B](https://huggingface.co/HuggingFaceTB/SmolLM3-3B) (primary) | Apache-2.0, 3B, tool calling; LiveCodeBench v4 15.2 (30.0 with thinking); BFCL 92.3. | Strong at tool use, weak at coding: not a coder. |
| Qwen3.5-0.8B and 2B (primary cards) | 0.8B: BFCL-V4 25.3, TAU2 11.6. 2B: BFCL-V4 43.6, TAU2 48.8. No coding benchmark is listed for either. | The 0.8B is too weak; the 2B is a cheap generalist to test. |
| [Beads](https://github.com/steveyegge/beads) | A Git-backed issue tracker for coding agents: hash-based ids (`bd-a1b2`) that do not collide across agents or branches, hierarchical ids for epics and tasks, dependency links, a **ready** list of tasks with no open blocker, an atomic **claim**, an audit trail, and JSON output. Stored in Dolt, with a JSONL export. | The shape of the board: hash ids, hierarchy, ready queue, atomic claim. We keep plain files instead of a database. |
| [Backlog.md](https://github.com/MrLesk/Backlog.md) | One Markdown file per task in the repository; fields for description, acceptance criteria, **Definition of Done**, dependencies, comments, plan, notes and a final summary; archive instead of delete; a terminal and a web Kanban; an MCP server and JSON output for agents. | Acceptance criteria and a Definition of Done per issue; an export to Markdown; a board view. |
| [LFM Open License](https://www.liquid.ai/lfm-license) | Commercial use is free below US$10 million annual revenue; non-profits are exempt for research use. | The models guide says "research use"; that wording is narrower than the licence and should be corrected. |
| Claude Code and Codex message queues ([issue 49373](https://github.com/anthropics/claude-code/issues/49373) and user reports) | A message typed while the agent works is queued and sent at the end of the turn; users ask for a visible, editable queue with "send now". | The ideas queue. |

Five conclusions drive the design:

1. **Code is not research.** Parallel writers conflict through decisions, not only bytes: one writer at a time by default.
2. **Share decisions, not transcripts.**
3. **Communicate through documents,** which can be validated, logged and replayed.
4. **Git is the safety net, not the lock manager.** Git gives cheap snapshots, branches and merges. It does not stop two agents
   from editing one file, and it has its own locks that crashes leave behind. So Bridle adds leases on top and a recovery step
   below.
5. **Cost is real.** Multi-agent runs multiply tokens, so every run has a budget and the user sees where the time goes.

## 3. The design

### 3.1 Terms

| Term | Meaning |
|---|---|
| **Run** | One request from the user, with everything it causes. Has an id, a budget, a journal and an owner. |
| **Team, role, member** | A **team** is a department (Engineering). A **role** is a position in it (Senior Engineer, Intern). A **member** is a named individual in a role, with its own prompt and speciality. Definitions, not processes. |
| **Agent** | One member working inside a run: a context, a model slot, a tool list and a budget. |
| **Instance / slot** | The seat on a model server that an agent uses. Several members can be queued for few slots. |
| **Handoff** | A typed message from one agent to another, with references to artifacts. |
| **Artifact** | A document in the run folder: plan, requirements, design note, review, pitch. |
| **Lease** | A claim on a path or tool resource, held by one agent. |
| **Checkpoint** | A snapshot of the working tree, stored as a Git commit under `refs/bridle/`, never on a branch. |
| **Scheduler** | Deterministic code that decides which agent runs next and which model servers are loaded. Not a model. |

### 3.2 What can actually run at the same time

The machine is the limit (RTX 4060 8 GB, 16 GB RAM).

| Resource | Capacity | Consequence |
|---|---|---|
| GPU memory | 8,188 MiB. `main` (Qwen3.5-9B, 65,536 tokens) uses 6,499 MiB in all, about 230 MiB of it the desktop; `deep` uses 6,552 MiB. | `main` and `deep` never coexist, and **`main` and any fleet cannot coexist** (6,499 + about 2,100 is more than 8,188). |
| System memory | 16 GB; about 382 MB free with `main` running. | Another process on the CPU is tight. |
| Swap time | About 20 s for a Qwen, Bonsai, Qwen swap (2026-10-28); `main` alone starts in about 11 s. | Batch work by model. |
| Files and Git | One working copy, one index, one set of refs. | Leases, a Git gate, worktrees for parallel writers. |

So there are three **modes**, chosen per run by the scheduler's rules (and by a setting):

| Mode | GPU holds | Use |
|---|---|---|
| **Solo** | Qwen3.5-9B | Today's behaviour. Hard reasoning and code. Agents take turns. |
| **Fleet** | A small manager model plus one LFM server with several slots | Many bounded small tasks in parallel. Qwen3.5-9B is unloaded. |
| **Swap** | Solo, then Fleet, then Solo | The 9B plans and reviews; the fleet executes; the 9B compiles the answer. Two swaps, about 11 to 20 s each. |

### 3.3 The scheduler, and the operating system comparison

An operating system does not ask a program which program should run next. A deterministic scheduler does. The same split is used
here.

| Operating system | Bridle |
|---|---|
| Process | Agent run (a member with its own context). |
| Scheduler | The model scheduler (F0.2): picks the next agent, groups agents that need the same servers, decides when a swap pays for itself. Rules, no model. |
| Context switch | Mode change or swap. Bridle, not the server, owns each agent's history, so a swap loses only the server's cache. |
| System call | A tool call through the policy hook. |
| File locks | Leases ([3.5](#35-the-same-file-and-the-same-resources-on-top-of-git)). |
| Inter-process communication | The mailbox: typed handoffs in an append-only log. |
| Supervisor | The run owner stops any agent; Esc stops the run; budgets end an agent that overruns. |
| Memory limit | The VRAM guard (already built). |
| Journal and recovery after a crash | The write-ahead log and start-up reconciliation ([3.6](#36-what-can-fail-and-what-happens)). |

**Scheduling policy (proposed).**

1. The next step is the next one in the run's plan, but steps that do not depend on each other may be reordered to put those that
   need the loaded servers together.
2. A swap needs a reason the rules can name (ADR-0010). Over the swap budget (default 2 per run) the step runs on what is loaded.
3. **Restore is guaranteed.** Every mode change writes a swap record before it acts (what was loaded, what to restore). The
   restore runs in a cleanup path on stop, error and a failed start; if the process dies, the next start reads the record and
   restores. The one-GPU-set check and the VRAM guard stay in force.
4. Esc during a swap lets the step in progress finish, then restores.

**What the server remembers.** After a reload the first request re-reads the whole prompt, which can be tens of thousands of
tokens. Slot save and restore could skip that, but reports say a restore silently re-reads when settings differ, so it is F6:
measure first. Correctness never depends on it.

### 3.4 Small models on the GPU: the fleet and its manager

**The request:** use LFM for all sub-agents, on the GPU, with Qwen3.5-9B out for that time, and find a smaller model than the 9B
that can manage them. This section is about models only. It has nothing to do with teams and roles.

#### One server with several slots, not several servers

"Multiple instances of LFM" is done as **one `llama-server` with `-np N` slots**. Bridle's launcher already passes `-np` (the
`launch.parallel` key), so no new launcher code is needed.

| | N servers | One server, N slots |
|---|---|---|
| Weights in VRAM | N copies | one copy |
| CUDA context and buffers | N | one |
| Throughput | N separate decodes | continuous batching merges the slots' decode steps |
| Per-agent prompt cache | per server | pinned per slot: each member always uses the same slot id, so its prompt prefix stays warm |
| Limit | memory | slot count is fixed when the server starts; every slot has the same model |

#### The memory, worked out from the files

Cache per token comes from the GGUF header of the LFM files on this machine and from the model cards (8-bit cache, 1.0625 bytes per
element).

| Model | Attention layers | KV per token (8-bit cache) | File (Q4_K_M) |
|---|---|---|---|
| LFM2.5-1.2B | 6 of 16, 8 KV heads of 64 | **6,528 bytes** | 0.73 GB |
| Qwen3.5-2B | 6 of 24, 2 KV heads of 256 | 6,528 bytes | to check |
| Qwen3.5-4B | 8 of 32, 4 KV heads of 256 | 17,408 bytes (same as the 9B) | 2.74 GB |
| Qwen3.5-9B | 8 of 32, 4 KV heads of 256 | 17,408 bytes | 5.68 GB |

The estimate uses the same formula as the guard in `harness_runtime::vram` (weights at 85 % of the file, a cache, and 736 MiB of
buffers, which was calibrated on the 9B and is an upper bound for small models):

| Configuration | Weights | Cache | Buffers | Total |
|---|---|---|---|---|
| Manager: Qwen3.5-4B Q4_K_M, 32,768 tokens | 2,221 MiB | 544 MiB | 736 MiB | **3,501 MiB** |
| LFM2.5-1.2B Q4_K_M, 8 slots of 16,384 tokens (one 131,072-token pool) | 592 MiB | 816 MiB | 736 MiB | **2,144 MiB** |
| **Fleet total, plus about 230 MiB of desktop** | | | | **about 5,875 MiB of 8,188 MiB** |
| For comparison, Qwen3.5-9B alone, 65,536 tokens | | | | 6,499 MiB (measured) |

About 2,300 MiB would remain free, above the 768 MiB reserve. These are estimates from file headers and have not been run. Whether
eight slots give eight times the work is **not measured**; it is the first measurement in phase P0.

#### Who manages the fleet

The manager plans (at most five steps, as today), assigns steps to agents, checks each report against its acceptance, and decides
a retry or an escalation. It writes JSON that code validates. It is a model, so it never controls resources: the scheduler does
that ([3.3](#33-the-scheduler-and-the-operating-system-comparison)).

| Candidate | Size | Evidence | For | Against |
|---|---|---|---|---|
| **Qwen3.5-4B** (Apache-2.0) | 2.74 GB at Q4_K_M | Model card: BFCL-V4 50.3, TAU2-Bench 79.9. The 9B scores 66.1 on BFCL-V4 (secondary). | Same family, chat template, tool parser and cache shape as the 9B Bridle already runs, so the VRAM guard and the provider already understand it. Runs on the installed llama.cpp b11756 if the 9B does (to confirm). | BFCL-V4 is 16 points below the 9B. |
| **LFM3-1.7B** (LFM Open License) | about 1.0 GB at 4 bits | Liquid's own: BFCL V4 71.39, ToolSandbox 80.36, IFEval 96.66; 43.40 on SWE-bench Verified with OpenCode against 51.20 for the 9B. Not independently replicated. | The smallest: one model could be both manager and workers in one server. 256K context. | Needs a llama.cpp build newer than b11756. Our ADR-0010 test found it answered `ASK_USER` to every stalled loop. The card does not recommend it for complex multi-step reasoning. |
| **LFM2.5-1.2B-Instruct** | 0.73 GB | Card: IFEval 86.23, BFCLv3 49.12; recommended for agentic tasks. | Already on disk; works on b11756. | BFCLv3 is not comparable to V4. The 1.2B Thinking variant agreed with the rules on only 5 of 16 stalled cases (ADR-0010). |
| **Qwen3.5-2B** | about 1.3 GB (to check) | BFCL-V4 43.6, TAU2-Bench 48.8. | Same family. | Weakest of the Qwen options. |
| **d1-2B** (LFM3 "decision" variant) | about 2B | Vendor: Decision Bench 85.2; typed answers with scores. | Fits the closed-list choices (a router, like Laya), not planning. | The call format is not documented in the sources read; needs the newer runtime. |
| Nemotron-Orchestrator-8B | 8B | NVIDIA: strong on its benchmarks. | Purpose-built orchestrator. | The size of the 9B; NVIDIA License, research use only; not tested on coding. **Not a candidate.** |
| Phi-4-mini, SmolLM3, Granite 4.0 micro, Qwen3-4B-2507 | 3 to 4B | No reliable comparison found. | | Check only if the above fail. |

**Recommendation: measure two fleets against the 9B on the same tasks, then choose.**

1. **Fleet-Q:** manager Qwen3.5-4B + LFM2.5-1.2B-Instruct workers (8 slots). Needs no new runtime.
2. **Fleet-L:** one LFM3-1.7B server as manager and workers. Needs the runtime upgrade; the smallest and fastest if it holds up.
3. **Baseline:** Qwen3.5-9B alone (Solo), on the existing evaluation corpus in `evals/tasks` and on the role set added in P3.

Measured: plan validity, handoff validity, tool-call success, task pass rate, tokens per second in aggregate, and VRAM. A fleet is
adopted for a kind of task only where it is not worse than the baseline on that kind. **No decision is made from vendor numbers.**

#### What the fleet is good for, and what it is not

The LFM card says **it is not recommended for programming or knowledge-intensive tasks.** So the fleet takes the bounded work and the
9B keeps the hard work:

| Fleet (parallel, small context) | Qwen3.5-9B (Solo or Swap) |
|---|---|
| Read named files and report facts with citations (the existing reader, ADR-0009) | Design and architecture |
| Run the configured checks and summarise failures (the failure parser is deterministic) | A change across several modules |
| Search the code and list matches; summarise a diff | Debugging that needs reasoning across files |
| Mechanical, exactly-specified edits (rename, add a documented parameter everywhere, fix formatting) | Review with judgement |
| Draft documentation and comments, write test skeletons from a signature | Writing the final answer for the user |
| Triage and classification | |

This is the answer to "all sub-agents use LFM": all fleet sub-agents do, but only on the work above. A fleet step that fails twice
is **escalated**: the scheduler records why, and a swap back to the 9B runs it, if the swap budget allows. If it does not, the run
stops and says so.

#### Small models that can code (survey, 2026-11-04)

The LFM card says LFM is not for programming, so the worker rig needs at least one model that is. The search found **no standard
agentic-coding score (SWE-bench) for any model under 4B**, and one source notes that agentic scores stay low for every model under 15B.
What exists are function-level scores from the model cards, which say less than a repository edit does. So the candidates are ranked
on what is documented, and **all of them must be measured on this project's own tasks before use.**

| Model | Size (Q4_K_M) | Coding numbers on the card | Tools | Licence | Cache per token (8-bit) |
|---|---|---|---|---|---|
| **Granite-4.0-H-Micro** (IBM) | 3B, 1.94 GB | HumanEval 81, HumanEval+ 75, MBPP 73, BigCodeBench 37.9 | yes, BFCL v3 57.56 | Apache-2.0 | **4,352 B** (4 attention layers, 8 KV heads of 64) |
| **Granite-4.0-H-1B** (IBM) | 1.5B, 901 MB | HumanEval 73, HumanEval+ 68, MBPP 69, BigCodeBench 29.1 | yes, BFCL v3 50.21 | Apache-2.0 | **4,352 B** (4 attention layers, 4 KV heads of 128) |
| Qwen3.5-4B (the manager) | 4B, 2.74 GB | LiveCodeBench v6 65.6 (a third-party guide; the card lists BFCL-V4 50.3) | yes | Apache-2.0 | 17,408 B |
| Qwen2.5-Coder-1.5B-Instruct | 1.54B | none on the card | not stated | Apache-2.0 | about 15,200 B (28 layers, 2 KV heads of 128) |
| Qwen3.5-2B | 2B | none listed; BFCL-V4 43.6 | yes | Apache-2.0 | 6,528 B |
| SmolLM3-3B | 3B | LiveCodeBench v4 15.2 (30.0 thinking) | yes | Apache-2.0 | to check |
| LFM2.5-1.2B | 1.17B, 0.73 GB | not for programming (card) | yes | LFM Open License | 6,528 B |
| LFM3-1.7B | 1.73B | vendor: SWE-bench Verified (OpenCode) 43.40 | yes | LFM Open License | to check |

**Choice for the worker rig: two coders from one family and a reader from another, so that not everything rests on one model.**

1. **Granite-4.0-H-Micro (3B)** as the **coder**: the strongest documented code scores of the small models, tool calling, 128K
   context, Apache-2.0. Its hybrid design keeps only 4 layers of cache, so several parallel slots are cheap: four slots of 16,384
   tokens cost about 272 MiB of cache.
2. **Granite-4.0-H-1B** as the **fast hands** for mechanical edits and many small parallel tasks: 901 MB.
3. **LFM2.5-1.2B-Instruct** as the **reader**: extraction, summaries, checks, triage, where its card says it is strong. It can also run on
   the CPU while the GPU holds the coder, so reading and coding really overlap.
4. **Qwen3.5-4B** stays the **manager**.
5. Not chosen: SmolLM3 (weak at code), Qwen3.5-0.8B (too weak), Qwen2.5-Coder-1.5B (kept as a patch-style fallback if the Granite
   models fail on this project), and Qwen2.5-Coder-3B (research licence, not Apache).

**What the GPU can hold.** Each separate model server costs its own buffers (the guard's 736 MiB is calibrated on the 9B and is an
upper bound for small models), so the number of **distinct GPU servers** is the limit on this card, not the number of slots:

| Rig | Servers on the GPU | Estimated use | Free of 8,188 MiB |
|---|---|---|---|
| **A** | Qwen3.5-4B manager (32,768 tokens) + Granite-H-Micro, 4 slots of 16,384 | 3,501 + 2,581 + 230 desktop = **6,312 MiB** | 1,876 MiB |
| **B** | Qwen3.5-4B manager + Granite-H-1B, 4 slots of 16,384 | 3,501 + 1,738 + 230 = **5,469 MiB** | 2,719 MiB |
| **C** | A, plus LFM2.5 on the **CPU** (8 threads) for readers | as A; the CPU model uses system memory (about 1 GB) | 1,876 MiB |
| A + a second GPU worker server | | about 7,800 MiB or more: **the guard refuses it** | under 768 MiB |

(Granite-H-Micro: weights 1,850 MiB at 85 % = 1,573, cache 65,536 tokens × 4,352 B = 272 MiB, buffers 736. Granite-H-1B: 730 + 272 +
736. Estimates from the cards and file sizes, not runs. The recurrent state of the Mamba layers is a small constant per slot that the
estimate does not yet include.)

So on this machine the rig is **a manager plus one worker model on the GPU at a time**, with the worker model chosen per kind of task
(coder, fast hands) and a reader on the CPU. Changing the worker model is a start of a small server (about 1 to 2 GB of weights),
much cheaper than a 9B swap, and the scheduler batches tasks by worker model to keep changes few. A bigger GPU would hold more
servers; the guard decides, not a constant. **llama.cpp support for the Granite hybrid architecture on the installed b11756 is to be
confirmed in P0.**

### 3.5 The same file and the same resources, on top of Git

#### Four rules about the working copy

1. **Readers are free; a writer takes a lease.** A lease has one holder per path.
2. **Sequential first.** In version 1 engineers run in turn in the user's working copy. This is the Cognition and Anthropic
   position, and it is all one GPU can do in Solo mode.
3. **The user's branch is not touched unless the user presses Apply.** Agents never commit, switch branches, reset, stash or push
   on the user's branch during a run (the policy gains a rule, below). The harness keeps checkpoints in a private ref namespace.
4. **Fail closed, and never delete without a copy.** Anything Bridle removes (a worktree, a branch, a lock record) has a
   checkpoint or a recorded reason first.

#### Checkpoints (shared-tree mode)

Before a step starts and after it ends, the harness takes a snapshot of the working tree **without touching the user's index, HEAD
or branch**, using Git plumbing and a temporary index:

```bat
:: Bridle does this for you; shown so that it can be done by hand
set GIT_INDEX_FILE=%TEMP%\bridle-index
set GIT_AUTHOR_NAME=Bridle& set GIT_AUTHOR_EMAIL=bridle@localhost
set GIT_COMMITTER_NAME=Bridle& set GIT_COMMITTER_EMAIL=bridle@localhost
git read-tree HEAD                                :: skipped when the repository has no commit yet
git add -A                                        :: honours .gitignore; Bridle also leaves out secret-named files
for /f %%t in ('git write-tree') do set TREE=%%t
for /f %%c in ('git commit-tree %TREE% -p HEAD -m "bridle checkpoint r-12 step 3"') do set C=%%c
git update-ref refs/bridle/runs/r-12/step-003 %C%
```

- The checkpoint is a commit object under `refs/bridle/`. It does not appear in `git log` of any branch, `git push` does not send it
  (only branches and tags are pushed by default; `git push --mirror` would, and the runbook says so), and `git gc` keeps it because
  a ref points to it. Blobs are shared with the repository, so a step costs only its changed files.
- Hooks and signing are not run (plumbing). The author is `Bridle <bridle@localhost>`.
- **Undo a step** is done from the harness's own write log: every file write records `(path, hash before, hash after)`. To undo,
  each file whose current hash still equals "after" is restored to "before". A file that changed again (the user typed in it) is
  **not** overwritten and is reported. The checkpoint diff catches changes made by commands (a formatter), which the write log cannot
  see; those are reported and can be restored from the checkpoint by the user.
- **A folder with no Git repository** gets the same snapshots in a shadow repository, `.bridle/shadow.git`, used with
  `--git-dir` and `--work-tree`, as Cline does. Parallel worktrees are not available there.
- Size limits: files over 5 MB are listed, not stored, and a step that would store more than 200 MB stops and asks. Both are
  settings.

#### Worktrees (parallel-writer mode, a later step of F1)

Each engineer gets its own worktree and branch from the run's base commit, so no two agents share a file, an index or a branch:

```bat
git worktree add -b bridle/r-12/eng-maya .worktrees\r-12-maya <base-commit>
```

- Bridle already creates worktree tasks (`bridle task start --worktree`, `.worktrees/agent-task-NNN`). Names are kept short for
  Windows path limits.
- The engineer's edits are committed **by the harness** on its own branch with plumbing, at each step end. The author is the
  member's name, and the commit message says which run and step.
- **Integration** is done in a separate integration worktree by the technical manager. Before merging, `git merge-tree --write-tree`
  (Git 2.38 or newer) computes the merge **without touching any working tree** and reports the conflicting files. A clean result is
  merged; a conflict goes to the technical manager (with the 9B as a consult when configured), then to the user. After each merge
  the configured verification runs, because Git cannot see a semantic conflict such as a changed function signature.
- **Apply** puts the integration branch onto the user's branch only when the user asks, as a squash (default), a merge, or an
  uncommitted patch. The user's own commit runs their hooks and signing as usual.
- The user's working tree is not touched at all until Apply, which is why this mode is the safe one for big changes.
- At most two parallel writers (memory), and only after the sequential mode is measured.
- Shared build output: by default the worktrees share one build folder and the `verify` lease serialises builds (Cargo already
  waits on its own build-directory lock); a setting gives each worktree its own, which costs disk.

#### Locks and who may run

| What | Mechanism |
|---|---|
| **One write-run per repository** | An exclusive operating-system file lock on `.bridle/repo.lock`, taken with the standard library's file locking. It is released by the system when the process dies, so a crash cannot leave it stuck. A second Bridle window or the CLI on the same repository sees it, says "another Bridle is running a team here", and is read-only. |
| **Run ownership** | The same kind of lock on `.bridle/runs/<run>/owner.lock`. A recovering process that can take it knows the owner is dead. |
| **Leases** | `.bridle/leases.json`, changed only while holding `.bridle/leases.lock`, written atomically. Each entry: path or resource, holder, run, step, a sequence number, and a time to live that the holder renews. Time to live uses a monotonic clock plus the sequence number, so a clock change cannot expire or extend a lease wrongly. |
| **Path equality on Windows** | Paths are resolved, then compared **case-insensitively** and without trailing dots and spaces, so `Src\Lib.rs` and `src/lib.rs` are one lease. Reserved device names and alternate data streams are already refused by the policy. |
| **Stale-read check** | `read_file` returns the file's hash; `write_file` and `edit_file` accept it and refuse a write whose hash no longer matches: "changed since you read it". This catches the user typing at the same time. It does not exist in the file tools today. |
| **Atomic writes** | The file tools already write atomically (the old content or the new). A rename that fails because another program holds the file (an antivirus scan, an editor) is retried three times with a short back-off and then fails with the system's message. |
| **The Git gate** | All Git commands Bridle runs go through one mutex per repository with a retry and back-off on `index.lock` and ref locks. Read-only commands run with `--no-optional-locks`, so `git status` from an agent does not take the index lock that the user's editor and terminal also want. Bridle never deletes a lock file by itself. |
| **Tool resources** | `verify` (test runner and build) is one lease per work area. |
| **The user is a writer** | A file with unsaved edits in the editor is held by the user; an agent claiming it is refused. |

#### What the policy adds for Git during a team run

The command classifier already treats `reset --hard`, `clean`, `checkout --`, `restore`, `rebase`, `branch -D`, `push`,
`update-ref -d`, `gc --prune`, `stash drop` and history rewrites as destructive or network, and `git worktree` as unrecognised
(denied). It treats `git add`, `commit`, `stash`, `switch`, `checkout -b` and `tag` as writes (allowed). During a team run:

- In shared-tree mode, **those writes are refused too**: the user's branch is only changed by Apply. The refusal names the rule.
- In worktree mode a Git command may name only the member's own branch; a command that names any other ref is refused.
- A new check ties these refusals to the run, with tests for each (a model that runs `git commit` in the middle of a run must be
  blocked).

#### Preflight: when a run will not start

| Repository state | Behaviour |
|---|---|
| In the middle of a merge, rebase, cherry-pick, revert or bisect (`MERGE_HEAD`, `rebase-merge`, ...) | **Refuse to start a write run**, say which, and how to finish or abort it. |
| Unresolved conflicts in the index | Refuse. |
| Uncommitted changes of the user | Allowed. The changed files are recorded as the user's baseline and are not leasable unless the user opts in. |
| Detached HEAD or an unborn branch | Allowed. The base is the HEAD commit, or the empty tree. |
| Submodules, sparse checkout, Git LFS pointers | Sequential mode works on the files as they are; **worktree mode is refused**, with the reason. |
| Shallow clone | Worktrees work; a merge base that is missing makes Apply fall back to a patch. |
| Repository inside a cloud-synced folder (OneDrive and the like) | Warn: file locking and renames are unreliable there. |
| Git older than 2.38 | Worktree integration unavailable; sequential mode works. `bridle doctor` says so. |

### 3.6 What can fail, and what happens

Every row has a test that forces it ([3.7](#37-tests-that-prove-it)).

#### Journal and recovery

Every state change of a run is a line in `.bridle/runs/<run>/journal.jsonl`: sequence number, time, kind, details and a checksum.
Changes that act on the world are logged **before** they act (an intent) and again **after** (a result), and the log is flushed to
disk each time. This is a write-ahead log, as in a database. On start-up the recovery step reads each run's journal; a torn last
line (checksum fails) is treated as not written; an intent without a result is either **repeated** (every action is idempotent:
creating a worktree checks that it exists, a ref update states the old value it expects) or **undone** from its checkpoint.

#### Failure matrix

| # | What happens | Detected by | What Bridle does | What the user sees |
|---|---|---|---|---|
| 1 | Bridle is killed or crashes during a step | Next start: `owner.lock` can be taken, journal has no end | Marks the run `interrupted`; the last checkpoint and the write log are intact; files are whole (atomic writes); leases released | "Run r-12 was interrupted at step 2. Resume, undo step 2, or keep the changes." |
| 2 | Power loss or blue screen | Same, plus a torn last journal line | Torn line ignored; checkpoint refs verified with `git cat-file -e`, falling back to the previous one | Same message |
| 3 | A model server crashes mid-turn | The request fails; process gone | The step fails, leases released, the scheduler restarts the server (guarded) up to twice; then the run pauses | "The model stopped. Restarted it. Continue?" |
| 4 | A swap fails (out of memory, start fails) | Start error; swap record exists | Restore the previous model; if that also fails the run is `failed: no model` and nothing holds leases | A banner with a Start button |
| 5 | Bridle dies between unloading and reloading | Next start finds the swap record | Reload what the record names, after the VRAM guard | "Restored the model after an interrupted swap." |
| 6 | Two agents want one file | Lease table | Second claim refused with the holder named; the agent waits, works elsewhere or reports | A card in the Agents panel |
| 7 | The user edits a file the agent read | Hash check at write | Write refused: "changed since you read it"; the agent re-reads once, then reports | The editor already marks the tab |
| 8 | The user switches branch, commits or resets in their terminal during a run | HEAD compared with the run's base at each checkpoint | Pause the run | "HEAD moved from X to Y. Continue on the new base, or stop." |
| 9 | A second Bridle window or the CLI on the same repository | `repo.lock` held | Second one is read-only | "Another Bridle is running a team here." |
| 10 | Merge conflict at integration | `merge-tree` result | Technical manager resolves in the integration worktree; after the retry limit, the run is `blocked` and the user's tree is untouched | A conflict list and a diff |
| 11 | Clean merge but broken behaviour | Verification after each merge | The engineer fixes, up to the retry limit, then `blocked` | The failing check |
| 12 | `index.lock` is held | Git error | Retry with back-off for 10 s, then fail the step; the lock is never deleted automatically | "Git is busy (another Git process holds index.lock)." `bridle doctor` says how to check |
| 13 | A worktree was left behind after a crash | Manifest against `git worktree list --porcelain` | Final checkpoint commit of any uncommitted work on its branch, then `git worktree remove` and `prune`. Bridle never uses `git worktree lock`, so nothing stays locked | A line in the recovery report |
| 14 | A branch `bridle/...` is left behind | Manifest | Kept if unmerged and not discarded; deleted only when merged or discarded by the user | Listed under Runs |
| 15 | Disk full | Write error | Atomic write fails whole; a failed checkpoint stops the run (undo could not be promised) | "Disk full. Stopped before changing more." |
| 16 | A test or build runs forever or leaves child processes | Time-out; process still alive after Bridle ends | The whole process tree is killed on time-out; on Windows each command runs in a **job object that kills its children when Bridle exits**, so a crash leaves no orphan build running (a safe wrapper crate is needed because the project forbids `unsafe`) | "Command timed out and was stopped." |
| 17 | An agent's context overflows | Token count | Compaction (built); over budget the agent stops | A card |
| 18 | A handoff does not parse, or breaks the schema | Validation | Refused with the reason; the sender retries twice, then escalates to the lead, then to the user | A refusal in the log |
| 19 | Delegation loops or explodes | Caps: depth 3, 12 handoffs, 8 agents, cycle limit | Refused naming the rule | A refusal |
| 20 | A file is locked by an antivirus scan | Rename error | Three retries, then fail the write with the system's message | The message |
| 21 | Path trickery (case, trailing dots, `..`, symlink, reserved names) | Path resolution and the policy | Refused | The rule name |
| 22 | A secret is in a file | Secret-file names; redaction | Never read by agents; left out of checkpoints | Nothing |
| 23 | The repository is mid-merge or mid-rebase at start | Preflight | Refuse to start | The reason and the command to finish |
| 24 | Apply onto a branch that moved | `merge-tree` | Three-way merge; a conflict goes to the user | A conflict list |
| 25 | The user's hooks fail at Apply | The commit's exit code | The branch is kept; nothing lost | The hook's output |
| 26 | The user closes the window mid-run | Close event | Stop the agents at a step boundary, take a checkpoint, mark `interrupted`; stop the model servers started | The next start offers to resume |
| 27 | The user switches project mid-run | Switch request | Ask: stop and checkpoint, or cancel the switch | A dialog |
| 28 | Clock changes | Monotonic time and sequence numbers | Leases and time-outs unaffected | Nothing |
| 29 | A very large or binary file changes | Size rule | Listed, not stored; undo of that file is not promised | A note |
| 30 | Something not in this table | The run ends in an error or is stopped | The last checkpoint and the journal remain; `bridle doctor --repair` lists everything it found and asks before changing anything | A report |

#### Predicted workflows

| # | Workflow | Behaviour |
|---|---|---|
| W1 | One agent edits a file (today's chat) | Unchanged; the write log and checkpoints are added when a run is a team run. |
| W2 | A read-only team run (a technical summary, a pitch) | No leases, no checkpoints of source; only artifacts are written. |
| W3 | A sequential team run in the working copy | Checkpoint, lease, edit, verify, checkpoint, release, per step. Undo per step. |
| W4 | Parallel engineers in worktrees | As above; integration and Apply. |
| W5 | The user edits while a run works | The stale-read check and the user's lease protect both sides. |
| W6 | The user closes the window or Esc | Stop at a step boundary; checkpoint; resume later. |
| W7 | Crash or power loss | Recovery at start-up; nothing lost beyond the step in progress. |
| W8 | Undo a step or a whole run | From the write log; files that changed again are reported, not overwritten. |
| W9 | Apply or discard a run | Apply as squash, merge or patch; discard removes worktrees and branches after a final checkpoint. |
| W10 | Two windows or the CLI | One write-run per repository. |
| W11 | A folder with no Git | Shadow repository; no worktrees. |
| W12 | The repository is mid-rebase | Refuse. |
| W13 | An idea arrives during a run | Parked; nothing enters the conversation ([section 6](#6-feature-f3-the-ideas-queue)). |
| W14 | A long run across a model swap | The swap record and restore guarantee. |
| W15 | The CLI's `bridle task` and the window on one repository | Task state is separate; the repository lock covers write-runs. |
| W16 | A repository of the user's with unstaged work | Baseline recorded; those files are not leasable. |

#### A manual runbook (preview of the user guide)

If Bridle itself is broken, everything above can be inspected and undone with plain Git:

| To | Run |
|---|---|
| List what Bridle saved | `git for-each-ref refs/bridle/` |
| See what a checkpoint changed | `git diff refs/bridle/runs/r-12/step-002 refs/bridle/runs/r-12/step-003 --stat` |
| Compare a checkpoint with the working copy | `git diff refs/bridle/runs/r-12/step-003 -- path\to\file` |
| Restore one file from a checkpoint | `git restore --source=refs/bridle/runs/r-12/step-003 --worktree -- path\to\file` |
| List worktrees | `git worktree list --porcelain` |
| Remove a left-over worktree | `git worktree remove --force .worktrees\r-12-maya` then `git worktree prune -v` |
| Remove a worktree that Git shows as locked and missing | `git worktree unlock <path>`, `git worktree prune -v`, `git branch -D bridle/r-12/eng-maya` |
| List run branches | `git branch --list "bridle/*"` |
| Preview a merge without touching files | `git merge-tree --write-tree <branch-a> <branch-b>` |
| Take a stale index lock away | First check that no Git process runs (`tasklist /FI "IMAGENAME eq git.exe"`), then delete `.git\index.lock` |
| Drop a run's checkpoints | `bridle runs prune r-12` (planned). By hand: list them with `git for-each-ref refs/bridle/runs/r-12`, then `git update-ref -d <ref>` for each |

The user guide (`docs/guide/team-runs-and-git.md`) and a runbook (`docs/reference/run-recovery.md`) are deliverables of P1, written
with the code, not after it.

### 3.7 Tests that prove it

1. **Crash points.** A hidden setting makes Bridle abort at a named point (`after_intent`, `mid_checkpoint`, `after_lease`, `mid_swap`,
   `mid_merge`, `before_apply`, ...). A test aborts at each, starts again, and checks the invariants below.
2. **Invariants** after every recovery:
   - I1 The user's branch, index and files are unchanged unless Apply ran (or the run was sequential and the files are in the
     last checkpoint).
   - I2 Every completed write is in a checkpoint or in the file.
   - I3 No lease, lock record, worktree or branch is left that the manifest does not explain.
   - I4 At most one GPU model set is loaded, and the previous model is back.
   - I5 Replaying the journal twice gives the same state.
3. **Property tests** on the lease table: whatever the order of claims, releases, expiries and crashes, no path has two holders.
4. **Git scenario tests** in temporary repositories: a dirty tree, a detached HEAD, an unborn branch, a mid-rebase state, a
   submodule, a shallow clone, CRLF settings, a case clash, a 300-character path, an empty repository, a repository with a
   `.gitignore` that hides build output.
5. **Kill tests** in the end-to-end suite: terminate the real window at random times during a scripted run, ten times, and check the
   invariants on restart.
6. **Policy tests:** a model that runs `git commit`, `git switch`, `git stash`, a `git` command naming another branch, or writes under
   `.bridle/`, is blocked in a team run.

What this does not cover is stated in [section 12](#12-risks-and-what-is-not-known).

### 3.8 Memory

The word covers two things.

**What agents remember and share** (three tiers):

| Tier | What | Who writes it | Lifetime |
|---|---|---|---|
| **Working context** | An agent's own conversation | The agent loop | One agent; compacted when long (built) |
| **Run blackboard** | The plan, the **decision log** (what was decided, by whom, why), artifacts | A harness tool, `handoff` or `artifact_write`; every entry carries the writer, the time and a prompt hash | One run; kept in `.bridle/runs/<run>/` |
| **Project memory** | Conventions and facts about the project | The user, in `AGENTS.md` (read into the prompt today, 4,096 bytes by default, framed as guidance and not a permission) | The project |

- Agents **do not write project memory** in version 1. An agent can *propose* an entry; the user accepts it. This is because a stored
  note is a way for one bad run, or one injected instruction, to poison every later run. This also avoids Letta's cost, where every
  memory operation spends model tokens, which a small model cannot afford.
- No vector store in version 1. The corpus is one repository, and the code index already answers lookups.
- What an agent receives is its role prompt, the brief, the decision log, and artifact references, within its window. It does not
  receive another agent's transcript.

**What the machine can hold:** the table in [3.4](#34-small-models-on-the-gpu-the-fleet-and-its-manager). The VRAM guard is
extended to fleet configurations (a manager and a multi-slot server together), and a mode that would leave under 768 MiB free is
refused with the numbers. System memory is checked as well: starting a second server on the CPU is refused when free memory is
under 1 GB.

### 3.9 How agents pass work to each other

A **handoff** is one JSON object, validated on receipt. Anything that does not parse is refused and the sender is told why.

```json
{
  "id": "h-0007", "run": "r-12", "from": "eng.maya", "to": "qa", "kind": "task",
  "brief": "Review the change to the cache for bugs.",
  "inputs": ["artifacts/diff-step-2.patch"],
  "acceptance": ["lists each bug with file and line", "says 'no bugs found' only if checks pass"],
  "budget": {"max_calls": 6, "max_tokens": 6000},
  "reply_to": "h-0003"
}
```

- `kind` is `task`, `report`, `question`, `review` or `decision`. A `report` carries a bounded summary (2,000 bytes, as in ADR-0009)
  and references, never the other agent's transcript.
- Messages are appended to `.bridle/runs/<run>/mailbox.jsonl`; artifacts live in `.bridle/runs/<run>/artifacts/`.
- **The file tools cannot write `.bridle/`** (a protected root, by design). Handoffs and artifacts are written by two harness tools,
  `handoff` and `artifact_write`, through the same policy, redaction and size limits, and only inside the run folder.
- A brief must have the four parts Anthropic found necessary: objective, output format, tool and source guidance, boundaries.
- Every handoff and every refusal is an event with `run`, `from` and `to`.

### 3.10 Permissions

A member's effective rights are the **intersection** of the policy and its role's allowlist (ADR-0011, rule 4: no role gets a
verdict the policy would not give). Role and member definitions come from the machine configuration and built-in defaults. A
repository's `bridle.toml` may **restrict** them (fewer tools, a lower budget, a disabled member) but may not add tools, add a
writer, change a slot or define a member, because the repository arrives with the code it configures (the existing trust rule).
Prompt text from a repository is untrusted.

## 4. Feature F1: sub-agents

| Kind | Reads | Writes | Model | State |
|---|---|---|---|---|
| Reader | up to 4 named files and a text | nothing | LFM, now on the CPU (`micro`) | Built (ADR-0009), off by default |
| Worker | the repository through the tools | files, under a lease | Fleet worker, or `main` in Solo | New. ADR-0011 allows workers to edit through the policy |
| Reviewer | the diff and the brief, **not** the parent's transcript | nothing | Fleet or `main` | New. A clean context, as Cognition reports works |
| Consult | a forked summary of the task | nothing | `deep` by swap | The escalation swap is built (2026-10-28), unused by the chat |

**Lifecycle.** `spawn(member, brief, inputs, budget)` makes an agent with its own context, runs it through the same turn loop,
tools and policy as the main agent, and ends it when it reports, runs out of budget, or is stopped. The parent receives the bounded
report and artifact references. Depth at most 3; at most 8 agents and 12 handoffs per run; a total call budget.

**Fleet start-up.** The scheduler stops `main`, checks the VRAM guard for the fleet, starts the manager and the LFM server, waits for
both to be **healthy** (F0.1), then runs. It records the swap first. If a server does not become healthy, it stops both and restores
`main`.

**Failure.** A failed or overrunning agent returns an error report. The parent decides; the loop rules still apply to each agent; the
run's budget is the backstop. A run that cannot restore `main` ends with an explicit message, never a silent state.

## 5. Feature F2: teams, roles and members

### 5.1 The model

```text
Team ──has──> Role ──has──> Member
(Engineering)  (Senior Engineer, Engineer, Intern)  (maya, sam, noor ...)
```

- A **team** is a department with a prompt that says what the team does, a default tool allowlist and the handoffs it may make.
- A **role** is a position inside a team: its level (lead, senior, junior, intern), its tool allowlist, whether it may write, its
  budget, whether its output needs a **review before it goes on** (the default for interns), and its **model class**: `manager`
  (the small manager, or the 9B in Solo) or `worker` (an LFM slot, or the 9B in Solo).
- A **member** is a named individual: a handle (`@maya`), a display name, its role, its **speciality** (a prompt fragment such as
  "backend, Rust, performance"), and whether it is enabled. A member's prompt is composed: team prompt, role prompt, then the
  member's speciality. A member prompt is limited in length and is treated as untrusted when it comes from a repository.
- **The user sets the headcount.** In the Team panel: add or remove a member, "add 3" to a role, rename, edit the speciality,
  disable. The roster is kept in the machine configuration, so it follows the user to every project. A project may disable members
  or lower their limits in `bridle.toml`, and may not add any.
- **Headcount is not concurrency.** Twenty members can exist while only as many run at once as there are free slots (eight in the
  fleet, one in Solo). The rest wait in a queue the Agents panel shows.

### 5.2 The six teams, and a starting roster

The teams follow the literature (orchestrator, programmer, reviewer, tester, retriever; MetaGPT's five; Scrum's three) and the
owner's list. The names and counts below are a **default the user edits**.

| Team | Handle | Roles (starting members) | Owns | Produces | May write files | Model class |
|---|---|---|---|---|---|---|
| Project Management | `@pm` | Project Manager (1) | The plan, the order of work, status | `plan.json` (stages with acceptance), status reports | no | manager |
| Product | `@product` | Product Owner (1) | Requirements, priorities, the pitch | `requirements.md`, user stories, `pitch.md` | artifacts only | manager |
| Technical Management | `@tech` | Technical Manager (1) | Architecture, interfaces, who does what, review, integration | `design.md`, the file split, review notes | artifacts only; integrates | manager |
| Engineering | `@eng` | Senior Engineer (1), Engineer (2), Intern (1) | One stage of implementation | Edits, test results, diff summaries | **yes, under a lease** (the intern only with a review) | worker; senior may be manager |
| Research | `@research` | Researcher (1) | Facts: code, docs, later the web | `findings.md` with citations | no | worker |
| Quality | `@qa` | QA Lead (1), QA Tester (1) | Whether the work meets the acceptance | `review.md`, verification results. **Informs only** (ADR-0011) | no | worker or manager |

**Who may hand work to whom** (allowed edges between teams; anything else is refused):

```text
user -> any team, role or member (by @)
pm   -> tech, product, research, qa
product -> research, tech
tech -> eng, research, qa
eng  -> tech, qa            (an intern's work goes to a senior engineer or QA first)
qa   -> eng, tech
```

The user directs; `pm` coordinates; `tech` manages the engineers; `product` supplies requirements; `qa` is independent.

### 5.3 How the user talks to it

- **`@` opens a tree** (teams, roles, members) in the same menu as `/`. A tag can name:
  - a **team**, `@eng`: Bridle picks the member;
  - a **role**, `@eng.intern`: Bridle picks among that role's members;
  - a **member**, `@maya`: exactly that member does it.
  Several tags give the same request to each: `@maya @sam review the cache change`.
- **An explicit tag always wins.** The member works on it, may hand work only along the allowed edges, and the member's cards show
  in the chat. The user can also answer a member's question directly: each member has its own thread, so the user can talk to one
  of them as well as to the team.
- **Who is picked when the user tagged a team or a role, or nobody.** The assigner is deterministic first:
  1. a member the request names;
  2. speciality match (keywords and tags in the members' specialities against the request);
  3. availability and load: the member with the fewest queued tasks;
  4. a tie goes round-robin.
  Only if the rules cannot decide does the small model answer, with **one choice from the list of member ids**, with a rules fallback
  (ADR-0010). The choice and its reason are shown and logged.
- **Untagged text.** With the team off, nothing changes. With the team on, the router picks a team from a closed list (rules first,
  Laya for the undecided, `pm` as the fallback) and the assigner picks the member.
- **Coordination between members.** They hand work to each other through the allowed edges (the tech manager asks the researcher; an
  engineer asks QA). The user can see every handoff and **step in at any point** by tagging a member, which pauses nothing else.
- The window shows who is working, each member's state (idle, working, waiting for a lease, queued), the current budget used, and the
  handoff graph of the run. Esc stops the whole run.

### 5.4 Two walkthroughs

**A. "`@tech` summarise the cache design and send it to `@product` for a pitch."** The technical manager (picked by the assigner)
reads the code with the read tools, writes `tech-summary.md` with `artifact_write`, and sends a `task` handoff to the Product Owner,
naming that file and the acceptance. The Product Owner writes `pitch.md` and reports. No swap, no leases, no checkpoints of source.

**B. "`@pm` implement this feature in 3 stages."**
1. `pm` writes `plan.json`: three stages, each with an objective, an acceptance and dependencies (the existing five-step limit).
2. The technical manager writes `design.md` with the **interfaces** and assigns **disjoint file sets** to engineers by speciality.
3. Per stage: checkpoint; the assigned engineer takes the leases, edits through the policy, runs the checks under the `verify` lease,
   and reports a diff summary; an intern's work goes to a senior engineer first; QA reviews with a clean context; a failure goes back
   up to `max_implementation_retries`, then to `tech`, then to the user; checkpoint; leases released.
4. `pm` reports after each stage and at the end. Apply is the user's decision.

### 5.5 Limits that keep a run bounded

Depth 3; 12 handoffs and 8 agents per run; a handoff cycle at most twice; a call and token budget per role and per run; Esc stops
everything. Every limit is a refusal that names the rule.

### 5.6 The crew rail

A strip on the **right edge of the window, centred vertically**, in the spirit of the contributors list on a GitHub page, but live. It shows
**one avatar per member** of the roster.

- **Avatar:** a circle with the member's initials and a colour taken from the member's handle; a ring in the **team's colour** groups
  the six teams; a leader has a small marker.
- **State dot:** grey idle, green working (pulses), amber waiting (queued, or waiting for a lease or the model), red blocked or failed.
- **Badge:** the number of open issues assigned to the member.
- **Hover:** a card with the name, role, team, speciality, the issue being worked on, the model slot and the budget used.
- **Click:** **tags** the member, by inserting `@handle ` into the chat input. **Right-click** (or a click on the card) opens the member's
  menu: open their thread, see their issues on the board, pause, or disable.
- **Header:** "Crew", the number working now, and a button that opens the roster editor (add, remove, duplicate, rename, speciality,
  disable); the headcount per role is set there.
- The rail can be collapsed. With more than 12 members it scrolls. It is data-driven, so a changed roster appears at once.

### 5.7 The board

**What it is.** A project board of the team's own, not the user's GitHub: issues, tracks (one lane per team), owners, dependencies, a
ready queue, comments and Git links. It answers "a project manager takes an issue and creates three: one for engineering, one for QA,
one for product".

**What other tools show.** Beads: hash ids, hierarchy, dependencies, a `ready` list, an atomic claim, an audit trail. Backlog.md: one
file per task, acceptance criteria, a Definition of Done, a Kanban board, an MCP server. GitHub Projects: fields and board and table views.
The design takes: hash ids and hierarchy (Beads), acceptance and Definition of Done (Backlog.md), a board with lanes and filters
(GitHub), and keeps plain files in `.bridle/board/` instead of a database.

**Issue.**

| Field | Meaning |
|---|---|
| `id` | A short hash, `B-a3f8`, made from the creation time and a counter, so no central counter is needed. A child has `B-a3f8.1`. |
| `title`, `body` | What and why. Text from agents is untrusted when another agent reads it, and is framed as such. |
| `kind` | `task`, `bug`, `feature`, `research`, `review`. |
| `status` | `backlog`, `ready`, `in_progress`, `in_review`, `blocked`, `done`, `cancelled`. |
| `assignee` | A member, a role, a team, or nobody. A team or role is resolved by the assigner ([5.3](#53-how-the-user-talks-to-it)). |
| `parent`, `blocked_by` | The hierarchy and the dependencies. A cycle is refused. |
| `acceptance`, `dod` | Checks that say when it is done. The project's default Definition of Done is added to each issue. |
| `priority` | `p0` to `p3`. |
| `labels`, `track` | Free labels; the track is the team lane. |
| `budget` | The most model calls, tokens and minutes the issue may use. |
| `links` | The run, the branch, checkpoints and artifacts. |
| `comments` | A thread. This is also how agents talk to each other: a handoff is an issue or a comment, so there is one record. |

**States.** `backlog -> ready -> in_progress -> in_review -> done`; `blocked` is automatic while a `blocked_by` issue is open; `cancelled`
from any state; `done -> ready` reopens it. A parent is done when all its children are done and its own checks pass.

**The project manager's flow.** `@pm` takes issue `B-a3f8` ("add a cache to the loader"). Its turn calls `board_create` three times:
`B-a3f8.1` for Engineering (the implementation), `B-a3f8.2` for Quality, **blocked by** `.1` (review and run the checks), and
`B-a3f8.3` for Product (write the user-facing note), blocked by `.1`. Each has acceptance. The board runner then works the **ready**
queue: `.1` is ready, so the assigner picks an engineer (by speciality and load); when `.1` is done, `.2` and `.3` become ready.
When all three are done, `B-a3f8` returns to the project manager, who closes it with a summary.

**Tools for members** (harness tools, written to the board store only, checked by the role's allowlist and the limits):
`board_ready`, `board_list`, `board_show`, `board_create`, `board_claim`, `board_comment`, `board_update`.

| Who | May create | May change status |
|---|---|---|
| User | anything | anything |
| Project Manager | issues for any team, children of any issue | parents, `cancelled`, reopen |
| Product Owner | requirement and research issues | their own |
| Technical Manager | engineering and review issues | their own; `in_review` to `done` of engineering issues |
| Engineer, Intern | sub-tasks of their own issue | their own, up to `in_review` (an intern's work needs a review) |
| QA | bugs (assigned to engineering) | review issues; `in_review` to `done` or back to `ready` with a comment |
| Researcher | findings issues | their own |

**What else is needed for this to work** (from the survey and from the failure matrix):

| Need | Why | Design |
|---|---|---|
| An **atomic claim** | Two members must not take one issue | `board_claim` is a compare-and-set under the store's lock; one issue per member at a time (a work-in-progress limit of 1). |
| **Stale-claim recovery** | A crash must not strand an issue | A claim records its run; at start-up an issue whose run was interrupted returns to `ready` with a comment. |
| **Idempotent creation** | A retried tool call must not duplicate issues | `board_create` takes an idempotency key (parent plus title) and returns the existing issue. |
| **Dependency graph and a ready queue** | Order of work | Cycle check on write; `ready` = no open blocker (Beads). |
| **Acceptance and a Definition of Done** | "Done" must be checkable | Criteria on each issue; a `run_tests` pass is recorded as evidence. A QA issue cannot close without it. |
| **Budgets and escalation** | A member must not loop | Per-issue call, token and time budgets; over budget or two failures sets `blocked` with the reason and tags the lead; after that the user. |
| **One audit trail** | Debugging and trust | The store is an append-only event log; every change names the actor, the time and the run. |
| **Limits** | Flooding by a confused model | At most 30 open issues per run, depth 3, a rate limit on creation. |
| **Untrusted text** | Prompt injection through issue text | Issue bodies and comments are framed as data when read by agents. |
| **User control** | The user is the owner | The user can assign, reassign, comment, reorder, close or cancel any issue; dragging a card is a status change by the user. |
| **Git links** | Work must be traceable | Issue to run to branch and checkpoints; Apply adds a `Bridle-Issue:` trailer. |
| **Concurrency** | Several members and two windows | One writer inside the app; a file lock for other processes; torn last event ignored on replay. |
| **Export** | Share and review | `bridle board export` writes Markdown (Backlog.md style). A sync to GitHub Issues is later and would go through the network policy. |

**Storage.** `.bridle/board/events.jsonl`: one JSON event per line (`created`, `updated`, `status`, `claim`, `comment`, `link`), each
with a checksum; the board is the replay of the log. A torn last line is ignored. `.bridle/` is machine-local and ignored by Git.

**The window.** A **Board** view opens as a tab in the editor area: columns for the statuses, a lane per team, cards with the id, title,
assignee avatar, priority, a lock icon when blocked, and the dependency count; a detail panel with the fields, the comment thread and
actions; a form to create an issue; filters by member and by team. The crew rail's badges and the board agree because both read the
same log.

## 6. Feature F3: the ideas queue

The owner confirmed the design. It stays as written, and it ships with checks that show it works.

**Today:** while a turn runs, Send is disabled and the typed text stays in the box (`chat.js`: `state.busy` returns early).

**Behaviour.**

- A **lightbulb button in the top right** opens the ideas panel, with a count badge. While a run is working, the input offers **Save as
  idea** beside Send. `/idea text` and a shortcut do the same at any time.
- A parked idea is **not in the conversation and not visible to the model.**
- **Delivery:** when the run ends successfully (the top-level run, not each step of a team run) the **oldest idea is sent as a normal
  message**; the next waits for that run to end. One idea per run, so each gets its own answer. A run that ends with an error or a
  stop **does not deliver**: the panel says "N ideas waiting" with Send and Hold.
- Each idea can be **edited, reordered, deleted, or sent now**. A **Hold** switch stops delivery. Ideas are never sent when the
  window starts; only when a run that this window watched ends.
- **Storage:** `.bridle/ideas.json` per project, written atomically under a file lock, so ideas survive a restart and are not shared
  across projects. It is under `.bridle/`, already in `.gitignore`. Limits: 50 ideas, 8,000 characters each. An idea is sent exactly as
  it was written, like any message you type. No images in version 1.

**Checks that must pass** (end-to-end suite):
1. An idea saved during a run is listed, with the badge, and does not appear in the model's request (the request body is inspected).
2. It is delivered once, after the run ends successfully, as a user message, in order, one per run.
3. A stopped run or an error holds delivery; Hold holds it; Send now works.
4. Edit, reorder and delete change what is sent.
5. A restart keeps the ideas; opening the window does not send them.
6. Another project's ideas are not shown.
7. Two windows on one project do not lose or double-send an idea (the file lock).
8. The limits (count, length) are enforced with a message.

## 7. Feature F4: web search for literature review (plan only)

Opt-in and policy-gated: `security.network` is `ask`, so denied by default; the tools appear only when enabled, use the configured
proxy, are redacted, bounded and cited, and research members are the only default users.

1. **Survey** (written before code): key-free APIs (OpenAlex, arXiv, Crossref, Semantic Scholar) against a self-hosted meta-search
   (SearXNG) against keyed services; and harnesses that ship a research loop (GPT Researcher, Stanford STORM, LangChain's open deep
   research, FutureHouse's paper-qa). Each is judged on: works through a proxy, no secret on the model's side, bounded output,
   citations, licence, and whether it can be called as a tool rather than as a second agent loop. The candidates are from memory and
   must be checked in the survey.
2. **Integration shape:** `web_search(query, source)` and `fetch_source(url)`, through the network policy, with a domain allowlist in
   the machine config, results truncated and framed as untrusted content, and each result written to an artifact with its URL so the
   researcher can cite it.
3. **Measure** on a few real literature-review requests and record it in `progress.md`.

## 8. Feature F5: window polish

Codespaces-inspired: resizable and collapsible panels, a command palette (Ctrl+Shift+P), tab and breadcrumb refinements, an outline
view, a settings page and notifications, plus the panels this plan adds: **Agents** (who is working, state, budgets), **Team** (the
tree of teams, roles and members, with add, remove and edit), **Runs** (checkpoints, undo, apply, discard, recovery report) and
**Ideas**. The new panels come with their features; the general polish comes last.

## 9. Phases, exit criteria and decision records

| Phase | Work | Exit criteria (measured, on the tested machine) | ADR |
|---|---|---|---|
| **P0 Foundations and measurements** | F0.1 readiness; F0.3 identity; **measure**: `micro`-class model speed on CPU and on GPU; aggregate tokens per second at 1, 2, 4 and 8 slots; Qwen3.5-4B and LFM3-1.7B on b11756 and on a newer build; fleet VRAM against the estimate; prompt re-read time after a `main` reload at 16K, 32K and 65K; the existing tests with the **fleet configurations against the 9B** | A message sent during start shows "Loading" and never an HTTP 503; the numbers are in `progress.md`; a manager is chosen from data | none |
| **P1 Git layer, leases, recovery** | F0.2 scheduler with the swap record; F0.4 checkpoints, repository lock, preflight, write log, undo, apply (sequential mode); F0.5 leases and the stale-write check; F0.6 `doctor --repair`; F0.7 handoff and artifact tools; the **user guide and the runbook** | Every crash point in [3.7](#37-tests-that-prove-it) recovers with invariants I1 to I5; ten random kills of the real window recover; a model that runs `git commit` during a run is blocked; the runbook commands were each run by hand | 0019 the Git layer and leases |
| **P2 Fleet sub-agents** | F1: fleet and Swap modes, the manager, workers, reviewers, consults; budgets; the Agents panel | A run swaps `main` for the fleet and back with `nvidia-smi` showing one set before and after; the fleet does the bounded tasks of [3.4](#34-small-models-on-the-gpu-the-fleet-and-its-manager) at no worse pass rate than the baseline; a forced fleet failure restores `main` | 0020 sub-agents and the fleet |
| **P3 Teams, crew rail and board** | F2: teams, roles and members as data, the roster editor, the `@` tree, the assigner, the six teams; F2.1 the crew rail; F2.2 the board, its tools and the board runner (Solo first); walkthroughs A and B; a **role evaluation set** in `evals/` | A and B pass end to end in the real window; tagging a team, a role and a member each reaches the right member; the headcount can be changed in the window and survives restart; no run exceeds its budgets | 0021 teams |
| **P4 Ideas** | F3 | The eight checks in [section 6](#6-feature-f3-the-ideas-queue) pass | 0022 ideas queue |
| **P5 Parallel writers** | Worktree mode, integration, Apply with `merge-tree` | Two engineers edit different files at once and merge cleanly; a forced conflict goes to the technical manager and then the user with the tree untouched | 0023 parallel writers |
| **P6 Search** | F4 survey, then integration | The survey is written; one literature-review request is answered with cited sources through the proxy, and the tool is denied when the setting is off | 0024 web search |
| **P7 Polish** | F5 | The end-to-end suite covers the new panels | none |

F3 (ideas) does not depend on anything and is small. It can ship before P1 if wanted; it is placed after the team only to keep the
stated order.

## 10. Where the data lives

```text
.bridle/                       (in .gitignore; machine-local)
  repo.lock                    one write-run per repository (operating-system file lock)
  leases.json, leases.lock     the lease table
  swap.json                    the model swap record
  shadow.git/                  checkpoints for a folder without Git
  ideas.json                   the ideas queue
  board/events.jsonl           the board (issues, claims, comments), one event per line
  runs/<run>/
    owner.lock  journal.jsonl  manifest.json  mailbox.jsonl  writes.jsonl
    artifacts/                 plan.json, design.md, review.md, pitch.md ...
.git/refs/bridle/...           checkpoints (refs, not branches)
.worktrees/<run>-<member>/     parallel-writer worktrees (existing folder)
bridle/<run>/<member>          their branches
```

The machine configuration (`%APPDATA%\bridle\config.toml`) holds the team roster and the fleet settings.

## 11. Decisions taken by default

The owner asked not to be asked, so each former question was settled with the default below. Each one can be changed later by editing a
setting; none is hard to undo.

1. **Apply:** a squash into one commit that the user edits. Bridle never commits on the user's branch by itself.
2. **Caps:** 8 agents per run, up to 8 slots per server; more members can exist and wait in the queue.
3. **Interns:** their output is always reviewed (by a senior engineer or QA) before it goes on.
4. **Handles:** team `@eng`, role `@eng.intern`, member `@name`. Default members get short names from a list (maya, sam, noor, ...) and
   can be renamed.
5. **Default mode:** Solo. The fleet is used when the board has two or more ready issues of fleet class, or when the user forces it.
6. **Ideas:** one idea per finished run, delivered at the end of the whole run, held when a run stops or fails.
7. **Licence wording:** the models guide is corrected (done in the same change as this revision).
8. **Other features:** none were named beyond the ideas queue and the board; this is revisited when one is.
9. **Models and runtime:** the manager is **Qwen3.5-4B**, which the owner approved. Files for the measurements (Qwen3.5-4B 2.74 GB,
   Granite-4.0-H-Micro 1.94 GB, Granite-4.0-H-1B 0.90 GB, and a newer llama.cpp for LFM3) are fetched only when the measurement
   phase needs them, with their SHA-256 recorded; Bridle itself never downloads anything.

## 12. Risks and what is not known

- **A small model in many seats.** Whether a Qwen3.5-4B manager makes a useful 3-stage plan, and whether LFM workers finish bounded
  edits, is **unmeasured**. The LFM card itself says it is not for programming. The fleet is therefore limited to bounded work with
  an escalation to the 9B, and P0 compares it with the 9B before anything is built on it.
- **Vendor numbers.** LFM3's figures are Liquid's own; the Qwen3.5-9B BFCL figure differs between sources (62.83 and 66.1). None is
  used to decide.
- **The estimates are estimates.** The fleet memory table comes from file headers and an overhead constant calibrated on one model;
  two server processes may cost more than the estimate. P0 measures it, and the guard refuses what does not fit.
- **Throughput.** Eight slots are not known to give eight times the work. The measurement decides the slot count.
- **Git cannot see everything.** It does not detect a semantic conflict, a change made by a command outside the write log is only in
  the checkpoint diff, and a large binary file is not stored. The plan lists each and says what is promised.
- **The list of failures is finite.** Row 30 is the rule for the rest: a safe, inspectable end state and nothing deleted without a
  copy. New failures found in use are added to the matrix and the tests.
- **Time.** A 3-stage run on a 40 tokens-per-second model is minutes to tens of minutes in Solo. The window must show progress and
  budgets, and every run must be stoppable.
- **Prompt injection through members and web pages.** Repository prompts and fetched pages are untrusted. Nothing here is a sandbox
  (security.md).
- **The multi-agent figures of other tools** (4 and 15 times the tokens; a 90.2 % gain on a research evaluation) come from Anthropic's
  research workload and are not evidence for local coding on small models.
- **Windows specifics.** NTFS case-insensitivity, antivirus locks, path length, job objects and OneDrive are handled in the tables
  above and each has a test; any other found in use is added.
- **CI.** The workflow also runs on `ubuntu-latest`, which has no WebKit packages, so `bridle-desktop` may fail to build there. That is
  outside this plan and is raised separately.

## 13. Build status

Updated 2026-11-04. Building started after the owner asked not to wait for confirmation. Everything below was run on the tested
machine; the numbers are in [progress.md](../status/progress.md).

| Item | State |
|---|---|
| F0.1 slot readiness (`warming`) | **Built.** A server that is alive but answers 503 is `warming`; the chat refuses to send to it. Two unit tests. |
| F3 ideas queue | **Built, and shown as the first column of the board.** Saving while a run works, one idea per finished run, hold, edit, reorder, delete, send now, make an issue. 11 unit tests (8 writers at once) and 2 end-to-end checks that pass with the model. |
| F2 teams, roles and members (data) | **Built.** Six teams, nine roles, ten starting members, handles for a member, a role and a team, a rule-based pick, per-member prompts, a saved roster (`team.json`), headcount editing. 10 unit tests. |
| F2.1 crew rail | **Built.** Avatars on the right edge with working state and open-issue badges; click to tag, right-click menu, team editor. End-to-end checked. |
| F2.2 board | **Built (first slice).** Append-only log with checksums, states, children, dependencies, ready queue, one claim per member, idempotent creation, recovery after a crash, Markdown export, role-limited board tools, a popup with columns, detail panel, filters and a runner. 14 + 4 unit tests, one policy test, and 3 end-to-end checks. |
| F1 sub-agents, small-model fleet, F0.2 scheduler | **Built in part (2026-11-04)** ([ADR-0022](../decisions/0022-the-fleet-is-small-models-loaded-together-and-a-deterministic-scheduler-swaps-them.md), [guide](../guide/fleet.md)). `scheduler.rs` (plans, swap budget, fit check, write-ahead swap record), `bridle fleet status/up/down/recover/run`, one worker server with several slots, config `[fleet]` (V51). Run on the real models (Granite-4.0-H-Micro manager, Granite-4.0-H-1B worker, 4 slots): eight tasks 3.7 times faster than one at a time; main stopped and restored; a swap record, a refused second `up` and `recover` shown. **Not built:** a manager that plans, use inside the board runner. (Worktrees and integration for parallel engineers now exist as lanes, ADR-0023, but the board runner does not give members lanes yet.) **Not shown:** answer quality (not graded; one answer cited a line that does not exist). |
| Window screenshots (added by the owner, not in the original plan) | **Built (2026-11-04)** ([ADR-0021](../decisions/0021-window-screenshots-start-the-program-and-capture-only-its-window.md), [guide](../guide/window-screenshots.md)). `window_screenshot` and `bridle desktop screenshot`; one program tried. |
| F0.5 leases and the stale-write check | **Built (2026-11-04).** `.bridle/leases.json` under a lock, claimed by the file tools for the active run, case-insensitive path keys, time to live, release at the end of a run; `read_file` shows a hash and `write_file`/`edit_file` refuse a file that changed since it was read (or when `expected_sha256` differs). 11 tests. Not built: the user's unsaved editor buffers as leases, the `verify` resource lease. |
| F0.4 shadow repository for a folder without Git, `doctor --repair` | **Built (2026-11-04).** `.bridle/shadow.git` with `GIT_DIR`/`GIT_WORK_TREE`, a 5 MB file limit and a 200 MB stop, taken over by a real repository made later; `bridle doctor --repair` for leases, runs cut short, index leftovers, unfinished downloads, temporary files, browser session folders. 5 tests. |
| F0.4 Git layer (checkpoints, undo, lock, preflight, journal), F0.6 recovery | **Built (second slice)** for sequential runs: 8 tests on real repositories and the runbook [run-recovery.md](../reference/run-recovery.md). Not built: worktrees for parallel engineers and the integration step. |
| Notices, background commands, lanes (from Infinity; added by the owner) | **Built (2026-11-04)** ([ADR-0023](../decisions/0023-notices-background-commands-and-lanes-from-infinity.md), [lessons](lessons-from-infinity-and-harness2.md)). Unforgeable notices after tool results, `command_start/status/stop`, and `bridle lanes` (worktrees for parallel members, three-way merge by the person). Tested, and run through the built program (20 of 20 live checks). **Not built:** lanes inside the board runner, a button for integration, board and lease notices. **Not measured:** how a small model uses any of it. |
| F0.7 handoff envelope and artifact store | **Replaced for now** by issues and comments on the board (a handoff is an issue or a comment). The envelope is not built. |
| F4 web search | **Built in part** ([web research](../guide/web-research.md), [the browser](../guide/web-browser.md)): search, pages, downloads, a managed browser, PDF text, archive listing. Not built: crawling, academic and news providers, an evaluation suite. |
| F5 further polish, F6 slot save and restore | **Not built.** |

**One real measurement.** The same request, `@priya` (project manager) asked to split "add `sub(a, b)` to `calc.py`" into one issue for
engineering and one for QA that waits for it, was run twice on Qwen3.5-9B. In one run she created both child issues, the runner gave them
to their owners in order, and the parent closed. In the other she created one of the two issues and her turn ended with the issue blocked,
so the runner stopped. The team flow works, and a 9B model playing the project manager is **not reliable** yet: it needs better
prompts or a small plan schema, and an evaluation set, before it is trusted (risk in [section 12](#12-risks-and-what-is-not-known)).

**Deviations from this plan.** The board popup replaced the separate ideas panel (requested by the owner). The roster is `team.json`
next to the machine configuration, not a section of the machine config. A tagged request becomes an issue for each tag instead of being
sent to the model directly.
