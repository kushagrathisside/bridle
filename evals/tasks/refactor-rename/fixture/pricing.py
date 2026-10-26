def compute_total(prices, tax_rate):
    subtotal = sum(prices)
    return round(subtotal * (1 + tax_rate), 2)
