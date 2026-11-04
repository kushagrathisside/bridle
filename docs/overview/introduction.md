# Introduction

**Bridle is a local-first coding agent for Windows. It runs on your own GPU, and every action it takes passes through a policy you
can read.**

You open a folder, type `bridle`, and a window opens: a chat with a model that runs on your machine, a code editor with diffs,
project search, a real terminal and image input. The model reads and edits your project with tools. A policy written in code, not in
a prompt, decides what each call may do. No shell command runs without a verdict, a secret value never reaches the model, and a
refusal names the rule that caused it.

> **The model proposes; the harness decides.**

Bridle needs no cloud service. The model is a server you start yourself through [llama.cpp](https://github.com/ggml-org/llama.cpp),
on loopback, and the configuration rejects any model endpoint that is not on this machine unless you turn external providers on. The
only network use is the one you switch on.

## Who it is for

Bridle is built for one developer's daily work first. It suits you when you:

- want a coding agent on local models, so source code and secrets stay on your computer;
- run an agent on real repositories and want its permissions decided by rules that are written down and tested, not by how a prompt
  happened to be phrased;
- want to see why a call was blocked, in the agent's own transcript and in a log you can read.

It is not a fit when you need a hosted model, or when you work on Linux or macOS: this release runs on native Windows 11 only. See
[status and limits](../project/status.md).

## The problem it solves

An agent with a shell and a file editor can do useful work, and it can also read a `.env` file, send a request to a remote host,
edit the same file thirty times while a test keeps failing, or fill its context with a log thousands of lines long. A model's good
intentions do not stop any of these. Bridle answers each with a rule that runs outside the model:

| Risk | What Bridle does |
|---|---|
| The agent runs something it should not | Every tool call is classified and given a verdict (allow, ask or deny) before it runs. `ask` is never allowed, and a failure to decide blocks the call. See [how decisions are made](../concepts/how-decisions-are-made.md). |
| A secret reaches the model | Secret files are denied to file tools and shell commands, secret-looking environment variables are blanked, and secret values in output are redacted before the model sees them. |
| The agent loops | Repeated edits and repeated commands are refused with the rule named. See [tasks and loops](../concepts/tasks-and-loops.md). |
| Long outputs fill the context | Old, large tool results are shortened in each request, and old turns are folded into a rolling summary. The stored session is not changed. |
| The GPU runs out of memory | A model server is refused when the model file's own header says it will not fit. |

## What is in the box

| Area | What it does | Read more |
|---|---|---|
| The window | Chat, editor, diffs, search, terminal, images, a project switcher | [The desktop app](../guide/desktop.md) |
| A team and a board | Named members you tag with `@`; a board of issues they split and work | [Team and board](../guide/team-and-board.md) |
| Lanes | Separate working copies for parallel members, merged by you | [Lanes](../guide/lanes.md) |
| A small-model fleet | Small models loaded together while the big one is unloaded | [The fleet](../guide/fleet.md) |
| Background commands | Commands that run while the model works, and notices that tell it what happened | [Background commands](../guide/background-commands.md) |
| Web research and a browser | Search, pages, downloads and a pinned Chromium, all off by default | [Web research](../guide/web-research.md), [the browser](../guide/web-browser.md) |
| Three hosts, one policy | The window, the terminal UI and OpenCode all go through the same decision path | [Architecture](../concepts/architecture.md) |

## What Bridle is not

- **It is not a sandbox.** An allowed test run executes code the model wrote, with your user's rights. Read the
  [security model](../concepts/security.md) before you rely on it.
- **It does not contain a model.** You download the model files and the llama.cpp runtime yourself. See [models](../concepts/models.md).
- **It does not replace OpenCode if you prefer it.** OpenCode can still run the agent loop and the interface, with Bridle's policy
  around it. The window is Bridle's own front end to the same policy.

## Where to go next

Start with [getting started](getting-started.md). It takes about fifteen minutes if the model files are already downloaded. The
whole project in one page is the [README](https://github.com/kushagrathisside/bridle#readme); the plain-text
[instruction manual](https://github.com/kushagrathisside/bridle/blob/main/docs/MANUAL.txt) ships in the release zip.
