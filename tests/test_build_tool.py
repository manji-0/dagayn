"""Behaviour of ``dagayn.tools.build`` against real git repositories.

``build_or_update_graph`` is what ``build_or_update_graph_tool``, the queue
worker, session prepare and the Python CLI fallback (jj/SVN, ``--scip``,
``CRG_DATA_DIR``, embeddings) all run, so these tests go through it rather than
the helpers it calls.
"""

from __future__ import annotations

import functools
import sqlite3
import subprocess
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

import pytest

import dagayn.tools.build as build_tool
from dagayn import write_lock
from dagayn.graph import GraphStore
from dagayn.paths import ALLOW_WIDE_ROOT_ENV, get_db_path
from dagayn.tools.build import (
    _embed_slice_seconds,
    _resolve_local_embedding_mode,
    build_or_update_graph,
    run_embedding_pass,
    run_postprocess,
)

_ROOT_HINT_ENVS = (
    "CRG_REPO_ROOT",
    "CLAUDE_PROJECT_DIR",
    "CURSOR_PROJECT_DIR",
    "WORKSPACE_FOLDER_PATHS",
    "CRG_DATA_DIR",
)


def _git(repo: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-c", "commit.gpgsign=false", *args],
        cwd=str(repo),
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    )


@pytest.fixture(autouse=True)
def _isolated_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for name in (*_ROOT_HINT_ENVS, "DAGAYN_HOOK_UPDATE", "DAGAYN_BACKEND", ALLOW_WIDE_ROOT_ENV):
        monkeypatch.delenv(name, raising=False)


@pytest.fixture()
def repo(main_repo: Path) -> Path:
    """``main_repo`` plus a caller of ``greet`` so flows have an entry point."""
    (main_repo / "app.py").write_text(
        "from hello import greet\n\n\ndef main():\n    return greet()\n", encoding="utf-8"
    )
    _git(main_repo, "add", "app.py")
    _git(main_repo, "commit", "-m", "app")
    return main_repo


@contextmanager
def _write_lock_held_by_another_thread(db_path: Path) -> Iterator[None]:
    held = threading.Event()
    release = threading.Event()

    def hold() -> None:
        with write_lock.graph_write_lock(db_path):
            held.set()
            release.wait(10)

    holder = threading.Thread(target=hold, daemon=True)
    holder.start()
    assert held.wait(5), "lock holder never acquired the lock"
    try:
        yield
    finally:
        release.set()
        holder.join(5)


def _short_lock_timeout(monkeypatch: pytest.MonkeyPatch) -> None:
    # ``graph_write_lock``'s default timeout is bound when the module loads, so
    # an env override would come too late; bound it at the call site instead.
    monkeypatch.setattr(
        build_tool,
        "graph_write_lock",
        functools.partial(write_lock.graph_write_lock, timeout=0.2),
    )


def _graph_metadata(repo: Path, key: str) -> str | None:
    store = GraphStore(get_db_path(repo))
    try:
        return store.get_metadata(key)
    finally:
        store.close()


class TestWriteLockContention:
    def test_hook_update_skips_while_another_writer_holds_the_lock(self, repo, monkeypatch):
        build_or_update_graph(full_rebuild=True, repo_root=str(repo), postprocess="none")
        monkeypatch.setenv("DAGAYN_HOOK_UPDATE", "1")

        with _write_lock_held_by_another_thread(get_db_path(repo)):
            result = build_or_update_graph(repo_root=str(repo), postprocess="minimal")

        assert result["status"] == "ok"
        assert result["skipped"] is True
        assert result["skip_reason"] == "hook_update_already_running"
        assert result["files_updated"] == 0

    def test_manual_build_reports_an_error_when_the_lock_never_frees(self, repo, monkeypatch):
        _short_lock_timeout(monkeypatch)

        with _write_lock_held_by_another_thread(get_db_path(repo)):
            result = build_or_update_graph(
                full_rebuild=True, repo_root=str(repo), postprocess="none"
            )

        assert result["status"] == "error"
        assert result["build_type"] == "full"
        assert result["skip_reason"] == "write_lock_unavailable"
        assert result["errors"] and result["errors"][0]["error"]
        # Nothing was written: the build never opened the store.
        assert _graph_metadata(repo, "last_build_type") is None

    def test_run_postprocess_reports_an_error_when_the_lock_never_frees(self, repo, monkeypatch):
        build_or_update_graph(full_rebuild=True, repo_root=str(repo), postprocess="none")
        _short_lock_timeout(monkeypatch)

        with _write_lock_held_by_another_thread(get_db_path(repo)):
            result = run_postprocess(repo_root=str(repo))

        assert result["status"] == "error"
        assert result["skip_reason"] == "write_lock_unavailable"
        assert _graph_metadata(repo, "last_postprocessed_at") is None


