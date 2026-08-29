"""検索ボタンの描画。"""


def render_search_button(label: str = "検索") -> str:
    """画面上部の検索ボタンを返す。"""
    return f"<button>{label}</button>"
