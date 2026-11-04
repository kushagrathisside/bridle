# Lessons from Infinity and Harness²

Written 2026-11-04 at the owner's request ("can we learn something from this to optimise our system"). Two projects were read; one set of
ideas was built ([ADR-0023](../decisions/0023-notices-background-commands-and-lanes-from-infinity.md)) and one was written down as a
proposal that is **not built**.

## What was read, and how far to trust it

| Source | Read | Not read |
|---|---|---|
| [Introducing Infinity](https://aws.amazon.com/blogs/opensource/introducing-infinity-a-runtime-for-extreme-agent-concurrency/) (AWS Open Source Blog, Shadaj Laddad, 23 October 2026) | The whole post | The video the owner first mentioned: it was not watched; the post is assumed to cover the same project |
| [hydro-project/infinity](https://github.com/hydro-project/infinity) | The README | The documentation it links, and the source |
| [google-research/harness2](https://github.com/google-research/harness2) | The README and the documentation folder's file names | The source, `SETUP.md`, `EXPERIMENTS.md`, `BENCHMARKS.md` |
| [Harness²: Recursive Agent Harnessing for an Open World](https://arxiv.org/abs/2610.14224) (Xu et al., Google) | The abstract and the HTML text through the limitations and future-work appendices | The experiments' raw tables beyond what the text states |

Both are research releases by their authors ("early research release"; "not an officially supported Google product"). **Neither result
has been replicated independently** as far as could be found, and every number below is the authors' own.

## Infinity

**What it is.** A Rust runtime for many agents at once. Each agent is treated like a stackless coroutine: a tool call ends an execution
*slice*; the runtime loads the thread's history from a store, runs one model completion, saves the output, dispatches at most one tool
call, and forgets the agent until the next event. Between turns an agent is only its conversation history. Messages for one thread are
handled in order, different threads in parallel. State is kept by thread ID, a tool-call turn is saved **before** the call is sent, and
every input has an ID so a redelivered message is skipped. Asynchronous events are shown to the model as an injected tool call
(`receive_event__injected`) that the runtime refuses if the model tries to make it. Children are started with `spawn_thread`. The README
adds a tool protocol (RAP) that serves tools over the network without holding a connection, with a compatibility layer for MCP servers,
and a coding agent, Infinity Code, which runs builds and tests in the background and edits in parallel in stacked sandboxes, showing each
result as a diff.

**The numbers, with their conditions.** About 100 KB per idle agent and about 75,000 agents in 8 GB; the benchmark launched 80,000
agents (3.2 million completions) on one thread of an AMD EPYC with a **simulated model** and **in-memory stores**. No throughput, latency
or cost is given, and the authors say the memory figures leave out real model and storage overhead.

**What the authors say they have not solved.** Scheduling is arrival order (priority is future work); how models reason about interleaved
events (they are mostly trained on synchronous tool traces); compaction in a tree of agents. The post describes **no security model and no
sandbox** for tool execution.

### What maps to Bridle

| Infinity | Bridle | Verdict |
|---|---|---|
| 75,000 idle agents in 8 GB | One 9B model in 8 GB of video memory; the limit is the GPU, not an agent's memory | Does not transfer |
| Save the turn before dispatching the tool call | The task journal records `CallStarted` before the call runs; the board's creation is idempotent; the fleet's swap record is written before anything is stopped | Already so |
| Stable input IDs, redelivery skipped | Board events are idempotent; run recovery is by journal | Already so, for the board and the runs |
| Per-thread ordered queue, threads in parallel | One write-run per project (`repo.lock`), leases per path | Matches; lanes extend it to parallel members |
| Injected event the model cannot forge | Web pages are framed as untrusted with a marker; file changes were reported only when a write failed | **Built: notices** |
| Slow tools run in the background | `run_command` blocks until the command ends or times out | **Built: background commands** |
| Parallel edits in stacked sandboxes, results as diffs | The plan's unbuilt worktrees and integration | **Built: lanes** (the board runner does not use them yet) |
| Tools over the network with no connection held (RAP) | Local MCP over stdio | Not needed here |
| Lambda, SQS, Aurora | Local-first by design | Not wanted |
| No sandbox for tools | Policy, sandbox, redaction and a machine-config-only trust boundary | Bridle is ahead; nothing to take |

## Harness²

**What it is.** A framework that improves an agent's *harness* (the system around a fixed model) without ground-truth verifiers. The
harness is eight files: system prompt, guardrails, memory, skills, tool guidance, scripts, subagents and a middleware plugin. **Task level:**
run the current harness once as a reference; *propose* two candidates (a general one aimed at planning, tool use, context management and
delegation, and a domain one); *probe* each on the same task and compare its trajectory and deliverable with the reference (a "behavioral
contrast"); *compose* a new harness from the edits the evidence supports, or keep the old one. With more than one step an agentic selector
compares anonymised deliverables. **Harnessing level:** keep a library whose entries are a harness plus an `EXPERIENCE.md` (the situation,
the lessons, the supporting evidence), added only when the contrasts support them.

**Results, as reported.** Gains of up to 11.5 points on WorkBuddy-Bench, 4.5 on JobBench and 11.8 percentage points on LAB (partial pass
rate), for GLM-5 and Gemini 3.5 Flash; more recursion helped (for example 76.5 to 80.8 on one subset, k = 1 to 5). Means over three
shuffled runs. The paper's own baselines are mixed: ReasoningBank fell up to 6.6 points *below* the base harness on coding, Harness
Scaling fell below it in 7 of 12 pairs, and Meta-Harness (an offline search) led on one LAB setting.

**Conditions.** The editor was a frontier model (Claude Opus 4.8); the executors were Gemini 3.5 Flash and GLM-5. **No small or open
local model was reported**, as far as the text read shows. Each step costs extra executions and editor sessions (the paper gives no
token counts). The authors name execution variability and "shared model biases" as limits: the signal does not prove correctness or
transfer. The repository's safety section is short: the executor's network is unrestricted and Codex runs with its sandbox disabled,
so benchmarks need network restrictions set by the user. The editing rules are tighter than that: only listed component paths are
editable; candidate harnesses and their scripts are read as text, never run; a harness that fails validation is discarded before any
rollout.

### What maps to Bridle, and what it would take (proposal, not built)

The plan's own risk note says the 9B project manager is unreliable (one of two runs created both child issues) and "needs better
prompts or a small plan schema, and an evaluation set". That is the Harness² problem on a small scale. Bridle has what the paper does
not: **real verifiers**. `evals/tasks/*` have a `verify.py`, `bridle bench` has conditions and result folders, and `evals/security` holds
72 adversarial cases. So the loop can accept an edit on a measured pass rate, not on a model's judgment.

A Bridle version would be **offline, run by the person, and never live**:

1. **Baseline first (first measurement done, 2026-11-04).** `bridle eval-agent plan --runs 10` gave the project manager (Qwen3.5-9B,
   Priya) the request "add `sub(a, b)` to `calc.py`, split into one engineering issue and one QA issue that waits for it" ten times in
   fresh repositories. Result: two or more child issues **10 of 10**; an engineering issue and a QA issue **8 of 10**; the QA issue
   waiting for the engineering issue **7 of 10**; **all three 7 of 10**. The failures were an issue assigned to neither role (twice, both
   ending at the step limit) and one plan with two engineering issues in which the QA issue waited for neither. With ten runs the interval
   around 7 of 10 is wide (very roughly 35 to 93 percent); it is a baseline to beat, not a rate to quote. The next step is more runs and
   a second request shape. The remaining text of this step is the plan for a larger set: add verifier-backed planning tasks (the plan is split into the right issues; dependent issues wait) to
   `evals/tasks` and measure the current prompts over many runs. Two runs are not a measurement. This is useful on its own.
2. **Editable surface: text only.** Role and member prompts, the planner prompt (`team::planner_system`), tool guidance. **Never**:
   the tool policy, the machine configuration, hooks, allow-lists, `security.*`. Candidates are read as text and never executed, as in
   Harness².
3. **Propose, probe, compose** with the local 9B as executor. The editor would be the same 9B (untested whether it can edit its own
   prompts usefully) or an external model, which is a network call that needs the owner's permission.
4. **Accept** only if the verifier pass rate rises over several runs **and** the security suite does not regress. A person reads the
   diff and commits it. Nothing is applied at run time.
5. **Experience entries** (situation, lesson, evidence) are worth copying only if they are kept next to the evidence; not designed yet.

**Cost and risk.** Every probe is a full run at local speed on one GPU, one after another. The paper's gains were found with strong
models; whether a 9B model gains anything is unknown. A prompt tuned on nine tasks can overfit them. Treat the first result as a
hypothesis.

## Decisions

| Item | Decision |
|---|---|
| Notices, background commands, lanes | Built ([ADR-0023](../decisions/0023-notices-background-commands-and-lanes-from-infinity.md)) |
| Board and lease notices; lanes for the board runner; a window button for integration | Next, small |
| Planner evaluation and an offline prompt-tuning loop | Proposal above; step 1 is the only part worth doing before anything else |
| Infinity's runtime, RAP, serverless deployment | Not taken |
| Live self-modifying harness | Not taken: policy and configuration must not be reachable by the thing they restrain |

## Sources

- [Introducing Infinity: a runtime for extreme agent concurrency](https://aws.amazon.com/blogs/opensource/introducing-infinity-a-runtime-for-extreme-agent-concurrency/)
- [hydro-project/infinity](https://github.com/hydro-project/infinity)
- [google-research/harness2](https://github.com/google-research/harness2)
- [arXiv 2610.14224](https://arxiv.org/abs/2610.14224)
