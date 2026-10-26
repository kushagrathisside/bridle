from .store import Store


def summary(store):
    return sorted(store.stock.items())
