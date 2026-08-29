"""Rust backend parity tests for Rust-owned parser paths."""

from __future__ import annotations

import json
import os
import shutil
import sqlite3
import sys
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.incremental import (
    _rust_backend_enabled,
    _split_rust_parser_files,
    full_build,
    incremental_update,
)
from dagayn.parser import CodeParser, EdgeInfo, NodeInfo

sys.path.insert(0, str(Path(__file__).parent.parent / "tools"))
from parity_export import export_db  # noqa: E402

from tests.conftest import PARITY_FIXTURE_DIR

FIXTURES = Path(__file__).parent / "fixtures"

RUST_OWNED_PARITY_FIXTURES = [
    "terraform_only",
    "markdown_only",
    "python_only",
    "notebook",
    "mixed",
]


def test_get_affected_flows_absolute_path_matches_rust_backend(tmp_path):
    """Python and Rust stores return the same affected flows for absolute paths."""
    try:
        from dagayn._core import GraphStore as RustGraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    from dagayn.flows import get_affected_flows, store_flows, trace_flows
    from dagayn.graph import GraphStore as PythonGraphStore
    from dagayn.parser import EdgeInfo, NodeInfo

    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").mkdir()
    db_path = repo / ".dagayn" / "graph.db"

    store = PythonGraphStore(db_path)
    store.set_metadata("repo_root", str(repo.resolve()))
    try:
        nodes = [
            NodeInfo("Function", "handler", "routes.py", 1, 10, "python"),
            NodeInfo("Function", "service", "services.py", 1, 10, "python"),
            NodeInfo("Function", "repo", "repo.py", 1, 10, "python"),
        ]
        for node in nodes:
            store.upsert_node(node, file_hash="abc")
        edges = [
            EdgeInfo("CALLS", "routes.py::handler", "services.py::service", "routes.py", 5),
            EdgeInfo("CALLS", "services.py::service", "repo.py::repo", "services.py", 5),
        ]
        for edge in edges:
            store.upsert_edge(edge)
        store.commit()
        store_flows(store, trace_flows(store))
    finally:
        store.close()

    abs_path = str((repo / "services.py").resolve())

    py_store = PythonGraphStore(db_path)
    try:
        py_result = get_affected_flows(py_store, [abs_path])
    finally:
        py_store.close()

    rust_store = RustGraphStore(db_path)
    try:
        rust_affected = json.loads(rust_store.get_affected_flows_json([abs_path]))
    finally:
        rust_store.close()

    assert py_result["total"] >= 1
    assert py_result["total"] == len(rust_affected)
    py_names = {flow["name"] for flow in py_result["affected_flows"]}
    rust_names = {flow["name"] for flow in rust_affected}
    assert py_names == rust_names


def test_rust_backend_is_default_when_extension_is_available(monkeypatch):
    """DAGAYN_BACKEND defaults to Rust when the native extension can be loaded."""
    monkeypatch.delenv("DAGAYN_BACKEND", raising=False)
    monkeypatch.setattr("dagayn.incremental._rust_backend_available", lambda: True)

    assert _rust_backend_enabled() is True


def test_rust_file_discovery_includes_compound_terraform_extensions(tmp_path):
    """Rust file discovery keeps .tftest.hcl family files (regression for #135)."""
    try:
        from dagayn._core import collect_parseable_files
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").mkdir()
    (repo / "main.tf").write_text('resource "a" "b" {}\n')
    (repo / "main.tftest.hcl").write_text('run "basic" {\n  command = apply\n}\n')
    (repo / "plain.hcl").write_text('something = "x"\n')

    files = collect_parseable_files(str(repo), False)
    assert "main.tf" in files
    assert "main.tftest.hcl" in files
    assert "plain.hcl" not in files


def test_rust_incremental_candidates_keep_compound_terraform_extensions(tmp_path):
    """filter_incremental_candidates does not mark .tftest.hcl as removed (#135)."""
    try:
        from dagayn._core import filter_incremental_candidates
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").mkdir()
    (repo / "main.tftest.hcl").write_text('run "basic" {}\n')

    parseable, removed = filter_incremental_candidates(str(repo), ["main.tftest.hcl"], [])
    assert parseable == ["main.tftest.hcl"]
    assert removed == []


