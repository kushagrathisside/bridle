# Bridle documentation

Bridle is a local-first coding agent for Windows: it runs on your own GPU, and every action it takes passes through a policy you can
read. This folder is its documentation. It is grouped by what you need to do: install and run it, use a feature, understand how it
works, look something up, or read why a decision was made. The same pages are published as a searchable site (built from these files;
see [releasing](project/releasing.md#9-publish-the-documentation-site)). The whole project in one page is the
[README](../README.md), and the plain-text manual that ships in the release zip is [MANUAL.txt](MANUAL.txt).

## Where to start

| If you want to | Read |
|---|---|
| Know what Bridle is and whether it fits | [overview/introduction.md](overview/introduction.md) |
| Install it and run the first task | [overview/getting-started.md](overview/getting-started.md), [guide/installation.md](guide/installation.md) |
| Know what works and what does not | [project/status.md](project/status.md) |

## Using Bridle

| If you want to | Read |
|---|---|
| Use the window (`bridle`) | [guide/desktop.md](guide/desktop.md) |
| Use the terminal Control Center | [guide/tui.md](guide/tui.md) |
| Work with named team members, the board and the ideas queue | [guide/team-and-board.md](guide/team-and-board.md) |
| Let members work in parallel and merge their work | [guide/lanes.md](guide/lanes.md) |
| Use a fleet of small models | [guide/fleet.md](guide/fleet.md) |
| Run commands in the background and read notices | [guide/background-commands.md](guide/background-commands.md) |
| Search the web and download files, safely | [guide/web-research.md](guide/web-research.md) |
| Read pages that need JavaScript with a managed Chromium | [guide/web-browser.md](guide/web-browser.md) |
| Take a picture of a program's window | [guide/window-screenshots.md](guide/window-screenshots.md) |
| Change the machine or repository configuration | [guide/configuration.md](guide/configuration.md) |
| Set up a repository and declare its tests | [guide/set-up-a-repository.md](guide/set-up-a-repository.md), [guide/run-verification.md](guide/run-verification.md) |
| Know which models and runtimes are used, and where to get them | [guide/models.md](guide/models.md) |
| Add, update or remove a model, a runtime or a plugin | [guide/models-runtimes-and-plugins.md](guide/models-runtimes-and-plugins.md) |
| Put a crashed run right | [reference/run-recovery.md](reference/run-recovery.md) |
| Fix a problem seen on the tested machine | [guide/troubleshooting.md](guide/troubleshooting.md) |

## Understanding Bridle

| If you want to | Read |
|---|---|
| Understand the layers and the path of a tool call | [concepts/architecture.md](concepts/architecture.md) |
| Understand what the policy allows, denies, and does not protect | [concepts/security.md](concepts/security.md) |
| Understand how a call gets its verdict | [concepts/how-decisions-are-made.md](concepts/how-decisions-are-made.md) |
| Read the design principles | [project/principles.md](project/principles.md) |
| Read why a decision was made | [decisions/](decisions/) |
| See what Bridle learned from other projects | [spec/lessons-from-infinity-and-harness2.md](spec/lessons-from-infinity-and-harness2.md) |
| Read the research and plan for graph-based code understanding | [spec/graph-engineering-plan.md](spec/graph-engineering-plan.md) |

## Looking things up

| If you want | Read |
|---|---|
| Every command and its options | [reference/cli.md](reference/cli.md) |
| Every tool the model can call | [reference/mcp-tools.md](reference/mcp-tools.md) |
| A Rust module and its rules | [reference/modules/README.md](reference/modules/README.md) |
| The requirements and where each is met | [reference/requirements-traceability.md](reference/requirements-traceability.md) |

## For maintainers

| If you want to | Read |
|---|---|
| Build, test and change Bridle | [../CONTRIBUTING.md](../CONTRIBUTING.md) |
| Make a release, by hand, step by step | [project/releasing.md](project/releasing.md) |
| See the dated record of what was built and verified | [status/progress.md](status/progress.md) |
| Read the plan | [spec/team-subagents-ideas-plan.md](spec/team-subagents-ideas-plan.md) |

## Conventions

- Commands are shown exactly as they are run, in a fenced block with a language tag.
- Paths are shown as they are on the tested machine: `D:\models`, `D:\runtimes`, `D:\cargo-target\harness`.
- A page says what was **verified** and what was not. Where a result has not been checked on the tested machine, it is stated.
