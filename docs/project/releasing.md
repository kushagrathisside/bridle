# Releasing Bridle

This page is the whole release process, written so that one person can do it by hand, in order, with nothing left to memory. Every
command is run from the repository root on Windows 11 unless it says otherwise. Nothing here needs WSL for the Rust program.

A release is four things: **a version**, **a verified build**, **a bundle** (a zip with a checksum), and **a published record** (a Git tag, a
GitHub release, and the documentation site). Do them in that order. Stop at the first step that fails; do not skip a check to keep going.

| Step | What | Where it is described |
|---|---|---|
| 1 | Decide the version and update it everywhere | [Version](#1-decide-and-set-the-version) |
| 2 | Update the changelog, the README status and the progress notes | [Words](#2-update-the-words) |
| 3 | Run the gate | [Gate](#3-run-the-gate) |
| 4 | Run the checks that need the real machine | [Real checks](#4-run-the-checks-that-need-the-real-machine) |
| 5 | Build the release executable | [Build](#5-build-the-release-executable) |
| 6 | Make the bundle and its checksums | [Bundle](#6-make-the-bundle) |
| 7 | Test the bundle as a stranger would | [Test the bundle](#7-test-the-bundle-as-a-new-user) |
| 8 | Tag, push and publish the GitHub release | [Publish](#8-tag-push-and-publish) |
| 9 | Build and publish the documentation site | [Site](#9-publish-the-documentation-site) |
| 10 | After the release | [Afterwards](#10-after-the-release) |

## What you need on the release machine

- Windows 11 with the Visual Studio 2022 C++ build tools, the Rust version pinned in `rust-toolchain.toml`, Git for Windows and Python 3
  (`py -3`).
- The vendored dependencies (`D:\cargo-vendor\bridle-desktop` on the tested machine) and the source replacement file named by
  `BRIDLE_VENDOR_CONFIG`. Builds run `--offline`; see [Contributing](../../CONTRIBUTING.md).
- For step 4: an NVIDIA GPU, the model weights and the llama.cpp runtime described in [Models](../guide/models.md).
- For steps 8 and 9: the GitHub CLI (`gh`), signed in with a token that can create releases, and Node.js 22.23.4 or newer for the site.

Set these once per shell (the defaults are the tested machine's; change them to yours):

```bat
set CARGO_TARGET_DIR=D:\cargo-target\harness
set BRIDLE_VENDOR_CONFIG=D:\cargo-vendor\bridle-desktop-source.toml
```

`scripts\dev.cmd` clears the proxy variables, sets `CARGO_BUILD_JOBS=4` and loads the Visual Studio environment for you. Run **one build
at a time**: the tested machine has 16 GB of memory and a parallel build starves it.

## 1. Decide and set the version

Bridle follows `MAJOR.MINOR.PATCH`. While it is `0.x`, a new feature raises the minor number and a fix raises the patch number.

The version lives in two places, and they must match:

1. `Cargo.toml`, under `[workspace.package]`: `version = "0.1.0"`. Every crate and `make_bundle.py` read it.
2. `crates/bridle-desktop/tauri.conf.json`: `"version": "0.1.0"`. The window reads it.

Change both, then rebuild once so `Cargo.lock` updates, and check that nothing else changed:

```bat
cargo --config %BRIDLE_VENDOR_CONFIG% check --workspace --offline
git diff --stat
```

`git diff --stat` should show `Cargo.toml`, `Cargo.lock` and `tauri.conf.json` only.

## 2. Update the words

1. **`CHANGELOG.md`.** Rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and add a new empty `## [Unreleased]` above it. Keep the
   `Added`, `Changed`, `Fixed` and `Known limits` headings. Write each entry as what a user can now do or what is now different, and name
   the thing that is **not** verified. A limit that is true must stay in the list.
2. **`README.md`.** Update the `Status` paragraph at the top, the `Status and known limits` section and the `Roadmap`. The numbers in them
   (tests passed, machines checked) must be the ones from this release's gate and checks.
3. **`docs/status/progress.md`.** Add an entry with the date, the commit, the gate result and what was and was not verified.
4. **`docs/MANUAL.txt`.** Change the version at the top. If a command, a setting or a limit changed, change it here too: this file ships in
   the bundle and is what a user reads first.
5. **Third-party licences.** If any dependency changed, regenerate the list and commit it. CI fails if it is stale:

   ```bat
   py -3 scripts\third_party_licenses.py
   git diff THIRD_PARTY_LICENSES.md
   ```

6. **The checklist.** `py -3 scripts\check_checklist.py` verifies that every row of `docs/status/checklist.md` marked built has its files,
   tests and entry point. It must pass.

Commit these as one commit, for example `chore(release): 0.2.0`.

## 3. Run the gate

The gate is what CI runs, plus the same on this machine:

```bat
scripts\dev.cmd gate
```

It runs `cargo fmt --all -- --check`, then `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`, then the whole
workspace test suite (`--no-fail-fast`). **All three must pass with no warnings.** Write down the number of tests passed, failed and
ignored; it goes into the progress notes and the changelog. Ignored tests are ones that need the network, the GPU or a real window, and they
are run by hand in step 4.

If a test fails, do not re-run until it passes. Find out why. A flaky test is a bug to fix before releasing.

CI (`.github/workflows/ci.yml`) also runs on every push, on Windows and Linux: formatting, clippy, tests, the type check of the OpenCode shim
(`opencode-plugin`), a check that no model weights are tracked, and a check that the third-party licence list is current. Wait for it to be
green on the release commit.

## 4. Run the checks that need the real machine

These cannot run in CI. Do each one and record the result.

**a. The end-to-end suite on the real window** (needs the GPU model; `--skip-model` runs the parts that do not):

```bat
scripts\dev.cmd e2e
```

It builds the debug program, opens the real window and drives it. Then check the GPU is free: `nvidia-smi` must show 0 MiB used. Stop
any model you started with `bridle models stop <slot>` (never with a task killer).

**b. The health of each model slot you ship a profile for:**

```bat
bridle doctor
bridle health main
bridle health micro
```

Each must end with `result: pass`.

**c. The security suites:** `cargo test -p harness-cli --test adversarial` and the policy tests are part of the gate; confirm they ran.

**d. The managed browser** (only if the browser code or its pinned runtime changed):

```bat
bridle web browser setup
bridle web browser test
bridle web browser test --live https://example.com
bridle web browser remove
```

The self-test must pass with the number of checks the previous release reported or more. Re-check the pinned hashes in
`crates/harness-runtime/src/web/browser/runtime.rs` against a fresh install; see [the browser guide](../guide/web-browser.md).

**e. The fleet and lanes** (only if they changed): `bridle fleet status`, then `bridle fleet run` on a small task file; `bridle lanes` on a
scratch Git repository (`new`, `diff`, `integrate --dry-run`, `integrate`, `undo`, `remove`). Model evaluations
(`bridle eval-agent notices`, `plan`, `lanes`) are optional and are recorded in the progress notes, not gated.

**f. A clean-folder run.** In an empty folder run `bridle init`, `bridle config validate` and `bridle doctor` with a copy of the machine
configuration that has no private paths in it, and confirm every message is understandable.

## 5. Build the release executable

```bat
scripts\dev.cmd release
```

This runs `cargo build --release -p bridle-desktop --locked --offline` and produces
`%CARGO_TARGET_DIR%\release\bridle.exe`. Check it starts:

```bat
%CARGO_TARGET_DIR%\release\bridle.exe --version
```

The output must show the version from step 1. The executable is **not code-signed**; Windows SmartScreen may warn on first run, and the
release notes say so.

## 6. Make the bundle

```bat
scripts\dev.cmd bundle D:\release-out
```

(or `py -3 scripts\make_bundle.py --exe %CARGO_TARGET_DIR%\release\bridle.exe --out D:\release-out` to reuse a release build). The script
reads the version from `Cargo.toml` and writes:

- `bridle-<version>-windows-x64.zip`, holding one folder `bridle-<version>-windows-x64` with `bridle.exe`, `install.cmd`, `README.txt`,
  `MANUAL.txt`, `SHA256SUMS.txt`, `LICENSE`, `NOTICE`, `THIRD_PARTY_LICENSES.md`, `CHANGELOG.md`, `docs\` (the web research and browser
  guides and their decision records) and `licenses\` (Monaco and xterm.js, built into the executable);
- `bridle-<version>-windows-x64.zip.sha256`, the checksum of the zip.

The script does not touch the network. Check the zip yourself:

```bat
certutil -hashfile D:\release-out\bridle-0.2.0-windows-x64.zip SHA256
type D:\release-out\bridle-0.2.0-windows-x64.zip.sha256
```

The two values must be equal. Open `README.txt` in the zip and read it once: its "Known limits" must match the README's.

Model weights, the llama.cpp runtime and the browser runtime are **never** in the bundle. The licences of what the user downloads
themselves are named in `README.txt`.

## 7. Test the bundle as a new user

On a clean Windows user account, or at least a clean folder and a fresh `PATH`:

1. Unpack the zip to a new folder, outside the repository.
2. `certutil -hashfile bridle.exe SHA256` and compare with `SHA256SUMS.txt`.
3. Run `install.cmd`. It copies `bridle.exe` to `%LOCALAPPDATA%\Bridle\bin` and adds that folder to the user `PATH`.
4. Open a **new** terminal in an empty project folder and run `bridle --version`, `bridle doctor`, then `bridle`. The window must open and
   ask where the models and runtimes are.
5. Follow `MANUAL.txt` from "First run" as written. Anything that is wrong or unclear in the manual is a release bug.
6. Uninstall as the manual says (delete the folder, remove it from the user `PATH`) and confirm `bridle` is no longer found.

## 8. Tag, push and publish

Only now publish anything. Publishing sends content outside the machine; take the release notes and file list from the work above.

```bat
git status                         :: must be clean
git tag -a v0.2.0 -m "Bridle 0.2.0"
git push origin main
git push origin v0.2.0
```

Then create the GitHub release, with the bundle, its checksum and the notes (the changelog section for this version):

```bat
gh release create v0.2.0 D:\release-out\bridle-0.2.0-windows-x64.zip D:\release-out\bridle-0.2.0-windows-x64.zip.sha256 ^
    --title "Bridle 0.2.0" --notes-file D:\release-out\notes-0.2.0.md
```

Write `notes-0.2.0.md` from the changelog entry. It must include: what is new, the supported platform (Windows 11 x64 only), the
checksum, that the executable is not code-signed, and the known limits. Do not add anything that was not verified.

Commit messages and release notes name the changes and nothing else.

## 9. Publish the documentation site

The site is Astro Starlight in `site/`. It builds from the Markdown in this repository; nothing in `site/src/content/docs/` is edited by
hand (it is generated and ignored by Git).

```bat
cd site
npm ci
npm run build
```

`npm run build` runs `node scripts/prepare-docs.mjs`, which copies each published page, rewrites its links and writes its front matter,
then `astro build`. The script **fails the build on a broken link**, which is the check you want. The result is `site\dist`. Preview it
with `npm run preview` and look at the home page, one guide, one reference page and the search before you publish.

Publishing: the repository's workflow `.github/workflows/site.yml` builds the site and deploys it to GitHub Pages on every push to `main`
that changes `site/`, `docs/` or the root Markdown files. Check the run in the Actions tab. (The workflow was written with this release process and has not yet run on GitHub; the first run is the test of it.) Pages must be set to "GitHub Actions" as its
source (Settings, Pages). By hand, upload `site\dist` as the Pages artifact. The address is set in `site/site.config.mjs`
(`SITE_URL` and `BASE`): change both if the site moves to a custom domain.

To add a page to the site, add its file to the `PAGES` list in `site/scripts/prepare-docs.mjs` and to the `sidebar` in
`site/astro.config.mjs`.

## 10. After the release

- Move any known limit that the release fixed out of the lists, and add the new ones the testing found.
- Open `docs/status/progress.md`, record the release commit, the tag and the bundle checksum.
- Keep the bundle and its checksum somewhere other than the release page, for example the release machine's archive folder.
- Start the next `## [Unreleased]` section in the changelog if you did not already.

## If something goes wrong

| Problem | What to do |
|---|---|
| The gate fails on the release commit | Fix it on a new commit. Never tag a commit whose gate failed. |
| A tag was pushed with a mistake | Delete the release and the tag (`gh release delete v0.2.0`, `git push origin :refs/tags/v0.2.0`, `git tag -d v0.2.0`), fix, and tag again with a new version if anyone could have downloaded it. |
| The bundle is wrong | Rebuild it from step 6; the zip is not edited by hand. |
| The site build reports a broken link | The link is wrong or the target page is not in `PAGES`. Fix the source Markdown, not the generated copy. |
| A model server is left running | `bridle models stop <slot>`, then check `nvidia-smi` shows 0 MiB. |
| A crash left leftovers (leases, temporary files, a stuck background command) | `bridle doctor --repair`. |

## Related

- [Contributing](../../CONTRIBUTING.md) for how to build and what a change must pass.
- [Changelog](../../CHANGELOG.md) and [status](status.md).
- [Installation](../guide/installation.md) for what a user does with the bundle.