def test_python_backend_is_rejected(monkeypatch):
    monkeypatch.setenv("DAGAYN_BACKEND", "python")

    with pytest.raises(RuntimeError, match="removed"):
        _rust_backend_enabled()


def test_python_store_uses_python_parser_when_rust_is_default(tmp_path, monkeypatch):
    """Direct Python GraphStore callers still parse through the Rust parser wrapper."""
    monkeypatch.delenv("DAGAYN_BACKEND", raising=False)
    monkeypatch.setattr("dagayn.incremental._rust_backend_available", lambda: True)

    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").mkdir()
    (repo / "main.py").write_text("def hello():\n    return 1\n", encoding="utf-8")

    store = GraphStore(repo / ".dagayn" / "graph.db")
    try:
        result = full_build(repo, store)
    finally:
        store.close()

    assert result.errors == []
    assert result.files_parsed == 1


def _copy_fixture(source: Path, dest: Path) -> None:
    for item in source.iterdir():
        if item.name in (".git", ".dagayn"):
            continue
        target = dest / item.name
        if item.is_dir():
            shutil.copytree(item, target)
        else:
            shutil.copy2(item, target)
    (dest / ".git").mkdir()


@pytest.mark.parametrize("name", RUST_OWNED_PARITY_FIXTURES)
def test_rust_backend_matches_python_parity_snapshots(name, tmp_path_factory, monkeypatch):
    """Rust-owned parser paths must preserve the Python graph contract."""
    try:
        from dagayn._core import GraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    monkeypatch.setenv("DAGAYN_BACKEND", "rust")
    source = PARITY_FIXTURE_DIR / name
    repo = tmp_path_factory.mktemp(f"rustparity_{name}")
    _copy_fixture(source, repo)

    db_path = repo / ".dagayn" / "graph.db"
    store = GraphStore(db_path)
    try:
        full_build(repo, store)
    finally:
        store.close()

    actual = export_db(db_path)
    expected = (PARITY_FIXTURE_DIR / "__snapshots__" / f"{name}.json").read_text(encoding="utf-8")
    assert actual == expected


def test_rust_backend_populates_edge_target_name(tmp_path, monkeypatch):
    """Rust-built graphs must expose populated edges.target_name for name lookups."""
    try:
        from dagayn._core import GraphStore as RustGraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    monkeypatch.setenv("DAGAYN_BACKEND", "rust")
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").mkdir()
    (repo / "main.py").write_text(
        "def helper():\n    pass\n\ndef main():\n    helper()\n",
        encoding="utf-8",
    )

    db_path = repo / ".dagayn" / "graph.db"
    store = RustGraphStore(db_path)
    try:
        full_build(repo, store)
    finally:
        store.close()

    conn = sqlite3.connect(db_path)
    try:
        columns = {row[1] for row in conn.execute("PRAGMA table_info(edges)")}
        assert "target_name" in columns

        rows = conn.execute(
            "SELECT target_qualified, target_name FROM edges WHERE kind = 'CALLS'"
        ).fetchall()
        assert rows
        for target_qualified, target_name in rows:
            assert target_name
            assert target_name == target_qualified.rsplit("::", 1)[-1]
    finally:
        conn.close()

    py_store = GraphStore(db_path)
    try:
        edges = py_store.search_edges_by_target_name("helper", kind="CALLS")
        assert edges
        assert {edge.target_qualified for edge in edges} == {"main.py::helper"}
    finally:
        py_store.close()


def test_rust_backend_incremental_touch_updates_mtime_without_reparse(
    tmp_path_factory, monkeypatch
):
    """Rust-owned unchanged content should not cross back into Python parsing."""
    try:
        from dagayn._core import GraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    monkeypatch.setenv("DAGAYN_BACKEND", "rust")
    source = PARITY_FIXTURE_DIR / "markdown_only"
    repo = tmp_path_factory.mktemp("rustparity_touch")
    _copy_fixture(source, repo)

    db_path = repo / ".dagayn" / "graph.db"
    store = GraphStore(db_path)
    try:
        full_build(repo, store)
        target = repo / "api.md"
        new_mtime_ns = int(target.stat().st_mtime_ns) + 2_000_000_000
        target.touch()
        # Force a deterministic mtime bump while keeping file content unchanged.
        os.utime(target, ns=(new_mtime_ns, new_mtime_ns))

        result = incremental_update(repo, store, changed_files=["api.md"])
        assert result.total_nodes == 0
        assert result.total_edges == 0
        assert result.errors == []
        assert store.get_file_meta_map()["api.md"][1] == new_mtime_ns
    finally:
        store.close()


