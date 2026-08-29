"""ハイブリッド検索の入口。"""


def hybrid_search(store, query: str) -> dict:
    """GraphStoreで自然言語検索を行う。"""
    return {"results": [], "query": query}


def rebuild_fts_index(store) -> int:
    """FTS 索引を再構築する。"""
    return 0
