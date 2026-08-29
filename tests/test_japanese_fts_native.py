"""Native GraphStore Japanese FTS quality gates (Lindera + covering bigrams)."""

from __future__ import annotations

from pathlib import Path

import pytest

from dagayn.parser import NodeInfo

pytest.importorskip("dagayn._core")


def _rust_store(tmp_path: Path):
    from dagayn._core import GraphStore as RustGraphStore

    return RustGraphStore(tmp_path / "graph.db")


def _seed_japanese_corpus(store, root: Path) -> None:
    files = {
        "nlp.md": "# 自然言語検索\n\nGraphStoreで自然言語検索を行う。\n",
        "ui.md": "# 検索ボタン\n\n画面上部の検索ボタンを押すと結果一覧を開く。\n",
        "ops.md": "# 運用検索\n\n運用チームがログを検索して障害を切り分ける。\n",
        "auth.py": "def verify_token(token: str) -> bool:\n    return True\n",
        "billing.py": "def run_billing_batch() -> None:\n    pass\n",
        "prose.md": (
            "# 長い説明\n\nこの文書は自然言語検索の背景を、助詞や接続を含めて長く書いたものです。\n"
        ),
    }
    for name, body in files.items():
        (root / name).write_text(body, encoding="utf-8")
    store.set_metadata("repo_root", str(root))
    nodes = [
        NodeInfo(
            kind="DocSection",
            name="nlp-search",
            file_path="nlp.md",
            line_start=1,
            line_end=1,
            language="markdown",
            extra={"display_name": "自然言語検索"},
        ),
        NodeInfo(
            kind="DocSection",
            name="search-button",
            file_path="ui.md",
            line_start=1,
            line_end=1,
            language="markdown",
            extra={"display_name": "検索ボタン"},
        ),
        NodeInfo(
            kind="DocSection",
            name="ops-search",
            file_path="ops.md",
            line_start=1,
            line_end=1,
            language="markdown",
            extra={"display_name": "運用検索"},
        ),
        NodeInfo(
            kind="Function",
            name="verify_token",
            file_path="auth.py",
            line_start=1,
            line_end=8,
            language="python",
            extra={"display_name": "トークン検証"},
        ),
        NodeInfo(
            kind="Function",
            name="run_billing_batch",
            file_path="billing.py",
            line_start=1,
            line_end=6,
            language="python",
            extra={"display_name": "課金バッチ"},
        ),
        NodeInfo(
            kind="Function",
            name="ユーザー取得",
            file_path="jp.py",
            line_start=1,
            line_end=5,
            language="python",
        ),
        NodeInfo(
            kind="DocSection",
            name="long-prose",
            file_path="prose.md",
            line_start=1,
            line_end=1,
            language="markdown",
            extra={"display_name": "長い説明"},
        ),
    ]
    for node in nodes:
        store.upsert_node(node, file_hash="h")
    store.commit()
    store.rebuild_fts_index()


def _names(store, query: str) -> tuple[list[str], str]:
    result = store.fts_query(query, 10)
    ids = [node_id for node_id, _ in result.hits]
    by_id = store.get_nodes_by_ids(ids)
    return [by_id[node_id].name for node_id in ids if node_id in by_id], result.match_mode


def test_native_japanese_fts_quality_gates(tmp_path):
    store = _rust_store(tmp_path)
    try:
        _seed_japanese_corpus(store, tmp_path)
        assert store.get_metadata("fts_segmenter") == "lindera"

        hit1 = [
            ("自然言語検索", "nlp-search", None),
            ("GraphStore 自然言語検索", "nlp-search", None),
            ("検索ボタン", "search-button", None),
            ("トークン検証", "verify_token", None),
            ("verify_token", "verify_token", None),
            ("課金バッチ", "run_billing_batch", None),
            ("自然言語検索する", "nlp-search", "and"),
            (
                "この文書は自然言語検索の背景を助詞や接続を含めて長く書いた",
                "long-prose",
                "and",
            ),
            ("ユーザー取得", "ユーザー取得", None),
            ("ユーザー", "ユーザー取得", None),
        ]
        for query, expected, want_mode in hit1:
            names, mode = _names(store, query)
            assert names and names[0] == expected, (query, names, mode)
            if want_mode is not None:
                assert mode == want_mode, (query, mode, names)

        names, mode = _names(store, "検索する")
        assert mode != "none"
        assert any(name in {"nlp-search", "search-button", "ops-search"} for name in names[:5])
    finally:
        store.close()
