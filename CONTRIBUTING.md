# Contributing

## Before you start

Read `docs/concepts/architecture.md` and `docs/concepts/security.md`. The architecture rule is that the policy lives in
`harness-core`, and the OpenCode-specific code lives only in `harness-cli/src/host/opencode_v1.rs`.

## The gate

Run these on Windows and on Linux before you open a change:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 scripts/third_party_licenses.py    # only when dependencies change
```

The adversarial suite (`crates/harness-cli/tests/adversarial.rs`) runs every case in `evals/security/cases.json`. A new
hostile shape goes into that file, with its expected verdict.

## Documentation

Each change updates the documentation it affects: the milestone page, the module page, the requirements register, and the
progress log. A change that alters a verdict or a configuration key also updates `docs/guide/configuration.md`.

## Commits

Small commits, one purpose each, with a message that says what changed and why. Do not commit model weights, `.env`
files or credentials; the repository check in CI looks for weights.

## Licence

By contributing you agree that your contribution is licensed under the Apache License, Version 2.0 (see `LICENSE`).
