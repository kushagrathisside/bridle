import hashlib
import subprocess
import sys
from pathlib import Path

reference = Path(__file__).resolve().parent / "fixture" / "geometry.py"
same = hashlib.sha256(reference.read_bytes()).hexdigest() == hashlib.sha256(Path("geometry.py").read_bytes()).hexdigest()
if not same:
    print("geometry.py was changed")
    sys.exit(1)
result = subprocess.run([sys.executable, "-m", "unittest", "-q", "test_geometry"])
sys.exit(result.returncode)
