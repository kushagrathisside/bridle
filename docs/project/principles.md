# Design principles

These principles explain the choices in Bridle. When a behaviour surprises you, it is usually one of these at work.

## The model proposes; the harness decides

The model asks for tool calls. Bridle decides whether they run. The model cannot change the policy, the settings, or the
rules that check its calls, because those live outside the model's reach. Changing the policy means changing the
configuration, which the agent cannot write.

## Fail closed

When Bridle cannot answer a call, the call is blocked. An unreadable configuration blocks every call. A shell command that
cannot be parsed is blocked. A tool name the server does not know is blocked. The safe answer is always "no", and the
message says why.

## Rules you can read

Decisions come from rules written in code. The same call in the same state gets the same answer. There is no model in
the policy path, so no answer depends on how the model happened to phrase a request.

The one place a model helps is the optional decision for a stalled loop. Its answer is limited to three choices, checked
strictly, and it cannot allow or block a call on its own. See [models and slots](../concepts/models.md).

## Local first

The default configuration runs on one machine. Model endpoints must be on loopback. External services are off, and
turning one on requires an explicit endpoint, HTTPS for a remote host, and a key read from an environment variable.
Bridle does not send the repository's files anywhere.

OpenCode itself fetches its model catalogue when it starts. That is outside Bridle's control, and it is listed in
[status and limits](status.md).

## Small surface, one adapter

Bridle's core does not know about OpenCode. Only one module reads OpenCode's payloads, and the plugin that OpenCode
loads contains no policy. If OpenCode changes, the change is confined to that module, and the policy stays the same.

## Evidence before claims

A statement in these docs is one that the code, a test or a recorded run supports. Where a result has not been verified
on the tested machine, the page says so. A known problem is listed rather than hidden.

## Limits, stated

Bridle is not a sandbox. An allowed test run executes code the model wrote, with your user's rights. Bridle reduces what
the agent can do without asking, and it keeps secrets out of the model's view where it can. It does not contain a
determined program. The [security model](../concepts/security.md) lists the risks that remain.
