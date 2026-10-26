# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub's private vulnerability reporting for this repository
(Security tab, "Report a vulnerability"). Do not open a public issue for a vulnerability.

Include the version (commit), the configuration profile, the tool call or command involved, and what you expected. Do not
include real secret values; a placeholder that shows the shape is enough.

## Scope

The threat model and the residual risks are in `docs/concepts/security.md`. The harness is not a sandbox: an allowed test
run executes code the model wrote, with the user's rights. Reports about the residual risks listed there are still welcome,
but they are not treated as new findings.

## Supported versions

Only the current `main` branch is maintained until a first release is published.
