import importlib.util
import sys

spec = importlib.util.spec_from_file_location("mathutil", "mathutil.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
cases = {(1, 3): 6, (5, 5): 5, (2, 4): 9, (0, 0): 0, (-2, 2): 0}
bad = [(args, want, module.range_sum(*args)) for args, want in cases.items() if module.range_sum(*args) != want]
if bad:
    print("wrong:", bad)
    sys.exit(1)
print("ok")
