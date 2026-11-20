# Security policy

## Reporting a vulnerability

Report security problems privately through GitHub's private vulnerability reporting for this repository (the Security tab,
then "Report a vulnerability"). Do not open a public issue for a vulnerability.

Include the commit you tested, the configuration profile, the tool call or command involved, and what you expected. Do
not include real secret values. A placeholder that shows the shape is enough.

## Scope

The threat model and the limits are in [security](docs/concepts/security.md). Bridle is not a sandbox: a test run that the
agent allows executes code the model wrote, with your user rights. Reports about the limits listed there are welcome, but
they are not treated as new findings.

## Supported versions

Only the latest release (`0.1.x`) and the current `main` branch are maintained.