def test_rust_graph_store_persists_centrality_scores(tmp_path):
    try:
        from dagayn._core import GraphStore as RustGraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    from dagayn.analysis import persist_centrality_scores

    db_path = tmp_path / "graph.db"
    store = RustGraphStore(db_path)
    try:
        nodes = [
            NodeInfo("File", "a.py", "a.py", 1, 1, "python"),
            NodeInfo("Function", "entry", "a.py", 1, 3, "python"),
            NodeInfo("Function", "middle", "a.py", 4, 6, "python"),
            NodeInfo("Function", "leaf", "a.py", 7, 9, "python"),
        ]
        edges = [
            EdgeInfo("CALLS", "a.py::entry", "a.py::middle", "a.py", 2),
            EdgeInfo("CALLS", "a.py::middle", "a.py::leaf", "a.py", 5),
        ]
        store.store_file_nodes_edges("a.py", nodes, edges)

        result = persist_centrality_scores(store)

        assert result["hub_scores_persisted"] == 3
        assert result["bridge_scores_persisted"] == 1
    finally:
        store.close()


def test_rust_graph_store_generates_suggested_questions(tmp_path):
    try:
        from dagayn._core import GraphStore as RustGraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    from dagayn.analysis import generate_suggested_questions, persist_centrality_scores

    db_path = tmp_path / "graph.db"
    store = RustGraphStore(db_path)
    try:
        nodes = [
            NodeInfo("File", "a.py", "a.py", 1, 1, "python"),
            NodeInfo("Function", "entry", "a.py", 1, 3, "python"),
            NodeInfo("Function", "middle", "a.py", 4, 6, "python"),
            NodeInfo("Function", "leaf", "a.py", 7, 9, "python"),
        ]
        edges = [
            EdgeInfo("CALLS", "a.py::entry", "a.py::middle", "a.py", 2),
            EdgeInfo("CALLS", "a.py::middle", "a.py::leaf", "a.py", 5),
        ]
        store.store_file_nodes_edges("a.py", nodes, edges)
        persist_centrality_scores(store)

        questions = generate_suggested_questions(store)

        assert questions
        assert questions[0]["category"] == "bridge_node"
        assert questions[0]["priority"] == "high"
    finally:
        store.close()


def test_rust_backend_routes_databricks_py_exports(tmp_path, monkeypatch):
    """Databricks .py exports stay in the Rust-owned backend path."""
    try:
        from dagayn._core import GraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    monkeypatch.setenv("DAGAYN_BACKEND", "rust")
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").mkdir()
    shutil.copy2(FIXTURES / "sample_databricks_export.py", repo / "notebook.py")

    db_path = repo / ".dagayn" / "graph.db"
    store = GraphStore(db_path)
    try:
        result = full_build(repo, store)
    finally:
        store.close()

    assert result.errors == []
    exported = json.loads(export_db(db_path))
    file_nodes = [node for node in exported["nodes"] if node["kind"] == "File"]
    assert file_nodes[0]["extra"].get("notebook_format") == "databricks_py"
    imports = {edge["target"] for edge in exported["edges"] if edge["kind"] == "IMPORTS_FROM"}
    assert {"bronze.events", "silver.users", "gold.summary", "silver.processed"} <= imports


@pytest.mark.parametrize(
    "fixture",
    [
        "sample.pl",
        "sample_bridge_perl.pl",
        "sample_vue.vue",
        "sample.xs",
        "sample.swift",
        "sample_bridge_swift.swift",
    ],
)
def test_rust_owned_parser_matches_python_parser(fixture):
    """Selected Rust-owned parser paths stay on the Python graph contract."""
    try:
        from dagayn._core import parse_rust_owned_files_compact_json
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    rel_path = f"tests/fixtures/{fixture}"
    source = Path(rel_path).read_bytes()
    py_nodes, py_edges = CodeParser().parse_bytes(Path(rel_path), source)
    payload = json.loads(parse_rust_owned_files_compact_json(Path.cwd(), [rel_path]))
    rust_nodes = payload["batch"][0][1]
    rust_edges = payload["batch"][0][2]

    assert rust_nodes == [
        [
            node.kind,
            node.name,
            node.file_path,
            node.line_start,
            node.line_end,
            node.language,
            node.parent_name,
            node.params,
            node.return_type,
            node.modifiers,
            node.is_test,
            node.extra,
        ]
        for node in py_nodes
    ]
    assert rust_edges == [
        [edge.kind, edge.source, edge.target, edge.file_path, edge.line, edge.extra]
        for edge in py_edges
    ]


