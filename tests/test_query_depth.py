"""Tests for ``query_graph`` depth walks and compact rows."""

from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.tools import query as query_module


@pytest.fixture
def store(tmp_path):
    """A repository whose graph names files under ``/repo``; the graph is
    closed before the tools read it, and each write opens it again."""
    root = tmp_path / "repo"
    (root / ".git").mkdir(parents=True)
    (root / ".dagayn").mkdir()
    db = root / ".dagayn" / "graph.db"
    graph = GraphStore(str(db))
    # Imports: b -> a, c -> b, d -> c, and d -> b closes a second path to b.
    for name in ("a", "b", "c", "d"):
        graph.upsert_node(
            NodeInfo(
                kind="File",
                name=f"/repo/{name}.py",
                file_path=f"/repo/{name}.py",
                line_start=1,
                line_end=20,
                language="python",
            )
        )
        graph.upsert_node(
            NodeInfo(
                kind="Function",
                name=f"f_{name}",
                file_path=f"/repo/{name}.py",
                line_start=2,
                line_end=5,
                language="python",
            )
        )
    for importer, imported, line in (("b", "a", 1), ("c", "b", 1), ("d", "c", 1), ("d", "b", 2)):
        graph.upsert_edge(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source=f"/repo/{importer}.py",
                target=f"/repo/{imported}.py",
                file_path=f"/repo/{importer}.py",
                line=line,
            )
        )
    # Calls: f_b -> f_a, f_c -> f_b, f_d -> f_c.
    for caller, callee in (("b", "a"), ("c", "b"), ("d", "c")):
        graph.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source=f"/repo/{caller}.py::f_{caller}",
                target=f"/repo/{callee}.py::f_{callee}",
                file_path=f"/repo/{caller}.py",
                line=3,
            )
        )
    graph.commit()
    graph.close()
    return root


def _query(root: Path, **kwargs):
    return query_module.query_graph(repo_root=str(root), **kwargs)


def _write(root: Path) -> GraphStore:
    return GraphStore(str(root / ".dagayn" / "graph.db"))


def test_depth_one_keeps_direct_rows_without_walk_fields(store):
    result = _query(store, pattern="importers_of", target="/repo/a.py")

    assert result["status"] == "ok"
    assert [row["file"] for row in result["results"]] == ["/repo/b.py"]
    assert "reachability" not in result
    assert "depth" not in result["results"][0]


def test_importers_of_walks_to_the_fixed_point(store):
    result = _query(store, pattern="importers_of", target="/repo/a.py", depth=6)

    rows = {row["file"]: row for row in result["results"]}
    assert set(rows) == {"/repo/b.py", "/repo/c.py", "/repo/d.py"}
    assert rows["/repo/b.py"]["depth"] == 1
    assert (rows["/repo/c.py"]["depth"], rows["/repo/c.py"]["via"]) == (2, "/repo/b.py")
    assert (rows["/repo/d.py"]["depth"], rows["/repo/d.py"]["via"]) == (2, "/repo/b.py")
    assert result["depth"] == 6
    assert result["reachability"]["state"] == "complete"
    assert result["reachability"]["depth_limit_reached"] is False
    assert "within 6 hops" in result["summary"]
    # Closed: nothing deeper to ask for.
    assert all(call["args"].get("depth") is None for call in result["next"])


def test_node_reached_by_two_paths_appears_once_at_its_shortest_hop(store):
    result = _query(store, pattern="importers_of", target="/repo/b.py", depth=3)

    files = [row["file"] for row in result["results"]]
    assert sorted(files) == ["/repo/c.py", "/repo/d.py"]
    assert {row["file"]: row["depth"] for row in result["results"]}["/repo/d.py"] == 1


def test_depth_limit_is_reported(store):
    result = _query(store, pattern="callers_of", target="/repo/a.py::f_a", depth=2)

    assert {row["qualified_name"] for row in result["results"]} == {
        "/repo/b.py::f_b",
        "/repo/c.py::f_c",
    }
    assert result["reachability"]["depth_limit_reached"] is True
    assert result["next"][0]["args"] == {
        "depth": 6,
        "pattern": "callers_of",
        "target": "/repo/a.py::f_a",
    }


def test_callers_of_walks_call_chains(store):
    result = _query(store, pattern="callers_of", target="/repo/a.py::f_a", depth=3)

    rows = {row["qualified_name"]: row for row in result["results"]}
    assert set(rows) == {"/repo/b.py::f_b", "/repo/c.py::f_c", "/repo/d.py::f_d"}
    assert rows["/repo/d.py::f_d"]["via"] == "/repo/c.py::f_c"
    assert result["reachability"]["nodes_visited"] == 3


def test_depth_is_capped(store):
    result = _query(store, pattern="callers_of", target="/repo/a.py::f_a", depth=99)

    assert result["depth"] == 6


@pytest.mark.parametrize(
    ("pattern", "depth"),
    [("callees_of", 2), ("children_of", 3), ("callers_of", 0)],
)
def test_invalid_depth_is_an_error(store, pattern, depth):
    result = _query(store, pattern=pattern, target="/repo/a.py::f_a", depth=depth)

    assert result["status"] == "error"
    assert "depth" in result["error"]


def _add_call(root, caller, callee, line):
    store = _write(root)
    store.upsert_edge(
        EdgeInfo(
            kind="CALLS",
            source=f"/repo/{caller}.py::f_{caller}",
            target=f"/repo/{callee}.py::f_{callee}",
            file_path=f"/repo/{caller}.py",
            line=line,
        )
    )
    store.commit()
    store.close()


def test_standard_folds_edges_into_rows(store):
    _add_call(store, "b", "a", 7)

    standard = _query(store, pattern="callers_of", target="/repo/a.py::f_a")
    full = _query(store, pattern="callers_of", target="/repo/a.py::f_a", detail_level="full")

    assert "edges" not in standard
    assert "_hints" not in standard
    assert standard["results"] == [
        {
            "kind": "Function",
            "name": "f_b",
            "qualified_name": "/repo/b.py::f_b",
            "line_start": 2,
            "line_end": 5,
            "lines": [3, 7],
            "confidence_tier": "EXTRACTED",
        }
    ]
    assert standard["result_count"] == 1
    assert len(full["results"]) == 2
    assert len(full["edges"]) == 2
    assert "counts" in full["answerability"]
    assert "answerability" not in standard


def test_minimal_returns_every_row_that_fits(store):
    graph = _write(store)
    for index in range(8):
        name = f"/repo/extra_{index}.py"
        graph.upsert_node(
            NodeInfo(
                kind="File",
                name=name,
                file_path=name,
                line_start=1,
                line_end=5,
                language="python",
            )
        )
        graph.upsert_edge(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source=name,
                target="/repo/a.py",
                file_path=name,
                line=1,
            )
        )
    graph.commit()
    graph.close()

    result = _query(store, pattern="importers_of", target="/repo/a.py", detail_level="minimal")

    assert result["result_count"] == 9
    assert len(result["results"]) == 9
    assert result["results_complete"] is True
    assert "guidance" not in result
    assert result["results"][0] == {
        "file": "/repo/b.py",
        "lines": [1],
        "confidence_tier": "EXTRACTED",
        "evidence_type": "extracted",
    }
