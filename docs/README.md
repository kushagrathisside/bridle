# Bridle documentation

This folder holds the documentation for Bridle, a local-first harness and orchestrator for multi-agent coding. It is
grouped by what you need to do: install and run it, understand how it works, check where it stands, look up a
module, or read why a decision was made.

> **Status:** release 1, native Windows. Milestones M0 to M6 are done and committed. The current status is in
> [status/progress.md](status/progress.md).

## Where to start

| If you want to | Read |
|---|---|
| Install Bridle and run the first task | [guide/installation.md](guide/installation.md) |
| Change the machine or repository configuration | [guide/configuration.md](guide/configuration.md) |
| Fix a problem seen on the tested machine | [guide/troubleshooting.md](guide/troubleshooting.md) |
| Know which models and runtimes are used, and where to get them | [guide/models.md](guide/models.md) |
| Understand the layers and the path of a tool call | [concepts/architecture.md](concepts/architecture.md) |
| Understand what the policy allows, denies, and does not protect | [concepts/security.md](concepts/security.md) |
| See what has been verified, and when | [status/progress.md](status/progress.md) |
| Look up a Rust module and its rules | [reference/modules/README.md](reference/modules/README.md) |
| See which requirement of the brief is met, and where | [reference/requirements-traceability.md](reference/requirements-traceability.md) |
| Read why a decision was made | [decisions/](decisions/) |

## Structure

```
docs/
  guide/              how to install, configure, troubleshoot, and choose models
  concepts/           architecture and security model
  status/             progress log and milestone records
  reference/          module notes and the requirements register
  decisions/          architecture decision records (ADRs)
  spec/               the project brief, kept verbatim
  archive/            snapshots that are no longer kept current
  diagrams/           draw.io diagrams: roadmap and tool-call flow
```

## Conventions

- Each page starts with a summary and a status line. The status line says whether the content is verified or planned.
- The status log in [status/progress.md](status/progress.md) is the source for milestone status. Other pages must agree
  with it.
- Commands are shown exactly as they are run, in a fenced block with a language tag.
- Paths are shown as they are on the tested machine: `D:\models`, `D:\runtimes`, `D:\cargo-target\harness`.

## Related

- [../README.md](../README.md): project overview, quick start and roadmap
- [spec/master-development-plan.md](spec/master-development-plan.md): the project brief, verbatim
