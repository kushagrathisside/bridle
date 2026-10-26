from pricing import compute_total


class Cart:
    def __init__(self):
        self.items = []

    def add(self, price):
        self.items.append(price)

    def checkout(self, tax_rate=0.2):
        return compute_total(self.items, tax_rate)
