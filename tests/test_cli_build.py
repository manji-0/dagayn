from __future__ import annotations

import argparse

from dagayn.cli.commands.build import (
    _print_local_embedding_summary,
    _remove_existing_graph_database,
    handle,
    register_commands,
)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command")
    register_commands(sub)
    return parser


def test_build_parser_accepts_local_embedding_options():
    args = _parser().parse_args(
        [
            "build",
            "--local-embedding",
            "low",
            "--local-embedding-port",
            "19090",
            "--local-embedding-bin",
            "/tmp/llama-server",
            "--keep-local-embedding-server",
            "--local-embedding-timeout",
            "12",
            "--local-embedding-request-timeout",
            "17",
            "--local-embedding-batch-size",
            "8",
        ]
    )

    assert args.command == "build"
    assert args.local_embedding == "low"
    assert args.local_embedding_port == 19090
    assert args.local_embedding_bin == "/tmp/llama-server"
    assert args.keep_local_embedding_server is True
    assert args.local_embedding_timeout == 12
    assert args.local_embedding_request_timeout == 17
    assert args.local_embedding_batch_size == 8


def test_build_parser_accepts_force_full_build_options():
    args = _parser().parse_args(["build", "--force-full-build"])
    alias_args = _parser().parse_args(["build", "--force"])

    assert args.command == "build"
    assert args.force_full_build is True
    assert alias_args.force_full_build is True


def test_remove_existing_graph_database_removes_sqlite_sidecars(tmp_path):
    db_path = tmp_path / "graph.db"
    paths = [db_path] + [
        db_path.with_name(f"{db_path.name}{suffix}") for suffix in ("-wal", "-shm", "-journal")
    ]
    for path in paths:
        path.write_text("stale")

    removed = _remove_existing_graph_database(db_path)

    assert removed == paths
    assert all(not path.exists() for path in paths)


def test_update_parser_accepts_local_embedding_options():
    args = _parser().parse_args(["update", "--local-embedding", "low"])

    assert args.command == "update"
    assert args.local_embedding == "low"
    assert args.local_embedding_port == 18080
    assert args.local_embedding_bin == "llama-server"
    assert args.keep_local_embedding_server is False
    assert args.local_embedding_timeout == 300
    assert args.local_embedding_request_timeout == 60
    assert args.local_embedding_batch_size == 1


def test_print_local_embedding_summary_includes_orphan_count(capsys):
    _print_local_embedding_summary(
        {
            "local_embedding": {
                "preset": "low",
                "text_mode": "metadata",
                "server_started": False,
                "newly_embedded": 1,
                "orphans_removed": 2,
                "total_embeddings": 9,
            }
        }
    )

    out = capsys.readouterr().out
    assert "Local embeddings (low/metadata, reused server)" in out
    assert "1 new, 2 orphan removed, 9 total" in out


def test_handle_runs_full_build_without_postprocess(tmp_path, monkeypatch, capsys):
    from dagayn.tools import build as build_tools

    calls = []

    def fake_build_or_update_graph(**kwargs):
        calls.append(kwargs)
        return {
            "files_parsed": 3,
            "total_nodes": 5,
            "total_edges": 7,
            "errors": [],
        }

    monkeypatch.setattr(build_tools, "build_or_update_graph", fake_build_or_update_graph)

    args = _parser().parse_args(["build", "--repo", str(tmp_path), "--skip-postprocess"])

    handle(args)

    assert calls == [
        {
            "full_rebuild": True,
            "repo_root": str(tmp_path),
            "postprocess": "none",
            "local_embedding": "none",
            "local_embedding_port": 18080,
            "local_embedding_bin": "llama-server",
            "keep_local_embedding_server": False,
            "local_embedding_timeout": 300,
            "local_embedding_request_timeout": 60,
            "local_embedding_batch_size": 1,
        }
    ]
    assert "Full build: 3 files, 5 nodes, 7 edges (postprocess=none)" in capsys.readouterr().out


def test_handle_prints_build_postprocess_result_without_rerunning(tmp_path, monkeypatch, capsys):
    from dagayn import postprocessing
    from dagayn.tools import build as build_tools

    def fake_build_or_update_graph(**_kwargs):
        return {
            "files_parsed": 3,
            "total_nodes": 5,
            "total_edges": 7,
            "errors": [],
            "fts_indexed": 11,
            "flows_detected": 2,
            "communities_detected": 4,
        }

    def fail_run_post_processing(_store):
        raise AssertionError("CLI should not re-run post-processing")

    monkeypatch.setattr(build_tools, "build_or_update_graph", fake_build_or_update_graph)
    monkeypatch.setattr(postprocessing, "run_post_processing", fail_run_post_processing)

    args = _parser().parse_args(["build", "--repo", str(tmp_path)])

    handle(args)

    out = capsys.readouterr().out
    assert "Full build: 3 files, 5 nodes, 7 edges (postprocess=full)" in out
    assert "FTS indexed: 11 nodes" in out
    assert "Flows: 2" in out
    assert "Communities: 4" in out
