# M8: LiteLLM gateway behind the policy

M8 adds LiteLLM as an optional gateway for model traffic (R-8.3, R-7.2, R-37.1). OpenCode can reach local, cloud and
decision providers through it, and the agent's endpoint policy covers the gateway like any other slot. The design is in
[ADR-0008](../../decisions/0008-litellm-is-an-optional-gateway-behind-the-policy.md).

> **Status:** Built and tested with a mock (2026-10-27). The live gateway check passed on 2026-10-28 with a LiteLLM 1.106.4
> gateway on `127.0.0.1:4000` in front of the main model. See "Live check" below.

## What was built

- `ProviderKind::Litellm` (`provider = "litellm"`): an OpenAI-compatible slot. Its `id` is the gateway's route name.
- `provider_for` builds the same OpenAI-compatible client for it. The request names the route in `model`, and the key from
  `api_key_env` goes in a bearer header.
- Transport: a loopback gateway uses the no-proxy agent. Any other host uses the environment's proxy and native-tls
  (`remote_agent`). Before M8, every OpenAI-compatible slot used the no-proxy agent; a remote endpoint would have stalled
  on this network. The change applies to every OpenAI-compatible slot, not only gateways.
- Policy: V36, a gateway on a non-loopback host needs `api_key_env`. V4 and V3 already apply to the endpoint.
- OpenCode config: a slot with `api_key_env` is sent as `{env:NAME}`, so the key is read when OpenCode starts and is not written
  to the generated file. The OpenCode side of this (that OpenCode expands `{env:NAME}` in `apiKey`) is not verified yet.
- `configs/hybrid.toml`: the local profile with a gateway slot (`role = "external"`, `hardware = "remote"`). It names
  `http://127.0.0.1:4000/v1`, the key name `BRIDLE_GATEWAY_KEY`, and the route `local-qwen`.
- `agent config validate` checks the hybrid profile, `local-only` and `local-with-jev`; a remote gateway without a key name
  fails with V36.

## Tests

- Core: two tests of V36 (a remote gateway without a key name is rejected; a loopback gateway is not) and one of the loopback
  classification.
- Runtime (mock server, always run): a LiteLLM slot sends its route name and the gateway key from its variable.
- Runtime (ignored, live): `a_gateway_slot_completes_a_request`, which needs a running gateway on `BRIDLE_GATEWAY_URL` (default
  `http://127.0.0.1:4000/v1`) and its key in `BRIDLE_GATEWAY_KEY`.
- Gate at the M8 code commit (`0854f62`): fmt and clippy clean; 660 tests pass, 3 ignored. The mock test was added after
  that, in `a3a6940`, and passes on its own.

## Live check

Planned: a LiteLLM proxy (1.106.4, installed in a Windows virtual environment) in front of the running `main` model
(Qwen3.5-9B on port 8001), with the route `local-qwen`. The steps are: start the model, start the gateway on
`127.0.0.1:4000`, run `agent models health gateway`, then run the ignored test with `--ignored`.

**Status: passed (2026-10-28).** The gateway was installed into a Windows virtual environment at `D:\llm-gateway\.venv`
and started by `D:\llm-gateway\start-litellm-gateway.ps1`, which the person ran in PowerShell. Results:

- `GET /health/liveliness` on `127.0.0.1:4000`: HTTP 200.
- A chat completion with model `local-qwen` and the gateway key reached the main model. The reply was cut at 20 tokens
  because the model's reasoning used the budget; the route itself worked.
- The ignored test `a_gateway_slot_completes_a_request` passed with the gateway running.

Not run: `agent models health gateway` (the planned step), and any cloud route. The gateway key in the script is a local
test value for loopback only. Its config is `D:\llm-gateway\litellm-config.yaml`.

## Limits

- No route leaves the machine in the tested setup. A cloud route is not tested, and no key was used.
- No loop rule or decision selects the gateway. Escalation is a later choice.
- The agent does not see the gateway's onward routes. The gateway's own file is the record of them.
- The OpenCode expansion of `{env:NAME}` is unverified; the live OpenCode run through the gateway would show it.

## Related

- [ADR-0008](../../decisions/0008-litellm-is-an-optional-gateway-behind-the-policy.md)
- [Configuration: advanced mode](../../guide/configuration.md#advanced-mode-a-litellm-gateway-m8)
- [Requirements register](../../reference/requirements-traceability.md): the M8 rows
