"""Rust backend parity tests for Rust-owned parser paths."""

from __future__ import annotations

import json
import os
import shutil
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
from dagayn.parser import CodeParser

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


def test_rust_backend_is_default_when_extension_is_available(monkeypatch):
    """DAGAYN_BACKEND defaults to Rust when the native extension can be loaded."""
    monkeypatch.delenv("DAGAYN_BACKEND", raising=False)
    monkeypatch.setattr("dagayn.incremental._rust_backend_available", lambda: True)

    assert _rust_backend_enabled() is True


def test_python_backend_can_be_forced(monkeypatch):
    monkeypatch.setenv("DAGAYN_BACKEND", "python")
    monkeypatch.setattr("dagayn.incremental._rust_backend_available", lambda: True)

    assert _rust_backend_enabled() is False


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

    assert result["errors"] == []
    assert result["files_parsed"] == 1


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
        pytest.skip(f"Rust extension is not available: {exc}")

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


def test_rust_backend_incremental_touch_updates_mtime_without_reparse(
    tmp_path_factory, monkeypatch
):
    """Rust-owned unchanged content should not cross back into Python parsing."""
    try:
        from dagayn._core import GraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")

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
        assert result["total_nodes"] == 0
        assert result["total_edges"] == 0
        assert result["errors"] == []
        assert store.get_file_meta_map()["api.md"][1] == new_mtime_ns
    finally:
        store.close()


def test_rust_backend_routes_databricks_py_exports(tmp_path, monkeypatch):
    """Databricks .py exports stay in the Rust-owned backend path."""
    try:
        from dagayn._core import GraphStore
    except ImportError as exc:
        pytest.skip(f"Rust extension is not available: {exc}")

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

    assert result["errors"] == []
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
        pytest.skip(f"Rust extension is not available: {exc}")

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
        pytest.skip(f"Rust extension is not available: {exc}")

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
        pytest.skip(f"Rust extension is not available: {exc}")

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
        pytest.skip(f"Rust extension is not available: {exc}")

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
        pytest.skip(f"Rust extension is not available: {exc}")

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
        pytest.skip(f"Rust extension is not available: {exc}")

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
        pytest.skip(f"Rust extension is not available: {exc}")

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
