from services import create_user


def run() -> None:
    user = create_user("Alice", "alice@example.com")
    print(user.name)
