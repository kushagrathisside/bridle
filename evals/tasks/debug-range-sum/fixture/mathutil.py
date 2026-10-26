def range_sum(lo, hi):
    """Sum of the integers from lo to hi, both included."""
    total = 0
    for value in range(lo, hi):
        total += value
    return total
