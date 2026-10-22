# Bridle

**A coding agent that runs on your own machine, with the checks a small model needs.**

Bridle connects [OpenCode](https://opencode.ai) to a small local model (Qwen3.5-9B on an 8 GB GPU). The model does
the coding. Bridle decides what the model may run and read, runs the tests itself to check the result, keeps secret
values out of the model's context, and writes a log of every decision.

Small local models are fast and private, but they make mistakes and ignore instructions. Bridle keeps the model's
power for the work it does well and takes away the decisions that should not depend on the model's judgement.

> **Status:** release 1, Windows native, one tested machine. Milestones M0 to M2 are complete. M3 (state and loop
> detection) is next. Details in [docs/progress.md](docs/progress.md).

## What it does

- **Checks every tool call.** Each command gets a class from `READ_ONLY` to `PRIVILEGED`. Unknown programs and
  commands it cannot parse are denied. Nothing that would need a human's approval is allowed.
- **Keeps secrets away from the model.** Secret files (`.env`, keys, credential files) are denied for file tools and
  shell arguments. Output is redacted before the model sees it, and the event log holds no secret values.
- **Checks results by running them.** Twenty deterministic tools let the model read git history, navigate code,
  check dependencies and run the project's own tests, linter and type checker. Failures are parsed into file, line
  and message.
- **Keeps the model local.** Model endpoints must be loopback addresses. External providers are off in the local-only
  profile.
- **Lets you add tools.** You can declare your own commands as tools in the machine config. They pass the same checks.

## How a tool call flows

1. The model asks OpenCode to run a tool: `bash`, `read`, `edit`, or one of Bridle's `local-agent_*` tools.
2. A thin plugin shim in OpenCode calls `agent hook tool-before`. The shim holds no policy logic.
3. Bridle checks the paths, the secret-file rules, the shell arguments and the command class.
4. Only an Allow passes. Any other verdict blocks the call, and the model sees the reason.
5. The output of an allowed call is redacted before the model sees it.
6. Every decision goes to `.local-agent/tool-events.jsonl` in the repository, with secret values left out.

If Bridle cannot answer, the call is blocked. A broken config blocks everything. See
[docs/architecture.md](docs/architecture.md) for the layers and where state lives.

## Quick start (Windows)

**You need:** Windows 11, Visual Studio 2022 with the C++ workload, an NVIDIA GPU with a current driver,
OpenCode 1.19.3, and llama.cpp b11756 (CUDA 12.4). The Rust version is pinned in `rust-toolchain.toml`.
Model weights are not included. The full list is in [docs/installation.md](docs/installation.md).

Run these from a shell with the Visual Studio environment loaded (`vcvars64.bat`):

```bat
:: 1. Build the agent command
set CARGO_TARGET_DIR=D:\cargo-target\harness
cargo build -p harness-cli --bin agent

:: 2. Tell Bridle where the models and runtimes are (once per machine)
D:\cargo-target\harness\debug\agent.exe init --machine --models-dir D:\models --runtimes-dir D:\runtimes --opencode-bin D:\runtimes\opencode\1.19.3\opencode.exe

:: 3. Set up a repository
cd /d path\to\your\repo
D:\cargo-target\harness\debug\agent.exe init --profile local-only
D:\cargo-target\harness\debug\agent.exe config validate

:: 4. Start the model and check it
D:\cargo-target\harness\debug\agent.exe models serve main --detach
D:\cargo-target\harness\debug\agent.exe health main

:: 5. Give the agent a task (or run opencode with no arguments for the interactive interface)
D:\runtimes\opencode\1.19.3\opencode.exe run "your task here"

:: Check the installation at any time
D:\cargo-target\harness\debug\agent.exe doctor
```

## Security

Bridle is a set of guardrails, not a sandbox. The full model is in [docs/security.md](docs/security.md).

- **Fail closed.** `Ask` is never allowed. An unanswered permission is denied.
- **The model cannot change its own policy.** The agent cannot write Bridle's control files or `.git`.
  Repository config can only make rules stricter, never looser.
- **Paths stay in the workspace.** Traversal, symlinks, device names and unresolved `~` or `$VAR` are denied.
- **Known limits.** An allowed test run executes code the model wrote, with your account's rights. A recursive search
  such as `grep -r` can read secret files without naming them, so output redaction is the only protection for those
  values. Run Bridle on repositories you would run yourself. See [residual risks](docs/security.md#residual-risks).

## Models

| Role | Model | Status |
|---|---|---|
| Main coding model | Qwen3.5-9B, Q4_K_M, served by llama.cpp | In use (release 1) |
| Micro model (classifier and summariser) | LFM2.5-1.2B-Thinking | Planned, M4 (research use) |
| Deep model | Ternary Bonsai 2 27B, PTQ1_0 | Planned, M5. Needs the PrismML llama.cpp fork |

Measured on the tested machine: the main model is ready about 11 seconds after start and uses about 5.4 GB of the
8 GB VRAM. The two larger models cannot run together on 8 GB. Numbers and checksums are in
[docs/models.md](docs/models.md).

## Tools for the model

Registered in OpenCode as `local-agent_<name>`.

| Group | Tools |
|---|---|
| Git | `git_status`, `git_diff`, `git_history`, `git_log_range` |
| Code navigation (tree-sitter: Rust, Python, C++) | `repo_map`, `file_summary`, `symbol_search`, `find_references` |
| Dependencies | `dependency_search` (Cargo, pyproject, requirements, package.json) |
| Verification | `run_verification`, `run_tests`, `run_linter`, `run_typechecker`, `parse_test_failure` |
| Commands and config | `run_command` (one program, no shell), `inspect_config` (secrets redacted), `harness_info` |
| Results | `inspect_artifact`, `summarize_experiment`, `compare_experiments` |
| Your tools | Adapters declared in the machine config |

`find_references` is syntactic: it finds identifiers by name, not by type. There is no persistent symbol index yet (M3).

## Roadmap

| Milestone | Scope | State |
|---|---|---|
| M0 | Audit and architecture | Done |
| M1 | Basic integration on Windows: policy, hook, runtime, OpenCode | Done |
| M2 | Deterministic tools | Done |
| M3 | State and loop detection, event log in a state directory | Next |
| M4 | Micro model as classifier or summariser (LFM2.5) | Planned |
| M5 | Deep model on demand (Bonsai), one GPU model at a time | Planned |
| M6 | Context management | Planned |
| M7 | Jev integration with TLS and endpoint policy | Planned |
| M8 | LiteLLM for external providers, behind the policy | Planned |
| M9 | Security threat-model tests and egress audit | Planned |
| M10 | Evaluation on a Rust coding benchmark | Planned |
| M11 | Open-source release: licence, README, CI | Planned |

The same stages as a diagram, with status colours, are in
[docs/diagrams/bridle-stages.drawio](docs/diagrams/bridle-stages.drawio) (page "Roadmap"). Open the file in
[draw.io](https://app.diagrams.net). Page "Tool call flow" in the same file shows the flow below.

## Documentation

| Document | What it covers |
|---|---|
| [Architecture](docs/architecture.md) | Layers, request flow, where state lives |
| [Installation](docs/installation.md) | Windows prerequisites, build, first run |
| [Configuration](docs/configuration.md) | Machine config, project config, profiles, validation rules |
| [Security](docs/security.md) | Decisions, command classes, secret handling, residual risks |
| [Models](docs/models.md) | Weights, checksums, licences, measured numbers |
| [Troubleshooting](docs/troubleshooting.md) | Problems seen on the tested machine and their fixes |
| [Progress log](docs/progress.md) | What is verified, what is not, and when |
| [Decisions (ADRs)](docs/adr/) | Host boundary, runtime OS and scope, tree-sitter for code intelligence |
| [Module notes](docs/modules/README.md) | One page per crate module |
| [Milestones](docs/milestones/) | Milestone plans and results, for example [M2](docs/milestones/m2-tools.md) |
| [Spec and traceability](docs/spec/) | Master development plan and requirements traceability |
| [Audit](docs/audit/) | Repository architecture audit (M0) |

## Repository layout

```
crates/harness-core      config, security policy, command classifier, redaction (no network)
crates/harness-runtime   model providers, health checks, server launcher, event log, MCP server and tools
crates/harness-cli       the `agent` command and the OpenCode adapter
opencode-plugin/         thin TypeScript shim (no policy logic)
configs/                 shipped profiles: local-only, development
examples/                toy Python project used for the end-to-end run
docs/                    architecture, installation, security, decisions, progress log
```

Some file names still use the earlier working name `local-agent` (for example `local-agent.toml` and
`.local-agent/`). The binary is `agent`.

## Development

```bat
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The build uses a pinned Rust toolchain (`rust-toolchain.toml`). Build output goes to `D:\cargo-target\harness`.

## Licence

No licence has been chosen yet. The project lead has decided on Apache-2.0, but no LICENSE file is in the repository,
and the workspace `Cargo.toml` still marks the licence as pending. No licence applies until a LICENSE file is added.
Model weights are not included and keep their own licences (see [docs/models.md](docs/models.md)).

## Author

Kushagra Srivastava

## Contributors

- trickybutshruti, on the documentation
