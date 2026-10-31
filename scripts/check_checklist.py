#!/usr/bin/env python3
"""Checks the project checklist against the code.

Every row of docs/status/checklist.md has an id, a stage, an item, a status and evidence. For a row marked `built` or
`partial` this script verifies the evidence exists:

- every file in "Built in" exists;
- every name in "Tests" is a test function defined somewhere under crates/;
- the "Plugged into" entry, `file::symbol`, names a symbol that appears in that file. This is the check that a built
  piece is reachable from an entry point (a command, a tool list, a hook, a provider, a validation rule).

A row marked `open` needs no evidence and is counted. It also reports public items that nothing outside their own file
uses: a candidate for code that was built and never plugged in. Those are warnings for review, not failures.

Run from the repository root: python3 scripts/check_checklist.py    (exit 1 when a built row lacks evidence)
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHECKLIST = ROOT / "docs" / "status" / "checklist.md"
SOURCE_GLOBS = ["crates/**/*.rs", "scripts/*.py", "opencode-plugin/*.ts", ".github/workflows/*.yml"]


def sources():
    files = []
    for pattern in SOURCE_GLOBS:
        files.extend(ROOT.glob(pattern))
    return files


def read(path):
    return path.read_text(encoding="utf-8", errors="replace")


def rows():
    """The checklist table rows as dicts, keyed by the header names."""
    lines = CHECKLIST.read_text(encoding="utf-8").splitlines()
    header = None
    for line in lines:
        if not line.startswith("|"):
            header = None
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if header is None:
            header = [h.lower() for h in cells]
            continue
        if set("".join(cells)) <= set("-: "):
            continue
        yield dict(zip(header, cells))


def main() -> int:
    all_text = {p: read(p) for p in sources()}
    test_fns = set()
    for path, text in all_text.items():
        if path.suffix == ".rs" and "crates" in path.parts:
            test_fns.update(re.findall(r"fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(", text))

    failures = []
    counts = {"built": 0, "partial": 0, "open": 0}
    for row in rows():
        rid = row.get("id", "?")
        status = row.get("status", "").lower()
        counts[status] = counts.get(status, 0) + 1
        if status not in ("built", "partial"):
            continue
        for path in [p.strip() for p in row.get("built in", "").split(",") if p.strip()]:
            if not (ROOT / path).exists():
                failures.append(f"{rid}: built-in file is missing: {path}")
        for name in [t.strip() for t in row.get("tests", "").split(",") if t.strip()]:
            if name not in test_fns:
                failures.append(f"{rid}: test function not found: {name}")
        plug = row.get("plugged into", "").strip().strip("`")
        if "::" not in plug:
            failures.append(f"{rid}: 'Plugged into' must be file::symbol, got '{plug}'")
            continue
        file_part, symbol = plug.split("::", 1)
        target = ROOT / file_part
        if not target.exists():
            failures.append(f"{rid}: plug file is missing: {file_part}")
        elif symbol not in read(target):
            failures.append(f"{rid}: plug symbol `{symbol}` does not appear in {file_part}")

    # A public item with no use in production code, neither in its own file nor in any other file, is used only by
    # tests or not at all: a candidate for built-but-unplugged. Test modules are not counted.
    production = {p: t.split("#[cfg(test)]\nmod ")[0] for p, t in all_text.items()}
    unused = []
    for path, text in all_text.items():
        if path.suffix != ".rs" or "crates" not in path.parts or "src" not in path.parts:
            continue
        for name in set(re.findall(r"pub (?:fn|struct|enum|const) ([A-Za-z_][A-Za-z0-9_]*)", text)):
            if name in ("new", "default", "run", "name", "description", "input_schema", "call"):
                continue
            pattern = r"\b%s\b" % re.escape(name)
            defined_here = len(re.findall(pattern, production[path]))
            used_elsewhere = [p for p, t in production.items() if p != path and re.search(pattern, t)]
            if not used_elsewhere and defined_here <= 1:
                unused.append(f"{path.relative_to(ROOT)}: `{name}`")

    print(f"checklist rows: built {counts.get('built', 0)}, partial {counts.get('partial', 0)}, open {counts.get('open', 0)}")
    if unused:
        print(f"\n{len(unused)} public item(s) used nowhere outside their own file (review: built but unplugged?):")
        for line in sorted(unused):
            print("  " + line)
    if failures:
        print(f"\n{len(failures)} failure(s):")
        for line in failures:
            print("  " + line)
        return 1
    print("\nall built and partial rows have their evidence")
    return 0


if __name__ == "__main__":
    sys.exit(main())
