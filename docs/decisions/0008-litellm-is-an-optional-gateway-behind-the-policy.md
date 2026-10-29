# ADR-0008: LiteLLM is an optional gateway, behind the same endpoint policy

- **Status:** Accepted (2026-10-27)
- **Date:** 2026-10-27
- **Deciders:** Kushagra Srivastava

## Context

LiteLLM is an optional gateway: OpenCode goes through it to reach local, cloud and decision
providers, and it routes models and keeps aliases. It must not be a dependency of local-only use. The gateway
holds its own provider keys, so the agent should not need them.

## Decision

1. A gateway is a model slot with `provider = "litellm"`. It speaks the OpenAI-compatible protocol, the same client as
   the other OpenAI-compatible slots. Its `id` is the route the gateway serves; the gateway maps it to a provider.
2. The agent holds only the name of the environment variable for the gateway's own key (`api_key_env`). Provider keys for
   cloud routes live in the gateway's configuration, not in the agent's.
3. The endpoint policy is the one that already applies to every slot, plus validation rule V36: a gateway on a host other
   than loopback needs a key name. Loopback gateways are local routes and need no cloud opt-in; a non-loopback gateway
   needs `external.cloud_models.enabled` and `https://` (V4).
4. Transport: a loopback gateway uses the no-proxy agent; any other host uses the environment's proxy and native-tls (the
   same choice as Jev, ADR-0007).
5. OpenCode receives the key as `{env:NAME}` in the generated config, so the key is read when OpenCode starts and is not
   written to the file.
6. No loop rule or decision selects the gateway slot. Escalation to a cloud route is an explicit choice, not an
   automatic one. The route exists and needs opt-in; automatic routing is not built.

## Consequences

- A user who wants cloud models adds routes to the gateway's own file, enables the cloud opt-in in the agent's config,
  and sets the key names. The agent's policy does not see the gateway's onward routes; the gateway's file is the
  record of them. This is stated in the configuration guide.
- The gateway is one more process on the machine. The live test needs one running; it is not started by the agent.
- The advanced mode is measured separately from the minimal mode. The comparison is part of the benchmark.

## Alternatives considered

- Making the gateway the default endpoint for OpenCode: rejected. It would add a network dependency to every local install.
- Storing the provider keys in the agent's config: rejected. The gateway already holds them, and two stores would drift.

## References

- [ADR-0007: the Jev backend and the endpoint policy](0007-jev-is-an-optional-decision-backend.md)