def test_rust_owned_c_header_parser_matches_python_parser(tmp_path):
    """C headers use the same C parser contract as .c and Perl XS files."""
    try:
        from dagayn._core import parse_rust_owned_files_compact_json
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "include").mkdir()
    rel_path = "include/user.h"
    source = b"""#ifndef USER_H
#define USER_H
#include <stdint.h>

typedef struct {
    int id;
} User;

static inline int user_id(User *user) {
    return user->id;
}

#endif
"""
    (repo / rel_path).write_bytes(source)

    py_nodes, py_edges = CodeParser().parse_bytes(Path(rel_path), source)
    payload = json.loads(parse_rust_owned_files_compact_json(repo, [rel_path]))

    assert payload["batch"][0][1] == [
        [
            node.kind,
            node.name,
            node.file_path,
            node.line_start,
            node.line_end,
            node.language,
            node.parent_name,
            node.params,
            node.return_type,
            node.modifiers,
            node.is_test,
            node.extra,
        ]
        for node in py_nodes
    ]
    assert payload["batch"][0][2] == [
        [edge.kind, edge.source, edge.target, edge.file_path, edge.line, edge.extra]
        for edge in py_edges
    ]


def test_rust_owned_extensionless_shebang_parser_matches_python_parser(tmp_path, monkeypatch):
    """Extension-less scripts detected by shebang stay in the Rust batch path."""
    try:
        from dagayn._core import parse_rust_owned_files_compact_json
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "bin").mkdir()
    rel_path = "bin/deploy"
    source = b'#!/usr/bin/env bash\ndeploy() {\n  echo "deploy"\n}\n\ndeploy "$@"\n'
    (repo / rel_path).write_bytes(source)

    monkeypatch.chdir(repo)
    py_nodes, py_edges = CodeParser().parse_bytes(Path(rel_path), source)
    payload = json.loads(parse_rust_owned_files_compact_json(repo, [rel_path]))

    assert payload["errors"] == []
    assert payload["batch"][0][1] == [
        [
            node.kind,
            node.name,
            node.file_path,
            node.line_start,
            node.line_end,
            node.language,
            node.parent_name,
            node.params,
            node.return_type,
            node.modifiers,
            node.is_test,
            node.extra,
        ]
        for node in py_nodes
    ]
    assert payload["batch"][0][2] == [
        [edge.kind, edge.source, edge.target, edge.file_path, edge.line, edge.extra]
        for edge in py_edges
    ]


def test_rust_backend_routes_extensionless_shebang_files(tmp_path, monkeypatch):
    """Rust/Python file splitting uses shebang detection for extension-less files."""
    monkeypatch.setenv("DAGAYN_BACKEND", "rust")
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "bin").mkdir()
    (repo / "bin" / "deploy").write_text("#!/usr/bin/env bash\necho deploy\n")
    (repo / "README").write_text("plain text without shebang\n")

    rust_files, python_files = _split_rust_parser_files(
        ["bin/deploy", "README"],
        repo,
    )

    assert rust_files == ["bin/deploy"]
    assert python_files == ["README"]


def test_rust_owned_svelte_parser_matches_python_parser(tmp_path):
    """Svelte script-block extraction stays on the Python graph contract."""
    try:
        from dagayn._core import parse_rust_owned_files_compact_json
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    repo = tmp_path / "repo"
    repo.mkdir()
    rel_path = "Component.svelte"
    source = b"""<script lang="ts">
import { writable } from 'svelte/store'

interface User {
  name: string
}

const count = writable(0)

function increment() {
  console.log('increment')
}
</script>

<button on:click={increment}>{$count}</button>
"""
    (repo / rel_path).write_bytes(source)

    py_nodes, py_edges = CodeParser().parse_bytes(Path(rel_path), source)
    payload = json.loads(parse_rust_owned_files_compact_json(repo, [rel_path]))

    assert payload["batch"][0][1] == [
        [
            node.kind,
            node.name,
            node.file_path,
            node.line_start,
            node.line_end,
            node.language,
            node.parent_name,
            node.params,
            node.return_type,
            node.modifiers,
            node.is_test,
            node.extra,
        ]
        for node in py_nodes
    ]
    assert payload["batch"][0][2] == [
        [edge.kind, edge.source, edge.target, edge.file_path, edge.line, edge.extra]
        for edge in py_edges
    ]


