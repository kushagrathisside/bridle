import subprocess
import sys
from pathlib import Path

sys.path.insert(0, ".")
try:
    from fizzbuzz import fizzbuzz
except ImportError as error:
    print("cannot import fizzbuzz:", error)
    sys.exit(1)
expected = ["1", "2", "Fizz", "4", "Buzz", "Fizz", "7", "8", "Fizz", "Buzz", "11", "Fizz", "13", "14", "FizzBuzz"]
if fizzbuzz(15) != expected:
    print("fizzbuzz(15) is", fizzbuzz(15))
    sys.exit(1)
if not Path("test_fizzbuzz.py").is_file():
    print("no test_fizzbuzz.py")
    sys.exit(1)
sys.exit(subprocess.run([sys.executable, "-m", "unittest", "-q"]).returncode)
