# M7: Jev decision backend and endpoint policy

M7 adds Jev as an optional `DecisionEngine` backend (R-9.1) and the endpoint policy and TLS that an external service
needs. Jev decides stalled loops only, as the micro model does (ADR-0005, ADR-0007).

> **Status:** Built and tested (2026-10-27), committed as `6ad8861`, with this page committed after it. **Not live:**
> no Jev service or key was available, so the decision path is tested against a local mock of the contract.

## What was built

- `harness-core/src/jev.rs`: the answer contract. The decision must be one of ESCALATE, ASK_USER or REPLAN. The
  confidence must be a number from 0 to 1, and it is kept. A short reason is cleaned and capped at 200 characters.
  Anything else is rejected with a named reason.
- `harness-runtime/src/jev.rs`: `JevEngine`, which implements `DecisionEngine`. It asks the rules first; only a stalled
  loop goes to Jev. The request is `schema`, `question`, a bounded `state`, the signal names and the choices. It holds no
  prompt, no command text and no tool output. A failure falls back to the rules, labelled
  `rules (fallback from jev: <reason>)`.
- `harness-cli/src/workflow.rs`: `decide_for` selects Jev when `decisions.provider = "jev"`, and the rules when Jev is not
  enabled, with the reason recorded.
- Configuration: `[external.jev]` has `enabled`, `endpoint`, `api_key_env` and `timeout_ms` (default 5000). Validation
  rules V30 to V35 are in [configuration](../../guide/configuration.md#external-decision-service-jev-m7).
- `configs/local-with-jev.toml`: the profile. Jev is off in it until the endpoint and key name are set.
- `harness-runtime/src/http.rs`: `local_agent` (loopback, no proxy) and `remote_agent` (environment proxy, native-tls).

## TLS finding

The first build used ureq's `rustls` backend. Every HTTPS call stalled in the handshake until the timeout. Plain TCP
to the host connected in about 50 ms, and curl reached the same host in under a second through the environment
proxy (`HTTPS_PROXY`). The cause was the route: this network passes outbound HTTPS through the configured proxy,
and the agent had been built with the proxy turned off. Two changes fixed it:

1. The remote agent uses the environment's proxy and the native-tls backend (SChannel with the system store).
2. The loopback agent keeps the proxy turned off, as before.

Measured after the change: a POST to a public host returned an HTTP status in about 2.6 s. The test is kept as an
ignored test, `tls_handshake_reaches_a_public_host`, and runs with `--ignored` on a machine with network access.

## Tests

- Core: five tests of the contract (a checked answer, a fenced answer, each rejection, cleaning and capping of the
  reason, capping of a rejected name).
- Validation: two tests of V30 to V35 and of loopback http, which is allowed without a key.
- Runtime: six tests with a local mock server. A checked answer keeps its confidence, and the key goes in a bearer
  header; a failed call falls back and never shows the key; an answer outside the contract falls back; an unreachable
  endpoint falls back; a key that is not set is an error before any call; the request names no prompt or command.
- CLI: two tests of `decide_for`: an unreachable Jev falls back to the rules, and a disabled Jev is not called.
- Live: one ignored TLS check against a public host (above).
- Gate: `wgate` passes (fmt, clippy with warnings denied, 657 tests; 2 ignored).

## Limits and what remains

- The contract is this project's own. The real Jev service may use other paths or fields. It must be checked against the
  service before Jev is enabled anywhere.
- No live decision was made. The A/B run (rules against LFM against Jev, R-39.3, R-57.7) needs a Jev service.
- Only stalled loops reach Jev. A wider use is a question for an A/B run.
- With Jev enabled, the last action text (capped at 120 characters) leaves the machine. See the security page.
- Linux builds need OpenSSL development files for native-tls. The CI job (M11) will install them.

## Related

- [ADR-0007](../../decisions/0007-jev-is-an-optional-decision-backend.md)
- [Module: orchestrator](../../reference/modules/orchestrator.md#jev-decision-backend-m7)
- [Requirements register](../../reference/requirements-traceability.md): the M7 rows
