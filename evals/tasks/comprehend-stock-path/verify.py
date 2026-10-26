import sys
from pathlib import Path

path = Path("answer.txt")
if not path.is_file() or path.read_text().strip() != "apply_delta":
    print("answer.txt does not hold apply_delta")
    sys.exit(1)
