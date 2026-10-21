# Local-first coding harness (working name: local-agent)

A coding agent that runs on your own machine. A small local model does the work inside OpenCode. The
harness around it decides what the model may do, checks the results by running real commands, keeps secrets out
of the model's context, and records every decision.

Status: **release 1, Windows native, M1 in progress.** The end-to-end loop works on one machine: OpenCode,
the plugin, the policy, a local Qwen model, an edit, and a test run. See [docs/progress.md](docs/progress.md).

## Why

Small local models are fast and private, but they make mistakes and ignore instructions. The harness keeps
the model's power where it is useful and takes the decisions that matter away from it: which commands run, which
files it can read, and whether a change is really done.

## Documentation

- [Architecture](docs/architecture.md): layers, request flow, where state lives
- [Installation](docs/installation.md): Windows setup, build, first run
- [Configuration](docs/configuration.md): machine config, project config, profiles, validation rules
- [Security model](docs/security.md): decisions, command classes, secret handling, residual risks
- [Models and runtimes](docs/models.md): weights, checksums, licences, measured numbers
- [Troubleshooting](docs/troubleshooting.md): problems seen on the target machine and their fixes
- [Progress log](docs/progress.md): what is verified, what is not, and when
- [Decisions (ADRs)](docs/adr/): ADR-0001 host boundary, ADR-0002 runtime OS and release scope

## Quick start (Windows)

See [docs/installation.md](docs/installation.md). In short: build `agent`, run `agent init --machine` with your
model and runtime folders, then in a repo run `agent init --profile local-only`, `agent models serve main --detach`,
and `opencode run "task"`.

## Layout

```
crates/harness-core      config, policy, command classifier, redaction (no I/O beyond reading config)
crates/harness-runtime   model providers, health checks, server launcher, MCP server
crates/harness-cli       the `agent` command and the OpenCode adapter
opencode-plugin/         thin TypeScript shim (no policy logic)
configs/                 shipped profiles
examples/                toy Python repo used as the end-to-end target
docs/                    documentation, decisions, specs, audits
```

## Licence

Not yet chosen. The project lead's decision is Apache-2.0 (decision D7); a LICENSE file will be added before
any publication. Model weights are not included and keep their own licences (see [docs/models.md](docs/models.md)).
