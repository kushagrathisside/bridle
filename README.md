# Bridle

Bridle is a local-first coding-agent harness. A small model runs on your own machine and does the coding work
inside [OpenCode](https://opencode.ai). Around the model, deterministic checks decide what it may do, verify its
changes by running real commands, keep secret values out of its context, and record every decision.

## Why

Small local models are fast and private, but they make mistakes and ignore instructions. Bridle keeps the model's
power for the work it does well and takes away the decisions that matter: which commands run, which files can be
read, and whether a change is really finished.

## Status

- **Milestone M2 (deterministic tools) is complete.** M3 (state machine and loop detection) is next.
- **Platform:** Windows native, release 1.
- What is verified, and what is not, is recorded in [docs/progress.md](docs/progress.md).

## How it works

A tool call from the model goes through these steps:

1. The model asks OpenCode to run a tool, such as `bash`, `read`, `edit`, or a Bridle MCP tool.
2. OpenCode calls the plugin shim before the tool runs. The shim forwards the call to `agent hook tool-before`
   and applies the answer. It holds no policy logic.
3. The CLI maps the call to a request and evaluates it against the policy: paths, secret-file rules, shell
   arguments, and command class.
4. Only an Allow passes. Any other verdict blocks the call, and OpenCode shows the reason to the model. If the hook
   cannot answer, the call is blocked.
5. After the tool runs, its output is redacted of secret values before the model sees it.
6. Every decision is written to `.local-agent/tool-events.jsonl`, with secret values left out.

The layers and where state lives are described in [docs/architecture.md](docs/architecture.md).

## Quick start (Windows)

Prerequisites, the folder layout for model weights and runtimes, and proxy notes are in
[docs/installation.md](docs/installation.md). The command-line binary is `agent`.

```bat
:: Build, from a shell with the Visual Studio environment loaded (vcvars64.bat)
set CARGO_TARGET_DIR=D:\cargo-target\harness
cargo build -p harness-cli --bin agent

:: Set the machine config once
D:\cargo-target\harness\debug\agent.exe init --machine --models-dir D:\models --runtimes-dir D:\runtimes --opencode-bin D:\runtimes\opencode\1.19.3\opencode.exe

:: In your repository
D:\cargo-target\harness\debug\agent.exe init --profile local-only
D:\cargo-target\harness\debug\agent.exe config validate
D:\cargo-target\harness\debug\agent.exe models serve main --detach
D:\cargo-target\harness\debug\agent.exe health main
D:\runtimes\opencode\1.19.3\opencode.exe run "your task here"

:: Check the installation
D:\cargo-target\harness\debug\agent.exe doctor
```

Run `opencode` without `run` for the interactive interface.

## Security

This summarises [docs/security.md](docs/security.md). It is a practical model, not a formal audit.

- **Secrets stay out of the model's context and logs.** Secret-file paths are denied for file tools and shell
  arguments, and output is redacted. See [Secret handling](docs/security.md#secret-handling).
- **Fail closed, and `Ask` is never allowed.** Unknown programs and unparseable commands are denied, and a missing or
  broken config blocks every call. See [Decisions](docs/security.md#decisions-that-shape-the-policy).
- **The model cannot change the policy.** Arguments are inspected rather than trusted, and the agent cannot write
  the harness's control files or `.git`. See [Decisions](docs/security.md#decisions-that-shape-the-policy).
- **Commands are classified before they run.** Each command gets a class from READ_ONLY to PRIVILEGED, and a
  compound command gets the class of its most dangerous part. See [Command classes](docs/security.md#command-classes).
- **Paths stay inside the workspace, and model endpoints are loopback only.** See [Paths](docs/security.md#paths)
  and [Network](docs/security.md#network).
- **Known residual risks.** Bridle is not a sandbox. An allowed test run executes code the model wrote, with your
  user account's rights. A recursive search such as `grep -r` can read secret files without naming them, so output
  redaction is the only protection for those values. Run Bridle on repositories you would run yourself. See
  [Residual risks](docs/security.md#residual-risks).

<details>
<summary>What leaves the machine</summary>

The local-only profile runs models on this machine and allows only loopback model endpoints. OpenCode fetches its
model catalog from models.dev when it starts; this is the known exception described in
[troubleshooting.md](docs/troubleshooting.md).

</details>

## Documentation

| Document | What it covers |
|---|---|
| [architecture.md](docs/architecture.md) | Layers, request flow, where state lives |
| [installation.md](docs/installation.md) | Windows prerequisites, build, first run |
| [configuration.md](docs/configuration.md) | Machine config, project config, profiles, validation rules |
| [security.md](docs/security.md) | Decisions, command classes, secret handling, residual risks |
| [models.md](docs/models.md) | Model weights, checksums, licences, measured numbers |
| [troubleshooting.md](docs/troubleshooting.md) | Problems seen on the target machine and their fixes |
| [progress.md](docs/progress.md) | What is verified, what is not, and when |
| [adr/](docs/adr/) | Architecture decisions: host boundary, runtime OS and scope, tree-sitter for code intelligence |
| [modules/](docs/modules/README.md) | Notes on the modules of each crate |
| [milestones/](docs/milestones/) | Milestone plans, for example M2 tools |
| [spec/](docs/spec/) | Master development plan and requirements traceability |
| [audit/](docs/audit/) | Repository architecture audit (M0) |

## Repository layout

```
crates/harness-core      config, model registry, security policy, redaction (no network)
crates/harness-runtime   model providers, health checks, server launcher, event log, MCP server and tools
crates/harness-cli       the `agent` command and the OpenCode adapter
opencode-plugin/         thin TypeScript shim (no policy logic)
configs/                 shipped profiles: local-only, development
examples/                example project (toy-python-repo)
docs/                    architecture, installation, security, ADRs, progress log
```

Some file names still use the earlier working name `local-agent` (for example `local-agent.toml` and
`.local-agent/`). The binary is `agent`.

## Licence

No licence has been chosen yet. The project lead's decision is Apache-2.0, but no LICENSE file is in the repository
yet, and the workspace `Cargo.toml` marks the licence as pending. No licence applies until a LICENSE file is added.
Model weights are not included and keep their own licences (see [docs/models.md](docs/models.md)).

## Author

Kushagra Srivastava

## Contributors

- trickybutshruti, on the documentation
