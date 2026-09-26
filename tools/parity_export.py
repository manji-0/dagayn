"""Emit a canonical JSON snapshot of a dagayn graph DB for parity testing.

Usage:
    uv run python tools/parity_export.py <repo_dir> --out <snapshot.json>
    uv run python tools/parity_export.py <repo_dir> --stdout
    uv run python tools/parity_export.py <repo_dir> --check-determinism

    uv run python tools/parity_export.py --regenerate typescript javascript

``--entity-lines`` writes one node / edge per line (the format of the
TypeScript and JavaScript snapshots), so parser changes show up as reviewable
line diffs. ``--regenerate`` builds the named fixtures under
``tests/fixtures/parity/`` in a temporary directory and rewrites their
committed snapshots in the fixture's own format.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))

from dagayn.graph.helpers import _sanitize_name
from dagayn.migrations import LATEST_VERSION


def _canon_extra(value: object) -> object:
    """Recursively sort dict keys alphabetically; lists preserve element order."""
    if isinstance(value, dict):
        return {k: _canon_extra(v) for k, v in sorted(value.items())}
    if isinstance(value, list):
        return [_canon_extra(item) for item in value]
    return value


def _node_row(row) -> dict:
    keys = row.keys()
    extra = json.loads(row["extra"]) if row["extra"] else {}
    # File nodes store the absolute path in `name` because _relativize_parsed_entities
    # normalizes `file_path` but not `name`.  Use `file_path` (always repo-relative)
    # so the canonical export is independent of the build working directory.
    name = row["file_path"] if row["kind"] == "File" else _sanitize_name(row["name"])
    return {
        "kind": row["kind"],
        "name": name,
        "qualified_name": _sanitize_name(row["qualified_name"]),
        "file_path": row["file_path"],
        "line_start": row["line_start"],
        "line_end": row["line_end"],
        "language": row["language"],
        "parent_name": _sanitize_name(row["parent_name"]) if row["parent_name"] else None,
        "params": row["params"],
        "return_type": row["return_type"],
        "modifiers": row["modifiers"],
        "is_test": bool(row["is_test"]),
        "file_hash": row["file_hash"],
        "signature": row["signature"] if "signature" in keys else None,
        "community_id": row["community_id"] if "community_id" in keys else None,
        "extra": _canon_extra(extra),
    }


def _edge_row(row) -> dict:
    extra = json.loads(row["extra"]) if row["extra"] else {}
    return {
        "kind": row["kind"],
        "source": _sanitize_name(row["source_qualified"]),
        "target": _sanitize_name(row["target_qualified"]),
        "file_path": row["file_path"],
        "line": row["line"],
        "confidence": row["confidence"],
        "confidence_tier": row["confidence_tier"],
        "extra": _canon_extra(extra),
    }


def _dumps(value: object) -> str:
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def _entity_lines(snapshot: dict) -> str:
    """Canonical JSON with one node / edge per line (same data as the compact form)."""
    lines = ["{"]
    for key in ("edges", "nodes"):
        lines.append(f"{_dumps(key)}:[")
        items = snapshot[key]
        last = len(items) - 1
        lines.extend(
            _dumps(item) + ("," if index < last else "") for index, item in enumerate(items)
        )
        lines.append("],")
    lines.append(f"{_dumps('schema_version')}:{_dumps(snapshot['schema_version'])}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def export_db(db_path: Path, *, entity_lines: bool = False) -> str:
    """Return a deterministic canonical JSON snapshot of the graph at db_path.

    *entity_lines* puts each node and edge on its own line; the content is
    identical to the compact single-line form.
    """
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    try:
        node_rows = conn.execute("SELECT * FROM nodes").fetchall()
        edge_rows = conn.execute("SELECT * FROM edges").fetchall()
    finally:
        conn.close()

    nodes = sorted(
        [_node_row(r) for r in node_rows],
        key=lambda n: n["qualified_name"],
    )
    edges = sorted(
        [_edge_row(r) for r in edge_rows],
        key=lambda e: (e["kind"], e["source"], e["target"], e["file_path"], e["line"]),
    )

    snapshot = {
        "schema_version": LATEST_VERSION,
        "nodes": nodes,
        "edges": edges,
    }
    if entity_lines:
        return _entity_lines(snapshot)
    return _dumps(snapshot) + "\n"


def regenerate(names: list[str]) -> None:
    """Build parity fixtures in a scratch directory and rewrite their snapshots."""
    import tempfile

    from tests.conftest import (
        ENTITY_LINE_PARITY_FIXTURES,
        PARITY_FIXTURE_DIR,
        build_parity_fixture,
    )

    for name in names:
        with tempfile.TemporaryDirectory() as scratch:
            db_path = build_parity_fixture(PARITY_FIXTURE_DIR / name, Path(scratch))
            snapshot = export_db(db_path, entity_lines=name in ENTITY_LINE_PARITY_FIXTURES)
        out = PARITY_FIXTURE_DIR / "__snapshots__" / f"{name}.json"
        out.write_text(snapshot, encoding="utf-8")
        print(f"Wrote {out} ({len(snapshot)} bytes)")


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "--regenerate":
        regenerate(sys.argv[2:])
        return
    parser = argparse.ArgumentParser(
        description="Export a canonical dagayn graph snapshot for Rust parity testing."
    )
    parser.add_argument(
        "repo_dir",
        type=Path,
        help="Repository root that has already been built (must have .dagayn/graph.db)",
    )
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--out", type=Path, help="Write snapshot to this file")
    group.add_argument("--stdout", action="store_true", help="Print snapshot to stdout")
    group.add_argument(
        "--check-determinism",
        action="store_true",
        help="Export the DB twice and assert the SHA256 is identical (tests serialization)",
    )
    parser.add_argument(
        "--entity-lines",
        action="store_true",
        help="Write one node / edge per line (TypeScript / JavaScript snapshots)",
    )
    args = parser.parse_args()

    db_path = args.repo_dir / ".dagayn" / "graph.db"
    if not db_path.exists():
        sys.exit(f"No graph DB at {db_path}. Run 'dagayn build' in that directory first.")

    if args.out:
        snapshot = export_db(db_path, entity_lines=args.entity_lines)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(snapshot, encoding="utf-8")
        sha = hashlib.sha256(snapshot.encode()).hexdigest()
        print(f"Wrote {args.out} ({len(snapshot)} bytes, SHA256: {sha})")
    elif args.stdout:
        print(export_db(db_path, entity_lines=args.entity_lines), end="")
    elif args.check_determinism:
        first = export_db(db_path)
        second = export_db(db_path)
        sha1 = hashlib.sha256(first.encode()).hexdigest()
        sha2 = hashlib.sha256(second.encode()).hexdigest()
        if sha1 != sha2:
            sys.exit(f"FAIL: SHA256 mismatch\n  first:  {sha1}\n  second: {sha2}")
        print(f"OK: serialization is deterministic (SHA256: {sha1})")


if __name__ == "__main__":
    main()
