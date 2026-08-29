"""認可とトークン検証。"""


def verify_token(token: str) -> bool:
    """トークン検証を行う。"""
    return bool(token)


def ユーザー取得(user_id: str) -> dict:
    """CJK 識別子の関数。ソースが無くても名前で当たること。"""
    return {"id": user_id}
