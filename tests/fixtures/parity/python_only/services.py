from models import User


def create_user(name: str, email: str) -> User:
    return User(name, email)


def get_email(user: User) -> str:
    return user.email
