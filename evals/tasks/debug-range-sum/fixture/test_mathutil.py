import unittest
from mathutil import range_sum


class RangeSumTests(unittest.TestCase):
    def test_inclusive(self):
        self.assertEqual(range_sum(1, 3), 6)


if __name__ == "__main__":
    unittest.main()