def test_rust_owned_astro_parser_matches_python_parser(tmp_path):
    """Astro keeps the existing TypeScript-backed parser contract."""
    try:
        from dagayn._core import parse_rust_owned_files_compact_json
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    repo = tmp_path / "repo"
    repo.mkdir()
    rel_path = "Page.astro"
    source = b"""---
import Layout from './Layout.astro'
const title = 'Hello'
function getTitle() { return title }
---
<Layout title={title} />
"""
    (repo / rel_path).write_bytes(source)

    py_nodes, py_edges = CodeParser().parse_bytes(Path(rel_path), source)
    payload = json.loads(parse_rust_owned_files_compact_json(repo, [rel_path]))

    assert payload["batch"][0][1] == [
        [
            node.kind,
            node.name,
            node.file_path,
            node.line_start,
            node.line_end,
            node.language,
            node.parent_name,
            node.params,
            node.return_type,
            node.modifiers,
            node.is_test,
            node.extra,
        ]
        for node in py_nodes
    ]
    assert payload["batch"][0][2] == [
        [edge.kind, edge.source, edge.target, edge.file_path, edge.line, edge.extra]
        for edge in py_edges
    ]


def test_rust_owned_zig_parser_matches_python_parser(tmp_path):
    """Zig stays on the current Python File-node-only parser contract."""
    try:
        from dagayn._core import parse_rust_owned_files_compact_json
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    repo = tmp_path / "repo"
    repo.mkdir()
    rel_path = "src/main.zig"
    (repo / "src").mkdir()
    source = b"""const std = @import("std");

pub fn main() void {
    std.debug.print("hello\\n", .{});
}
"""
    (repo / rel_path).write_bytes(source)

    py_nodes, py_edges = CodeParser().parse_bytes(Path(rel_path), source)
    payload = json.loads(parse_rust_owned_files_compact_json(repo, [rel_path]))

    assert payload["batch"][0][1] == [
        [
            node.kind,
            node.name,
            node.file_path,
            node.line_start,
            node.line_end,
            node.language,
            node.parent_name,
            node.params,
            node.return_type,
            node.modifiers,
            node.is_test,
            node.extra,
        ]
        for node in py_nodes
    ]
    assert payload["batch"][0][2] == [
        [edge.kind, edge.source, edge.target, edge.file_path, edge.line, edge.extra]
        for edge in py_edges
    ]


def test_rust_owned_powershell_parser_matches_python_parser(tmp_path):
    """PowerShell stays on the current Python File-node-only parser contract."""
    try:
        from dagayn._core import parse_rust_owned_files_compact_json
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")  # ty: ignore[too-many-positional-arguments]

    repo = tmp_path / "repo"
    repo.mkdir()
    rel_path = "scripts/hello.ps1"
    (repo / "scripts").mkdir()
    source = b"""function Invoke-Hello {
    param($Name)
    Write-Host "Hello $Name"
}

Invoke-Hello -Name World
"""
    (repo / rel_path).write_bytes(source)

    py_nodes, py_edges = CodeParser().parse_bytes(Path(rel_path), source)
    payload = json.loads(parse_rust_owned_files_compact_json(repo, [rel_path]))

    assert payload["batch"][0][1] == [
        [
            node.kind,
            node.name,
            node.file_path,
            node.line_start,
            node.line_end,
            node.language,
            node.parent_name,
            node.params,
            node.return_type,
            node.modifiers,
            node.is_test,
            node.extra,
        ]
        for node in py_nodes
    ]
    assert payload["batch"][0][2] == [
        [edge.kind, edge.source, edge.target, edge.file_path, edge.line, edge.extra]
        for edge in py_edges
    ]
