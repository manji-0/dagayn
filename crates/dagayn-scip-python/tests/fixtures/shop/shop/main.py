from pydantic import BaseModel

from shop import Cart, total

try:
    import json as codec
except ImportError:
    import pickle as codec


class Order(BaseModel):
    count: int


def checkout(cart: Cart) -> int:
    label = "\U0001f6d2"; count = total(prices=cart.add(3).items)
    order = Order(count=count)
    order.model_dump()
    codec.dumps(label)
    return len(cart.items)