class TestRepoRootResolution:
    def test_auto_detects_the_repository_from_the_working_directory(self, repo, monkeypatch):
        monkeypatch.chdir(repo / ".git")

        result = build_or_update_graph(full_rebuild=True, postprocess="none")

        assert result["status"] == "ok"
        assert get_db_path(repo).is_file()
        assert _graph_metadata(repo, "repo_root") == str(repo.resolve())

    def test_refuses_an_auto_detected_home_directory(self, monkeypatch):
        monkeypatch.setattr(
            "dagayn.incremental_files.find_project_root", lambda *a, **k: str(Path.home())
        )

        with pytest.raises(ValueError, match="your home directory") as excinfo:
            build_or_update_graph(full_rebuild=True, postprocess="none")
        assert ALLOW_WIDE_ROOT_ENV in str(excinfo.value)


class TestIncrementalPostprocess:
    def test_full_postprocess_after_a_commit_traces_the_changed_files(self, repo):
        first = build_or_update_graph(full_rebuild=True, repo_root=str(repo))
        assert first["status"] == "ok"

        (repo / "cli.py").write_text(
            "from app import main\n\n\ndef run():\n    return main()\n", encoding="utf-8"
        )
        _git(repo, "add", "cli.py")
        _git(repo, "commit", "-m", "cli")

        result = build_or_update_graph(repo_root=str(repo), base="HEAD~1", postprocess="full")

        assert result["status"] == "ok"
        assert result["build_type"] == "incremental"
        assert "cli.py" in result["changed_files"]
        assert "flows_detected" in result
        assert "communities_detected" in result
        assert result.get("summaries_computed") is True
        assert _graph_metadata(repo, "postprocess_level") == "full"
        store = GraphStore(get_db_path(repo))
        try:
            assert any(n.name == "run" for n in store.get_nodes_by_file("cli.py"))
        finally:
            store.close()


@pytest.fixture()
def failing_signatures(monkeypatch: pytest.MonkeyPatch) -> None:
    """Make every store the build tool opens fail to compute signatures."""
    real_get_store = build_tool._get_store

    def get_store(repo_root=None, cached=True):
        store, root = real_get_store(repo_root, cached=cached)

        def boom(*_args, **_kwargs):
            raise sqlite3.OperationalError("disk I/O error")

        store.compute_missing_signatures = boom
        return store, root

    monkeypatch.setattr(build_tool, "_get_store", get_store)


_PAYLOAD_DROPS_WARNINGS = (
    "build_result_payload flattens BuildResult.postprocess over the top-level keys, "
    "and PostprocessResult.warnings defaults to [], so the build's own warnings are "
    "always replaced by [] on the wire (MCP tool, session prepare, queue worker)"
)


class TestPostprocessFailuresAreWarnings:
    def test_build_survives_a_failed_postprocess_step(self, repo, failing_signatures, caplog):
        result = build_or_update_graph(
            full_rebuild=True, repo_root=str(repo), postprocess="minimal"
        )

        assert result["status"] == "ok"
        assert "Signature computation failed: disk I/O error" in caplog.text
        assert "signatures_updated" not in result
        # The remaining steps still ran.
        assert result["fts_indexed"] > 0
        assert _graph_metadata(repo, "postprocess_level") == "minimal"

    @pytest.mark.xfail(strict=True, reason=_PAYLOAD_DROPS_WARNINGS)
    def test_build_reports_a_failed_postprocess_step(self, repo, failing_signatures):
        result = build_or_update_graph(
            full_rebuild=True, repo_root=str(repo), postprocess="minimal"
        )

        assert result["warnings"] == [
            "Signature computation failed: OperationalError: disk I/O error"
        ]

    def test_run_postprocess_survives_failed_steps(self, repo, failing_signatures, caplog):
        build_or_update_graph(full_rebuild=True, repo_root=str(repo), postprocess="none")

        with patch(
            "dagayn.search.rebuild_fts_index",
            side_effect=sqlite3.OperationalError("fts5 missing"),
        ):
            result = run_postprocess(repo_root=str(repo))

        assert result["status"] == "ok"
        assert "Signature computation failed: disk I/O error" in caplog.text
        assert "FTS index rebuild failed: fts5 missing" in caplog.text
        assert "fts_indexed" not in result
        # The failed FTS step was rolled back and the later steps still wrote.
        assert result["flows_detected"] >= 1
        assert result["communities_detected"] >= 1
        assert _graph_metadata(repo, "last_postprocessed_at")

    @pytest.mark.xfail(strict=True, reason=_PAYLOAD_DROPS_WARNINGS)
    def test_run_postprocess_reports_failed_steps(self, repo, failing_signatures):
        build_or_update_graph(full_rebuild=True, repo_root=str(repo), postprocess="none")

        result = run_postprocess(repo_root=str(repo))

        assert result["warnings"] == [
            "Signature computation failed: OperationalError: disk I/O error"
        ]


