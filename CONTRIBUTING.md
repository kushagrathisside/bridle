# Contributing

Thank you for looking at Bridle. This page lists what to read, what to run and how to submit a change.

## Before you change code

Read [architecture](docs/concepts/architecture.md) and [security](docs/concepts/security.md). Two rules shape the code:

- The policy lives in `crates/harness-core`, which has no network code and no knowledge of OpenCode.
- Code that reads OpenCode's payloads lives only in `crates/harness-cli/src/host/`.

## Checks

Run the same checks as CI before you open a change:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The OpenCode shim is type-checked against its pinned plugin package:

```bash
cd opencode-plugin
npm ci
npm run typecheck
```

If you add or change a dependency, regenerate the third-party licence list with `python3 scripts/third_party_licenses.py`.
CI fails when the list differs from the committed `THIRD_PARTY_LICENSES.md`.

## Adversarial cases

`crates/harness-cli/tests/adversarial.rs` runs every case in `evals/security/cases.json`. When you find a new hostile
command shape, add it to that file with the verdict it must receive.

## Documentation

- When a configuration key or a verdict changes, update the [configuration reference](docs/guide/configuration.md).
- When code in a crate changes, update that crate's page in `docs/reference/modules/`.
- When a verified result changes, update `docs/project/status.md`.

To build the documentation site, use Node.js 22.23.4 or later:

```bash
cd site
npm ci
npm run build
```

The build fails on a broken link, so check the links you add.

## Commits and code

- Make small commits with one purpose each. The message says what changed and why.
- Code comments describe the code. Do not add internal labels or references to code, commits or public docs.
- Do not commit model weights, `.env` files or credentials. CI fails when a model weight is tracked.

## Licence

By contributing, you agree that your contribution is licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
