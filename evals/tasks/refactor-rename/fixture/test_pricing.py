import unittest
from pricing import compute_total
from cart import Cart


class PricingTests(unittest.TestCase):
    def test_total_with_tax(self):
        self.assertEqual(compute_total([10, 20], 0.5), 45.0)

    def test_cart_checkout(self):
        cart = Cart()
        cart.add(10)
        self.assertEqual(cart.checkout(0.0), 10.0)


if __name__ == "__main__":
    unittest.main()
