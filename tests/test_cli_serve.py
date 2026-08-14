from __future__ import annotations

import argparse
import os
import sqlite3
from types import SimpleNamespace

import pytest

from dagayn.cli.commands.serve import handle, register_command


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command")
    register_command(sub)
    return parser


def test_serve_parser_accepts_remote_embedding_provider():
    args = _parser().parse_args(["serve", "--remote-embedding", "openai"])

    assert args.command == "serve"
    assert args.remote_embedding == "openai"


def test_serve_parser_rejects_removed_tool_profile_flag():
    with pytest.raises(SystemExit):
        _parser().parse_args(["serve", "--tool-profile", "review"])


def test_serve_rejects_local_and_remote_embedding_together():
    parser = _parser()
    args = parser.parse_args(["serve", "--local-embedding", "low", "--remote-embedding", "openai"])

    with pytest.raises(SystemExit):
        handle(args, parser)


def test_serve_local_embedding_sets_search_default_to_openai(monkeypatch):
    calls: list[dict] = []

    class FakeServer:
        base_url = "http://127.0.0.1:18080/v1"
        started = False
        command: list[str] = []
        preset = SimpleNamespace(
            model="qwen3-embedding-0.6b-gguf-q8_0",
            text_mode="material",
            request_max_length=None,
        )

    class FakeContext:
        def __enter__(self):
            return FakeServer()

        def __exit__(self, *_args):
            return None

    monkeypatch.setattr(
        "dagayn.local_embeddings.local_embedding_server",
        lambda *_args, **_kwargs: FakeContext(),
    )
    monkeypatch.setattr("dagayn.main.main", lambda **kwargs: calls.append(kwargs))
    monkeypatch.setenv("CRG_OPENAI_MODEL", "old-model")

    parser = _parser()
    args = parser.parse_args(["serve", "--local-embedding", "low"])
    handle(args, parser)

    assert calls[0]["embedding_provider"] == "openai"
    assert calls[0]["embedding_model"] == "qwen3-embedding-0.6b-gguf-q8_0"
    assert calls[0]["local_embedding"] == "low"
    assert calls[0]["local_embedding_port"] == 18081
    assert calls[0]["local_embedding_bin"] == "auto"
    assert calls[0]["keep_local_embedding_server"] is False
    assert calls[0]["local_embedding_timeout"] == 300
    assert calls[0]["local_embedding_request_timeout"] == 60
    assert calls[0]["local_embedding_batch_size"] == 1
    assert os.environ["CRG_OPENAI_MODEL"] == "qwen3-embedding-0.6b-gguf-q8_0"
    assert os.environ["DAGAYN_EMBEDDING_TEXT_MODE"] == "material"
    assert os.environ.get("CRG_OPENAI_MAX_LENGTH") is None


def test_serve_bare_local_embedding_uses_bge_sidecar(monkeypatch):
    calls: list[dict] = []

    class FakeServer:
        base_url = "http://127.0.0.1:18080/v1"
        started = False
        command: list[str] = []
        preset = SimpleNamespace(
            model="bge-m3-gguf-q8_0",
            text_mode="material",
            request_max_length=None,
        )

    class FakeContext:
        def __enter__(self):
            return FakeServer()

        def __exit__(self, *_args):
            return None

    monkeypatch.setattr("dagayn.main.main", lambda **kwargs: calls.append(kwargs))
    monkeypatch.setattr(
        "dagayn.local_embeddings.local_embedding_server",
        lambda *_args, **_kwargs: FakeContext(),
    )

    parser = _parser()
    args = parser.parse_args(["serve", "--local-embedding"])
    handle(args, parser)

    assert calls[0]["embedding_provider"] == "openai"
    assert calls[0]["embedding_model"] == "bge-m3-gguf-q8_0"
    assert calls[0]["local_embedding"] == "bge-m3"
    assert os.environ["DAGAYN_EMBEDDING_TEXT_MODE"] == "material"


def test_serve_remote_embedding_sets_search_default(monkeypatch):
    calls: list[dict] = []
    monkeypatch.setattr("dagayn.main.main", lambda **kwargs: calls.append(kwargs))

    parser = _parser()
    args = parser.parse_args(["serve", "--remote-embedding", "google"])
    handle(args, parser)

    assert calls[0]["embedding_provider"] == "google"
    assert calls[0]["embedding_model"] is None
    assert calls[0]["local_embedding"] is None


