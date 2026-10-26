import unittest
from geometry import area, perimeter


class GeometryTests(unittest.TestCase):
    def test_area(self):
        self.assertEqual(area(2, 3), 6)

    def test_perimeter(self):
        self.assertEqual(perimeter(2, 3), 11)


if __name__ == "__main__":
    unittest.main()
