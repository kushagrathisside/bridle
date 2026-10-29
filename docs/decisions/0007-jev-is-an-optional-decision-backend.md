# ADR-0007: Jev is an optional decision backend for stalled loops, behind an endpoint policy

- **Status:** Accepted (2026-10-27)
- **Date:** 2026-10-27
- **Deciders:** Kushagra Srivastava

## Context

Jev is an optional `DecisionEngine` backend. It returns structured, probabilistic decisions, and
its output must be kept as it is. It is an external service, so it must not be required, and a remote call must
not expose a key or a route the user did not choose. ADR-0005 already limits the local micro model
to stalled loops, and the same limit applies here until an A/B run shows a wider use is worth its cost.

## Decision

1. Jev decides stalled loops only, the same three choices as the micro model: ESCALATE, ASK_USER and REPLAN. Every other
   case, and every failure, is decided by the rules.
2. The answer is checked strictly. A decision outside the choices, a missing confidence, or a confidence outside 0 to 1 is
   rejected, and the rules decide. The confidence that Jev gives is kept.
3. The endpoint policy is validation rules V30 to V35. Jev is off by default. An enabled endpoint is required. `http://` is
   allowed only for a loopback host. A remote endpoint must use `https://` and name an environment variable for its key.
   A URL with a user name or password is rejected. The timeout must be between 100 ms and 60 s.
4. The key is read from the environment, sent only as a bearer header, and never appears in a decision, an error or a
   debug print.
5. Remote requests use the proxy that the environment names (`HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`), as the rest of the
   machine does, and the certificates are checked with the system store through native-tls. Loopback requests never use a
   proxy. The config names the endpoint; the environment names the route.
6. The TLS backend is native-tls, not rustls. With rustls the TLS handshake stalled on the target machine, while curl
   reached the same host through the environment proxy. The system store also keeps a corporate root certificate
   trusted, which the bundled Mozilla roots would not.

## Consequences

- The Jev contract (the request body, the path `/v1/decisions`, and the answer fields) is this project's own. It is
  tested against a local mock of that contract. It has not been checked against a live Jev service, which was not
  available. The contract must be confirmed against the real service before Jev is turned on.
- With Jev enabled, the loop summary leaves the machine: the phase, the counters, the last action (capped at 120
  characters) and the signal names. The enable switch is the consent. See the residual risk in the security page.
- Linux builds need OpenSSL development files for native-tls. Release 1 is Windows-only; the Linux CI job will install
  them.
- Jev is used only when the config selects it, so the local-only profile does not change.

## Alternatives considered

- Jev for every decision: rejected. It is an external call on every step, and no evidence supports that yet.
- rustls with the bundled roots: rejected after the TLS stall, and because it would not trust a corporate root.
- A proxy setting in `[external.jev]`: not added. The environment already names the route, and a second setting could
  disagree with it.

## References

- [ADR-0005: the micro model decides stalled loops only](0005-micro-model-decides-stalled-loops-only.md)
