# Bridle documentation

This folder holds the documentation for Bridle, a local-first harness and orchestrator for multi-agent coding. It is
grouped by what you need to do: install and run it, understand how it works, look up a module, or read why a decision
was made.

## Where to start

| If you want to | Read |
|---|---|
| Install Bridle and run the first task | [guide/installation.md](guide/installation.md) |
| Change the machine or repository configuration | [guide/configuration.md](guide/configuration.md) |
| Open the interactive control center | [guide/tui.md](guide/tui.md) |
| Fix a problem seen on the tested machine | [guide/troubleshooting.md](guide/troubleshooting.md) |
| Know which models and runtimes are used, and where to get them | [guide/models.md](guide/models.md) |
| Understand the layers and the path of a tool call | [concepts/architecture.md](concepts/architecture.md) |
| Understand what the policy allows, denies, and does not protect | [concepts/security.md](concepts/security.md) |
| Look up a Rust module and its rules | [reference/modules/README.md](reference/modules/README.md) |
| Read why a decision was made | [decisions/](decisions/) |

## Conventions

- Commands are shown exactly as they are run, in a fenced block with a language tag.
- Paths are shown as they are on the tested machine: `D:\models`, `D:\runtimes`, `D:\cargo-target\harness`.

## Related

- [../README.md](../README.md): project overview and quick start
