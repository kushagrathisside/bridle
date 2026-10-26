# M9: Security threat tests and egress audit

M9 checks the policy against hostile tool calls, fixes what the check finds, and lists every place a request can leave the
machine. The rules themselves are in [security.md](../../concepts/security.md).

> **Status:** Built and tested (2026-10-27), committed as `5bef0f9`, with this page committed after it. Gate green: 669 tests
> pass, 3 are ignored. One residual limit is known and kept (below).

## What was done

1. **A hostile run.** Seventy tool calls were sent through the real `agent hook tool-before`: secret reads by every name form,
   every shell reader found in the command classifier, network uploads, destructive commands, writes to the secret file,
   and ordinary calls that must still pass. The set is `evals/security/cases.json`. It is now part of the test suite.
2. **Five gaps found and fixed.**
   - A bash escape hid a secret name. `cat .en\v` reads `.env`. The shell word is now also judged with its escapes removed.
   - A Windows short name could address a secret without naming it (`ENV~1`). A path with `~` and a digit is now judged as a
     possible secret.
   - A glob that could name a secret (`.env*`) was allowed. Glob patterns and grep include filters are judged for secret
     names. A grep pattern is a regular expression, so it is not judged as a path.
   - The environment could be dumped (`env`, `printenv`, a bare `set`, `export -p`, PowerShell `Env:` listings, the .NET
     call). These now give the rule `secret_env`. `set -e` is still allowed.
   - The secret check ran after the path guard, so a glob was reported as a symlink escape. The secret check now runs first.
3. **One usability defect found and fixed.** The redactor hid the first word of validation messages: the path
   `models.<slot>.api_key_env:` was read as a key with a value. The separator in the output is now a dash.
4. **Two existing expectations updated** where the new order of checks gives a more accurate rule (the glob case in the hook
   fixtures, and the glob tests).
5. **Egress audit.** `agent egress` prints every outbound destination of the configuration: the model slots, the Jev service
   when it is on, whether each is loopback, and when it is used. It also says whether a proxy is set, without printing its
   value. It sends nothing.

## Result

| Measure | Result |
|---|---|
| Hostile cases | 72 (70 from the run, plus two network tools) |
| Cases that get the expected verdict | 72 of 72 |
| Gaps found and fixed | 5 (see above) |
| Residual | 1: a recursive grep over the workspace can read a secret file without naming it (security.md, residual risk 1) |
| Redaction | telemetry redaction already covers the PEM blocks, the authorization headers, the bearer tokens, URL passwords, and the event log (R-27.3, existing tests) |

## Egress audit

| Destination | Set by | Loopback in the shipped profiles | Condition |
|---|---|---|---|
| Model slots (`main`, `micro`, `deep`) | `models.<slot>.endpoint` | yes | every request to the slot |
| LiteLLM gateway (`hybrid` profile) | `models.gateway.endpoint` | yes | every request to the slot (not selected by any rule) |
| Jev decision service | `external.jev.endpoint` | not set | stalled loops, when `decisions.provider = "jev"` (off in every shipped profile) |
| OpenCode model catalog (models.dev) | OpenCode, at startup | not controlled | at startup |
| Model weights | the install step | not controlled | once |
| webfetch and websearch tools | OpenCode | denied by the policy | never (tested) |

The proxy (`HTTPS_PROXY`, `HTTP_PROXY`) is used only by remote destinations, and loopback never uses it. Check the list for
any configuration with `agent egress`.

## Tests

- `harness-core/src/security/tool_policy.rs`: escaped words, short names, the environment dumps and their exceptions, and the
  secret-before-path order. Each has a test of its own.
- `harness-cli/tests/adversarial.rs`: every case in `evals/security/cases.json` through the real hook, plus one ordinary read.
- `harness-cli/src/commands/egress.rs`: the shipped local profile reaches only loopback; a remote Jev endpoint is listed with
  its condition.

## Limits and what remains

- The recursive grep residual is unchanged. Closing it would block searches of the whole workspace, which the agent needs.
  Redaction of the output is the remaining protection, and a secret in an unrecognised format is not caught.
- The short-name rule is a heuristic. A legitimate file with `~` and a digit in its name is refused for reads.
- The environment-dump rule judges shell text. A command built at run time (`eval`, a script file) is not seen.
- The egress list is what the configuration declares. It does not see a connection made by a tool the model runs.

## Related

- [security.md](../../concepts/security.md)
- [Module: core-security](../../reference/modules/core-security.md)
- [Requirements register](../../reference/requirements-traceability.md): the M9 rows
