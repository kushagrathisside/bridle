import subprocess
import sys
from pathlib import Path

for path in Path(".").glob("*.py"):
    if "compute_total" in path.read_text():
        print("the old name is still in", path)
        sys.exit(1)
if "def calculate_total" not in Path("pricing.py").read_text():
    print("pricing.py does not define calculate_total")
    sys.exit(1)
sys.exit(subprocess.run([sys.executable, "-m", "unittest", "-q"]).returncode)
