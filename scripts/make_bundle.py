#!/usr/bin/env python3
r"""Builds the Windows release bundle: a zip with bridle.exe, the installer, a quick start, the licences and checksums.

    python scripts\make_bundle.py [--exe target\release\bridle.exe] [--out %USERPROFILE%\Downloads]

The version comes from `[workspace.package]` in Cargo.toml. The zip holds one folder, `bridle-<version>-windows-x64`, with:
bridle.exe, install.cmd (the installer, which finds bridle.exe next to itself), README.txt, MANUAL.txt (the instruction manual),
SHA256SUMS.txt, LICENSE, NOTICE,
THIRD_PARTY_LICENSES.md, CHANGELOG.md, docs\ (the web research and browser guides and their decision records) and licenses\
(Monaco and xterm.js, which are built into the exe). A `.zip.sha256` file
is written next to the zip. Build the exe first (`scripts\dev.cmd release`). Nothing here talks to the network.
"""
import argparse
import hashlib
import os
import re
import shutil
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

README = """Bridle {version} for Windows (x64)
==============================

Bridle is a local-first coding agent. `bridle` opens a window on a folder: a chat with a local model, a code editor with
diffs, search, a terminal, and image input, all under one policy that decides what the model may do. Run `bridle COMMAND`
(for example `bridle doctor`) to use the same program from the terminal.

What you need
-------------
  * Windows 11 (the WebView2 runtime is included with it).
  * An NVIDIA GPU with a current driver, for the local models. The shipped profile is tuned for 8 GB of VRAM.
  * Model weights and the llama.cpp runtime. They are NOT in this bundle. Get them yourself, check the SHA-256 values in the
    project's docs (guide/models.md), and put them in two folders, for example D:\\models and D:\\runtimes.
  * Git for Windows is recommended: its Git Bash is the terminal's default shell.

The full instruction manual is MANUAL.txt in this folder: install, first run, the window, the team and board, configuration,
web research, the browser, every command, troubleshooting and uninstall.

Install
-------
  1. Unpack this folder anywhere.
  2. Run install.cmd. It copies bridle.exe to %LOCALAPPDATA%\\Bridle\\bin and adds that folder to your user PATH.
     (To undo it, delete that folder and remove it from your user PATH.)
  3. Open a new terminal, go to a project folder and run:   bridle
  4. The first time, Bridle asks where your models and runtimes are, and whether to set up the folder (it writes bridle.toml
     and a .bridle/ state folder, and adds .bridle/ to .gitignore).

If your network only lets traffic out through a proxy, set it in the window: Info, then Network. Your local model servers never
use a proxy.

Optional: web research and the browser
--------------------------------------
Both are OFF by default and are switched on in your machine configuration, not in a project. See docs\\web-research.md and
docs\\web-browser.md in this folder. The managed browser (Chromium) is NOT in this bundle: only if you run
`bridle web browser setup` does Bridle download Node.js, playwright-core and Chromium (about 210 MB, 560 MB on disk), check each
against its hash, and install them next to your configuration. `bridle web browser test` checks the install without the internet and
`bridle web browser remove` deletes it. The model gets no browser tools unless you also set security.network = "allow".

Check the download
------------------
  certutil -hashfile bridle.exe SHA256       (compare with SHA256SUMS.txt)

Known limits ({version})
--------------------
  * Windows only; checked on one machine (RTX 4060 8 GB, 16 GB RAM). The executable is not code-signed, so Windows
    SmartScreen may warn the first time.
  * One conversation is open at a time, and images are limited to 4 MiB (PNG, JPEG, GIF, WebP).
  * `bridle > file.txt` or `bridle | more` keeps the pipe or file open until the window closes.
  * The browser and the search providers were checked on that one machine and one company proxy. The providers have never talked to
    the real services (docs\\web-research.md says what was and was not verified).
  * No installer program and no automatic update. See CHANGELOG.md for what is in this version.

Licences: LICENSE (Apache-2.0), NOTICE, THIRD_PARTY_LICENSES.md, and licenses\\ for the editor (Monaco, MIT) and the terminal
(xterm.js, MIT) that are built into bridle.exe. The browser runtime that `bridle web browser setup` downloads is not part of
this bundle: Node.js is MIT, playwright-core is Apache-2.0, and Chromium carries the Chromium Project's BSD-style licences.
"""


def version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'\[workspace\.package\][^\[]*?\nversion\s*=\s*"([^"]+)"', text, re.S)
    if not match:
        sys.exit("cannot read the version from Cargo.toml")
    return match.group(1)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", default=str(ROOT / "target" / "release" / "bridle.exe"))
    parser.add_argument("--out", default=str(Path(os.environ.get("USERPROFILE", str(Path.home()))) / "Downloads"))
    args = parser.parse_args()
    exe, out = Path(args.exe), Path(args.out)
    if not exe.is_file():
        sys.exit(f"not found: {exe} (build it with scripts\\dev.cmd release)")
    out.mkdir(parents=True, exist_ok=True)
    ver = version()
    name = f"bridle-{ver}-windows-x64"
    stage = out / f".{name}-stage"
    if stage.exists():
        shutil.rmtree(stage)
    (stage / "licenses").mkdir(parents=True)
    (stage / "docs").mkdir()
    shutil.copy(exe, stage / "bridle.exe")
    shutil.copy(ROOT / "scripts" / "install-bridle.cmd", stage / "install.cmd")
    for item in ("LICENSE", "NOTICE", "THIRD_PARTY_LICENSES.md", "CHANGELOG.md"):
        shutil.copy(ROOT / item, stage / item)
    for doc in (
        "docs/guide/web-research.md",
        "docs/guide/web-browser.md",
        "docs/decisions/0019-web-research-is-policy-gated-native-tools.md",
        "docs/decisions/0020-managed-browser-is-a-policy-gated-playwright-worker.md",
    ):
        shutil.copy(ROOT / doc, stage / "docs" / Path(doc).name)
    vendor = ROOT / "crates" / "bridle-desktop" / "ui" / "vendor"
    shutil.copy(vendor / "monaco" / "LICENSE", stage / "licenses" / "monaco-editor-LICENSE.txt")
    shutil.copy(vendor / "monaco" / "ThirdPartyNotices.txt", stage / "licenses" / "monaco-editor-ThirdPartyNotices.txt")
    shutil.copy(vendor / "xterm" / "LICENSE", stage / "licenses" / "xterm.js-LICENSE.txt")
    (stage / "README.txt").write_text(README.format(version=ver), encoding="utf-8", newline="\r\n")
    manual = (ROOT / "docs" / "MANUAL.txt").read_text(encoding="utf-8")
    (stage / "MANUAL.txt").write_text(manual, encoding="utf-8", newline="\r\n")
    (stage / "SHA256SUMS.txt").write_text(f"{sha256(stage / 'bridle.exe')}  bridle.exe\n", encoding="utf-8", newline="\r\n")
    archive = out / f"{name}.zip"
    if archive.exists():
        archive.unlink()
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
        for base, _, files in os.walk(stage):
            for file in sorted(files):
                full = Path(base) / file
                bundle.write(full, Path(name) / full.relative_to(stage))
    (out / f"{name}.zip.sha256").write_text(f"{sha256(archive)}  {archive.name}\n", encoding="utf-8", newline="\r\n")
    shutil.rmtree(stage)
    print(f"wrote {archive} ({archive.stat().st_size} bytes)")
    print(f"sha256 {sha256(archive)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
