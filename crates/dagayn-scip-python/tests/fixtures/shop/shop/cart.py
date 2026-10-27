__all__ = ["Cart", "total"]


def total(prices: list[int]) -> int:
    subtotal = 0
    for price in prices:
        subtotal += price
    return subtotal


class Cart:
    def __init__(self) -> None:
        self.items: list[int] = []

    def add(self, price: int) -> "Cart":
        self.items.append(price)
        return self

    def summary(self) -> str:
        def line(amount: int) -> str:
            return f"{amount}"

        return line(total(self.items))
