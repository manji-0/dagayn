"""Shared helpers for resolving bare-name graph edges with import context."""

from __future__ import annotations

import json
import logging
from typing import Any

from .graph._sql import _edge_target_name

logger = logging.getLogger(__name__)

_FILE_TARGET_SUFFIXES = (
    ".md",
    ".markdown",
    ".py",
    ".tf",
    ".tfvars",
    ".rs",
    ".js",
    ".ts",
    ".tsx",
    ".jsx",
    ".java",
    ".go",
    ".rb",
    ".php",
    ".cs",
    ".cpp",
    ".hpp",
    ".c",
    ".h",
    ".swift",
    ".kt",
    ".scala",
    ".dart",
    ".ipynb",
)

_INFERRED_CONFIDENCE = 0.6
_INFERRED_TIER = "MEDIUM"
_UNRESOLVED_TIER = "LOW"


def looks_like_file_target(target: str) -> bool:
    """Return True when *target* looks like a file path rather than a symbol name."""
    path = target.split("::", 1)[0]
    if "/" in path or "\\" in path:
        return True
    lower = path.lower()
    return any(lower.endswith(suffix) for suffix in _FILE_TARGET_SUFFIXES)


def node_file_from_qualified(qualified: str, fallback_file: str = "") -> str:
    """Extract the file path prefix from a qualified node name."""
    if "::" in qualified:
        return qualified.split("::", 1)[0]
    return fallback_file


def build_import_targets(conn: Any) -> dict[str, set[str]]:
    """Map source file paths to imported file paths (from IMPORTS_FROM edges)."""
    import_targets: dict[str, set[str]] = {}
    for row in conn.execute(
        "SELECT DISTINCT file_path, target_qualified FROM edges WHERE kind = 'IMPORTS_FROM'"
    ).fetchall():
        target = row["target_qualified"]
        target_file = target.split("::", 1)[0] if "::" in target else target
        import_targets.setdefault(row["file_path"], set()).add(target_file)
    return import_targets


def is_plausible_bare_edge(
    source_file: str,
    target_file: str,
    import_targets: dict[str, set[str]],
) -> bool:
    """Return True when *source_file* may refer to a symbol in *target_file*."""
    if not source_file or not target_file:
        return False
    if source_file == target_file:
        return True
    return target_file in import_targets.get(source_file, set())


def _bare_name_candidates(
    conn: Any,
    bare_name: str,
    *,
    kinds: tuple[str, ...] = ("Function", "Test", "Class"),
) -> list[str]:
    placeholders = ",".join("?" for _ in kinds)
    rows = conn.execute(
        f"SELECT qualified_name FROM nodes WHERE name = ? AND kind IN ({placeholders})",
        (bare_name, *kinds),
    ).fetchall()
    return [row["qualified_name"] for row in rows]


def _resolve_via_imports(
    candidates: list[str],
    source_file: str,
    import_targets: dict[str, set[str]],
) -> str | None:
    imported = [
        qn
        for qn in candidates
        if is_plausible_bare_edge(
            source_file,
            node_file_from_qualified(qn),
            import_targets,
        )
    ]
    if len(imported) == 1:
        return imported[0]
    return None


def resolve_bare_call_targets(store: Any) -> int:
    """Resolve bare-name CALLS targets using import-aware disambiguation."""
    conn = store._conn
    bare_edges = conn.execute(
        "SELECT id, source_qualified, target_qualified, file_path "
        "FROM edges WHERE kind = 'CALLS' AND target_qualified NOT LIKE '%::%'"
    ).fetchall()
    if not bare_edges:
        return 0

    import_targets = build_import_targets(conn)
    resolved = 0
    for edge in bare_edges:
        bare_name = edge["target_qualified"]
        candidates = _bare_name_candidates(conn, bare_name)
        if not candidates:
            continue

        src_file = node_file_from_qualified(edge["source_qualified"], edge["file_path"])
        qualified = _resolve_via_imports(candidates, src_file, import_targets)
        if qualified is None:
            continue

        conn.execute(
            "UPDATE edges SET target_qualified = ?, target_name = ?, "
            "confidence = ?, confidence_tier = ? WHERE id = ?",
            (
                qualified,
                _edge_target_name(qualified),
                _INFERRED_CONFIDENCE,
                _INFERRED_TIER,
                edge["id"],
            ),
        )
        resolved += 1

    if resolved:
        conn.commit()
        logger.info("Resolved %d bare-name CALLS targets", resolved)
    return resolved


def resolve_bare_inheritance_targets(store: Any) -> int:
    """Resolve bare-name INHERITS/IMPLEMENTS targets using import context."""
    conn = store._conn
    bare_edges = conn.execute(
        "SELECT id, source_qualified, target_qualified, file_path, extra "
        "FROM edges WHERE kind IN ('INHERITS', 'IMPLEMENTS') "
        "AND target_qualified NOT LIKE '%::%'"
    ).fetchall()
    if not bare_edges:
        return 0

    import_targets = build_import_targets(conn)
    resolved = 0
    demoted = 0
    for edge in bare_edges:
        bare_name = edge["target_qualified"]
        candidates = _bare_name_candidates(conn, bare_name, kinds=("Class",))
        src_file = node_file_from_qualified(edge["source_qualified"], edge["file_path"])
        qualified = _resolve_via_imports(candidates, src_file, import_targets)

        if qualified is not None:
            conn.execute(
                "UPDATE edges SET target_qualified = ?, target_name = ?, "
                "confidence = ?, confidence_tier = ? WHERE id = ?",
                (
                    qualified,
                    _edge_target_name(qualified),
                    _INFERRED_CONFIDENCE,
                    _INFERRED_TIER,
                    edge["id"],
                ),
            )
            resolved += 1
            continue

        try:
            extra = json.loads(edge["extra"] or "{}")
        except (json.JSONDecodeError, TypeError):
            extra = {}
        if extra.get("bare_name_unresolved"):
            continue

        extra["bare_name_unresolved"] = True
        conn.execute(
            "UPDATE edges SET extra = ?, confidence = ?, confidence_tier = ? WHERE id = ?",
            (
                json.dumps(extra),
                0.3,
                _UNRESOLVED_TIER,
                edge["id"],
            ),
        )
        demoted += 1

    if resolved or demoted:
        conn.commit()
        logger.info(
            "Resolved %d bare-name inheritance targets; demoted %d unresolved",
            resolved,
            demoted,
        )
    return resolved