class TestRunPostprocess:
    def test_completes_a_graph_built_without_postprocessing(self, repo):
        build_or_update_graph(full_rebuild=True, repo_root=str(repo), postprocess="none")
        assert _graph_metadata(repo, "last_postprocessed_at") is None

        result = run_postprocess(repo_root=str(repo))

        assert result["status"] == "ok"
        assert result["summary"] == "Post-processing complete."
        assert result["signatures_updated"] is True
        assert result["fts_indexed"] >= 3  # greet, main, and the file nodes
        assert result["flows_detected"] >= 1  # main -> greet
        assert result["communities_detected"] >= 1
        assert not result.get("warnings")
        assert _graph_metadata(repo, "last_postprocessed_at")

    def test_skipped_steps_leave_their_tables_alone(self, repo):
        build_or_update_graph(full_rebuild=True, repo_root=str(repo), postprocess="none")

        result = run_postprocess(flows=False, communities=False, repo_root=str(repo))

        assert result["fts_indexed"] >= 3
        assert "flows_detected" not in result
        assert "communities_detected" not in result


class TestScip:
    def test_full_build_reports_hints_for_indexers_it_may_not_run(self, repo, monkeypatch):
        monkeypatch.delenv("DAGAYN_SCIP_ALLOW_BUILD", raising=False)
        (repo / "pom.xml").write_text("<project/>\n", encoding="utf-8")
        (repo / "Main.java").write_text(
            "class Main { void run() {} }\n",
            encoding="utf-8",
        )

        result = build_or_update_graph(
            full_rebuild=True, repo_root=str(repo), postprocess="none", scip=True
        )

        assert result["status"] == "ok"
        assert result["scip_overlay"] == []
        assert any(hint.startswith("java:") for hint in result["scip_hints"])


class TestRunEmbeddingPass:
    def test_embeds_without_a_structural_update(self, repo):
        build_or_update_graph(full_rebuild=True, repo_root=str(repo), postprocess="none")
        before = _graph_metadata(repo, "last_updated")
        # An uncommitted edit an update would pick up.
        (repo / "hello.py").write_text("def greet():\n    return 'hi'\n", encoding="utf-8")
        local = {"status": "ok", "summary": "Embedded 2 new node(s).", "newly_embedded": 2}

        with patch.object(build_tool, "_run_local_embedding", return_value=local) as run:
            result = run_embedding_pass(
                repo_root=str(repo), local_embedding="low", embed_files=["hello.py"]
            )

        assert result["status"] == "ok"
        assert result["summary"] == "Embedded 2 new node(s)."
        assert result["local_embedding"] == local
        root = run.call_args.args[0]
        assert Path(root) == repo.resolve()
        assert run.call_args.kwargs["file_paths"] == ["hello.py"]
        assert run.call_args.kwargs["local_embedding"] == "low"
        assert _graph_metadata(repo, "last_updated") == before


class TestEmbeddingSettings:
    @pytest.mark.parametrize(
        ("raw", "expected"),
        [(None, 4.0), ("2.5", 2.5), ("not-a-number", 4.0), ("0", None), ("-1", None)],
    )
    def test_slice_seconds(self, monkeypatch, raw, expected):
        if raw is None:
            monkeypatch.delenv("DAGAYN_EMBED_SLICE_SECONDS", raising=False)
        else:
            monkeypatch.setenv("DAGAYN_EMBED_SLICE_SECONDS", raw)
        assert _embed_slice_seconds() == expected

    @pytest.mark.parametrize(
        ("local_embedding", "mode", "expected"),
        [
            ("bge-m3", None, "bge-m3"),
            ("low", None, "llama-qwen3"),
            (" Qwen3 ", None, "llama-qwen3"),
            (None, None, "bge-m3"),
            ("low", " BGE-M3 ", "bge-m3"),
        ],
    )
    def test_local_embedding_mode(self, local_embedding, mode, expected):
        assert _resolve_local_embedding_mode(local_embedding, mode) == expected