def test_serve_infers_local_embedding_from_existing_graph(monkeypatch, tmp_path):
    calls: list[dict] = []
    server_calls: list[dict] = []
    repo = tmp_path / "repo"
    db_dir = repo / ".dagayn"
    db_dir.mkdir(parents=True)
    db = db_dir / "graph.db"
    conn = sqlite3.connect(db)
    try:
        conn.execute(
            """
            CREATE TABLE embeddings (
                qualified_name TEXT PRIMARY KEY,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'unknown'
            )
            """
        )
        conn.execute(
            "INSERT INTO embeddings VALUES (?, ?, ?, ?)",
            (
                "dagayn/search.py::hybrid_search",
                b"\x00\x00\x00\x00",
                "hash",
                "openai:qwen3-embedding-0.6b-gguf-q8_0@http://127.0.0.1:19090/v1",
            ),
        )
        conn.commit()
    finally:
        conn.close()

    class FakeServer:
        base_url = "http://127.0.0.1:19090/v1"
        started = True
        command: list[str] = []
        preset = SimpleNamespace(
            model="qwen3-embedding-0.6b-gguf-q8_0",
            text_mode="material",
            request_max_length=None,
        )

    class FakeContext:
        def __enter__(self):
            return FakeServer()

        def __exit__(self, *_args):
            return None

    def fake_server(level, **kwargs):
        server_calls.append({"level": level, **kwargs})
        return FakeContext()

    monkeypatch.setattr("dagayn.local_embeddings.local_embedding_server", fake_server)
    monkeypatch.setattr("dagayn.main.main", lambda **kwargs: calls.append(kwargs))

    parser = _parser()
    args = parser.parse_args(["serve", "--repo", str(repo)])
    handle(args, parser)

    assert server_calls == [
        {
            "level": "low",
            "runtime": "llama",
            "port": 19090,
            "binary": "auto",
            "keep_running": False,
            "startup_timeout": 300,
        }
    ]
    assert calls[0]["embedding_provider"] == "openai"
    assert calls[0]["embedding_model"] == "qwen3-embedding-0.6b-gguf-q8_0"
    assert calls[0]["local_embedding"] == "low"
    assert calls[0]["local_embedding_port"] == 19090
    assert os.environ["DAGAYN_EMBEDDING_TEXT_MODE"] == "material"


def test_serve_infers_bge_local_embedding_from_existing_graph(monkeypatch, tmp_path):
    calls: list[dict] = []
    server_calls: list[dict] = []
    repo = tmp_path / "repo"
    db_dir = repo / ".dagayn"
    db_dir.mkdir(parents=True)
    db = db_dir / "graph.db"
    conn = sqlite3.connect(db)
    try:
        conn.execute(
            """
            CREATE TABLE embeddings (
                qualified_name TEXT PRIMARY KEY,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'unknown'
            )
            """
        )
        conn.execute(
            "INSERT INTO embeddings VALUES (?, ?, ?, ?)",
            (
                "dagayn/search.py::hybrid_search",
                b"\x00\x00\x00\x00",
                "hash",
                "openai:bge-m3-gguf-q8_0@http://127.0.0.1:19093/v1",
            ),
        )
        conn.commit()
    finally:
        conn.close()

    class FakeServer:
        base_url = "http://127.0.0.1:19093/v1"
        started = True
        command: list[str] = []
        preset = SimpleNamespace(
            model="bge-m3-gguf-q8_0",
            text_mode="material",
            request_max_length=None,
        )

    class FakeContext:
        def __enter__(self):
            return FakeServer()

        def __exit__(self, *_args):
            return None

    def fake_server(level, **kwargs):
        server_calls.append({"level": level, **kwargs})
        return FakeContext()

    monkeypatch.setattr("dagayn.local_embeddings.local_embedding_server", fake_server)
    monkeypatch.setattr("dagayn.main.main", lambda **kwargs: calls.append(kwargs))

    parser = _parser()
    args = parser.parse_args(["serve", "--repo", str(repo)])
    handle(args, parser)

    assert server_calls[0]["level"] == "bge-m3"
    assert server_calls[0]["port"] == 19093
    assert calls[0]["embedding_provider"] == "openai"
    assert calls[0]["embedding_model"] == "bge-m3-gguf-q8_0"
    assert calls[0]["local_embedding"] == "bge-m3"
    assert calls[0]["local_embedding_port"] == 19093
