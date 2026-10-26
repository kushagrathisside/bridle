import sys
from pathlib import Path

path = Path("summary.txt")
if not path.is_file():
    print("no summary.txt")
    sys.exit(1)
first = path.read_text().splitlines()[0].strip() if path.read_text().strip() else ""
if first != "42.7":
    print("first line is", repr(first), "expected 42.7")
    sys.exit(1)
