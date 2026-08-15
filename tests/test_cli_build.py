from __future__ import annotations

import argparse

from dagayn.cli.commands.build import (
    _print_local_embedding_summary,
    _print_vcs_status,
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
    assert args.local_embedding_mode is None
    assert args.local_embedding_port == 19090
    assert args.local_embedding_bin == "/tmp/llama-server"
    assert args.keep_local_embedding_server is True
    assert args.local_embedding_timeout == 12
    assert args.local_embedding_request_timeout == 17
    assert args.local_embedding_batch_size == 8


def test_build_parser_accepts_bge_local_embedding_default():
    args = _parser().parse_args(["build", "--local-embedding"])

    assert args.command == "build"
    assert args.local_embedding == "bge-m3"
    assert args.local_embedding_mode is None


def test_build_parser_accepts_llama_qwen3_mode_override():
    args = _parser().parse_args(["build", "--local-embedding", "--mode", "llama-qwen3"])

    assert args.command == "build"
    assert args.local_embedding == "bge-m3"
    assert args.local_embedding_mode == "llama-qwen3"


def test_build_parser_accepts_force_full_build_options():
    args = _parser().parse_args(["build", "--force-full-build"])
    alias_args = _parser().parse_args(["build", "--force"])

    assert args.command == "build"
    assert args.force_full_build is True
    assert alias_args.force_full_build is True


def test_visualize_parser_rejects_removed_serve_flag():
    try:
        _parser().parse_args(["visualize", "--serve"])
    except SystemExit as exc:
        assert exc.code == 2
    else:  # pragma: no cover - argparse should exit
        raise AssertionError("--serve should not be accepted by visualize")


def test_visualize_parser_requires_explicit_non_html_export_format():
    parser = _parser()
    args = parser.parse_args(["visualize", "--format", "svg"])

    assert args.format == "svg"
    for argv in (
        ["visualize"],
        ["visualize", "--mode", "community"],
        ["visualize", "--format", "html"],
        ["visualize", "--format", "dot"],
    ):
        try:
            parser.parse_args(argv)
        except SystemExit as exc:
            assert exc.code == 2
        else:  # pragma: no cover - argparse should exit
            raise AssertionError(f"{argv!r} should not be accepted by visualize")


def test_visualize_parser_does_not_offer_graphviz_dot_format():
    parser = _parser()
    try:
        parser.parse_args(["visualize", "--format", "dot"])
    except SystemExit as exc:
        assert exc.code == 2
    else:  # pragma: no cover - argparse should exit
        raise AssertionError("Graphviz/DOT export should not be accepted")


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


def test_update_base_defaults_to_the_graphs_own_commit():
    """A hard-coded HEAD~1 default silently skips every commit in between."""
    args = _parser().parse_args(["update"])

    assert args.base is None


def test_update_parser_accepts_local_embedding_options():
    args = _parser().parse_args(["update", "--local-embedding", "low"])

    assert args.command == "update"
    assert args.local_embedding == "low"
    assert args.local_embedding_mode is None
    assert args.local_embedding_port is None
    assert args.local_embedding_bin == "auto"
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
                "mode": "llama-qwen3",
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


def test_print_local_embedding_summary_for_bge_sidecar(capsys):
    _print_local_embedding_summary(
        {
            "local_embedding": {
                "preset": "bge-m3",
                "text_mode": "material",
                "mode": "bge-m3",
                "server_started": False,
                "newly_embedded": 1,
                "orphans_removed": 0,
                "total_embeddings": 9,
            }
        }
    )

    out = capsys.readouterr().out
    assert "Local embeddings (bge-m3/material, reused server)" in out


class _FakeMetaStore:
    def __init__(self, meta: dict[str, str]):
        self._meta = meta

    def get_metadata(self, key: str) -> str | None:
        return self._meta.get(key)


def test_print_vcs_status_warns_on_git_commit_drift(tmp_path, monkeypatch, capsys):
    import dagayn.incremental as incremental

    monkeypatch.setattr(incremental, "detect_vcs", lambda _root: "git")
    monkeypatch.setattr(
        incremental,
        "_git_branch_info",
        lambda _root: ("main", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    )
    store = _FakeMetaStore(
        {
            "git_branch": "main",
            "git_head_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        }
    )

    _print_vcs_status(tmp_path, store)

    out = capsys.readouterr().out
    assert "Built on branch: main" in out
    assert "Built at commit: aaaaaaaaaaaa" in out
    assert "WARNING: Graph was built at commit 'aaaaaaaaaaaa'" in out
    assert "HEAD is now 'bbbbbbbbbbbb'" in out


def test_print_vcs_status_warns_on_git_branch_change(tmp_path, monkeypatch, capsys):
    import dagayn.incremental as incremental

    monkeypatch.setattr(incremental, "detect_vcs", lambda _root: "git")
    monkeypatch.setattr(
        incremental,
        "_git_branch_info",
        lambda _root: ("feature", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    )
    store = _FakeMetaStore(
        {
            "git_branch": "main",
            "git_head_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        }
    )

    _print_vcs_status(tmp_path, store)

    out = capsys.readouterr().out
    assert "WARNING: Graph was built on 'main'" in out
    assert "you are now on 'feature'" in out
    assert "Built at commit" in out
    assert "HEAD is now" not in out


def test_print_vcs_status_warns_on_svn_revision_drift(tmp_path, monkeypatch, capsys):
    import dagayn.incremental as incremental

    monkeypatch.setattr(incremental, "detect_vcs", lambda _root: "svn")
    monkeypatch.setattr(
        incremental,
        "_svn_revision_info",
        lambda _root: ("trunk", "42"),
    )
    store = _FakeMetaStore(
        {
            "svn_branch": "trunk",
            "svn_revision": "41",
        }
    )

    _print_vcs_status(tmp_path, store)

    out = capsys.readouterr().out
    assert "SVN branch: trunk" in out
    assert "SVN revision at build: 41" in out
    assert "WARNING: Graph was built at SVN revision '41'" in out
    assert "working copy is now '42'" in out


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
            "local_embedding_mode": None,
            "local_embedding_port": None,
            "local_embedding_bin": "auto",
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


def test_handle_build_does_not_open_a_cli_graph_store(tmp_path, monkeypatch):
    """build_or_update_graph owns the connection; a CLI GraphStore corrupts WAL."""
    from dagayn.tools import build as build_tools

    def fake_build_or_update_graph(**_kwargs):
        return {
            "files_parsed": 1,
            "total_nodes": 1,
            "total_edges": 0,
            "errors": [],
        }

    def boom(*_args, **_kwargs):
        raise AssertionError("CLI build must not open GraphStore")

    monkeypatch.setattr(build_tools, "build_or_update_graph", fake_build_or_update_graph)
    monkeypatch.setattr("dagayn.graph.GraphStore", boom)

    args = _parser().parse_args(
        ["build", "--repo", str(tmp_path), "--force-full-build", "--skip-postprocess"]
    )
    handle(args)


def test_handle_postprocess_does_not_open_a_cli_graph_store(tmp_path, monkeypatch):
    from dagayn.tools import build as build_tools

    def fake_run_postprocess(**_kwargs):
        return {"flows_detected": 0, "communities_detected": 0, "fts_indexed": 0}

    def boom(*_args, **_kwargs):
        raise AssertionError("CLI postprocess must not open GraphStore")

    monkeypatch.setattr(build_tools, "run_postprocess", fake_run_postprocess)
    monkeypatch.setattr("dagayn.graph.GraphStore", boom)

    args = _parser().parse_args(["postprocess", "--repo", str(tmp_path)])
    handle(args)


def test_handle_update_closes_metadata_peek_before_rebuild(tmp_path, monkeypatch):
    from dagayn.tools import build as build_tools

    open_stores: list[object] = []

    class PeekStore:
        def __init__(self, *_args, **_kwargs):
            self.closed = False
            open_stores.append(self)

        def get_metadata(self, key: str) -> str | None:
            assert key == "git_head_sha"
            return "abc123"

        def close(self) -> None:
            self.closed = True

    def fake_build_or_update_graph(**kwargs):
        assert open_stores, "update should peek git_head_sha"
        assert open_stores[0].closed, "peek store must close before rebuild"
        assert kwargs["base"] == "abc123"
        return {
            "files_updated": 0,
            "total_nodes": 0,
            "total_edges": 0,
        }

    monkeypatch.setattr("dagayn.graph.GraphStore", PeekStore)
    monkeypatch.setattr(build_tools, "build_or_update_graph", fake_build_or_update_graph)

    args = _parser().parse_args(["update", "--repo", str(tmp_path)])
    handle(args)
