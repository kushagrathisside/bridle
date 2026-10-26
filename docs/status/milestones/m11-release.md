# M11: open-source release preparation

M11 prepares the repository for release: the licence, the notices, the policies, the continuous integration, and the
third-party licence list (R-52, R-53, R-57.11, AC-R1 to AC-R4). It does not publish. Publication waits for the maintainer.

> **Status:** Prepared. The licence, notices, policies, the licence list and the workflow are written. The workflow has
> never run, because the repository has not been pushed since M6. Publication has not been done.

## What is built

- `LICENSE`: Apache-2.0, copied from the licence text shipped with a vendored crate.
- `NOTICE`: the copyright line, and where the third-party terms are.
- `THIRD_PARTY_LICENSES.md`: 160 crates with their declared licence expressions, from `scripts/third_party_licenses.py`
  (reads `Cargo.lock` with `cargo metadata --locked`). One item needs attention at release: `webpki-root-certs` carries
  CDLA-Permissive-2.0, a data licence, and is reached through ureq's native-tls feature. Nothing in the list is a copyleft
  licence. The script does not decide compatibility; a person reviews the list at each release.
- `CHANGELOG.md`, `SECURITY.md` (private reporting through GitHub, no personal address), `CONTRIBUTING.md` (the gate, and the
  documentation rule).
- `.github/workflows/ci.yml`: Windows and Ubuntu jobs (format, clippy with warnings denied, tests with `--locked`), a shim
  typecheck job, and a repository job that refuses tracked model weights and an out-of-date licence list.
- The licence is declared once in the workspace manifest, and each crate inherits it. `publish = false` stays.

## Not done

- **The CI workflow has not run.** It runs only on GitHub, after a push. The Ubuntu job needs OpenSSL headers, which the
  workflow installs; that is not verified either.
- **Publication** (crates.io, a GitHub release, a tag) is not done. It needs the maintainer's approval.
- The versioning and the release notes beyond the changelog are not written.

## Related

- [Checklist](../checklist.md): rows M11-1 to M11-6
- [Requirements register](../../reference/requirements-traceability.md)
