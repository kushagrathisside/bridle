from .ledger import apply_delta as _apply_delta


class Store:
    def __init__(self):
        self.stock = {}

    def restock(self, sku, amount):
        self._adjust(sku, amount)

    def _adjust(self, sku, amount):
        self.stock[sku] = _apply_delta(self.stock.get(sku, 0), amount)
