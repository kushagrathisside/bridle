# Introduction

Bridle is a local-first harness for coding agents. It runs inside [OpenCode](https://opencode.ai) and sits between the
agent and the tools it calls. Every tool call is decided before it runs. Secret files are denied, command output is
redacted before the model reads it, repeated failing calls are refused, and each decision is recorded with its reason.

Everything runs on your machine. Bridle does not need a cloud service: the model is a server you start yourself, on
loopback, and the configuration rejects any model endpoint that is not on this machine unless you turn external
providers on.

## Who it is for

Bridle is for a developer who runs a coding agent on their own repositories and wants two things: local models, and a
clear answer to the question "what is this agent allowed to do right now?"

It is a good fit when you:

- run an agent unattended, and want its permissions decided by rules that are written down and tested;
- keep source code and secrets out of hosted services;
- want to see why a tool call was blocked, in the agent's own transcript and in a log you can read.

It is not a fit when you need a hosted model, or when you work on Linux or macOS: the current release runs on native
Windows only. See [status and limits](../project/status.md).

## The problem

An agent with a shell and a file editor can do useful work, and it can also read a `.env` file, send a request to a
remote host, edit the same file thirty times while a test keeps failing, or fill its context with a log that is
thousands of lines long. None of these is blocked by the model's good intentions alone.

Bridle answers each one with a rule that runs outside the model:

- **A policy for every tool call.** Each call is classified, checked against your settings, and allowed, denied, or
  refused. The reason is returned to the agent. See [how decisions are made](../concepts/how-decisions-are-made.md).
- **Secrets stay out of the model's context.** Secret files are denied to file tools and to shell commands. Output that
  contains a secret value is redacted before the model reads it.
- **Loops are caught.** When the same file is edited again and again, or the same command repeats without progress, the
  next call is refused and the agent is told which rule fired. See [tasks and loops](../concepts/tasks-and-loops.md).
- **Long outputs do not fill the context.** Old, large tool results are shortened in each request. The stored session
  is not changed.

## What Bridle is not

- **It is not a sandbox.** An allowed test run executes code the model wrote, with your user's rights. Read the
  [security model](../concepts/security.md) before you rely on it.
- **It does not contain a model.** You download and run the model servers yourself. See [models](../concepts/models.md).
- **It does not replace OpenCode.** OpenCode runs the agent loop and the interface. Bridle adds the rules around it.

## Compared with the alternatives

- **OpenCode on its own.** OpenCode can ask you before a tool runs. Bridle turns those questions into rules and denies
  anything the rules would ask about, because an unattended run has no one to answer. Use OpenCode alone when you are
  at the keyboard and the prompts are enough.
- **OpenCode with a local model and no harness.** You get the same agent, but no per-call policy, no secret redaction
  layer, no loop refusals, and no decision record. Bridle adds those.
- **A hosted coding agent.** Bridle keeps the model and the repository on your machine. It is slower to set up, and the
  local models are smaller than the hosted ones.

## Where to go next

Start with [getting started](getting-started.md). It takes about fifteen minutes if the model files are already
downloaded.
