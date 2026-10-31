# Models and slots

Bridle does not contain a model. It talks to model servers you run yourself, on this machine. Each server is a model
slot in the configuration. A slot names the model, the endpoint it answers on, and the role it plays. This page explains
the slots, which of them are needed, and which are optional.

For the download, the checksums and the start commands, see [serve the models](../guide/models.md).

## The slots

| Slot | Role | Model in the reference setup | When it runs |
|---|---|---|---|
| `main` | The agent's own model, used for every turn of the agent loop | Qwen3.5-9B, Q4_K_M, on the GPU | Always, for the agent. |
| `micro` | Decides a stalled loop among three answers, when the loop rules report one | LFM2.5-1.2B-Thinking, on the CPU | Only when the decision provider is set to `local_lfm` and micro decisions are enabled. Off by default. |
| `deep` | Takes over a stalled loop from the micro decision, once per task | Ternary Bonsai 2 27B, on the GPU | Only when deep escalation is enabled. Off by default. |

The `main` slot is required. `micro` and `deep` are optional, and Bridle works with `main` alone.

## One GPU model at a time

The `main` and `deep` models do not fit on an 8 GB GPU together. Bridle keeps one GPU model running at a time. When
deep escalation sends a stalled loop to `deep`, Bridle stops the other GPU model, starts `deep`, asks it, stops `deep`
and starts the other model again. On the tested machine this swap took about 15 seconds.

## Deep escalation

Escalation is off by default. When enabled, it applies only to an ESCALATE decision whose confidence reaches
`routing.escalation_threshold`, which is 0.75 by default. A task may use one escalation. The confidence of a
repeated-failure decision grows with the number of identical failures, so the default threshold is met at the third one.
The [configuration reference](../guide/configuration.md) lists the keys.

## Optional external services

Bridle can send a stalled-loop decision to an external decision service, Jev, and it can route model calls through a
LiteLLM gateway. Both are off in the default configuration, and both need an endpoint and, for remote hosts, an
environment variable that holds the key.

- **Jev** is built and tested against a local mock of its contract. No live Jev service has been checked.
- **LiteLLM** was checked against a local gateway on the tested machine. Its route name is set per slot.

Remote endpoints must use HTTPS. Plain HTTP is allowed only for a loopback address. See [status and limits](../project/status.md).

## The subagent tier

A small local model can answer a question about a few files, or condense a text, and give cited evidence. It is off by
default (`subagents.enabled`). When on, it uses the `micro` slot. Its answers are checked against the files they cite, and
the main model never receives the files' whole transcript.

## Endpoints must be local

Each slot's endpoint must be on this machine, unless external services are enabled over HTTPS. The configuration check
rejects any other address, so an unintended remote model cannot be configured by accident.
