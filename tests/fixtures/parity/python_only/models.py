class User:
    def __init__(self, name: str, email: str) -> None:
        self.name = name
        self.email = email


class Address:
    def __init__(self, street: str, city: str) -> None:
        self.street = street
        self.city = city
