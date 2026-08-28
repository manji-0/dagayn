"""Shared helpers for resolving bare-name graph edges with import context."""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass
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


def normalize_namespace(value: str) -> str:
    """Canonicalize a namespace path so `A\\B`, `A::B` and `A.B` compare equal."""
    normalized = value.replace("\\", ".").replace("::", ".")
    return ".".join(part for part in normalized.split(".") if part)


@dataclass(frozen=True)
class SymbolVisibility:
    """Indirect visibility between files: namespaces and declaring classes.

    Held as per-file maps rather than an expanded file-to-file product: a
    single namespace with N files would otherwise cost N^2 entries.
    """

    declared: dict[str, set[str]]
    """File -> namespaces it declares."""
    imported: dict[str, set[str]]
    """File -> namespaces its imports name."""
    class_files: dict[str, set[str]]
    """Class name -> files declaring that class."""

    def can_see(self, source_file: str, target_file: str) -> bool:
        """True when *source_file* reaches *target_file* without a file import.

        Either the two share a namespace -- C# and Java need no import
        statement then -- or the source imports one the target declares.
        """
        declared = self.declared.get(target_file)
        if not declared:
            return False
        if declared & self.declared.get(source_file, frozenset()):
            return True
        return bool(declared & self.imported.get(source_file, frozenset()))

    def declaring_files(self, target_qualified: str) -> set[str]:
        """Files declaring the class that owns *target_qualified*.

        A C++ method is defined in a `.cpp` that nobody includes, while its
        class is declared in the header that callers do include -- so the
        header, not the definition file, is what a caller can see.
        """
        symbol = target_qualified.partition("::")[2]
        owner = symbol.rpartition(".")[0]
        if not owner:
            return set()
        return self.class_files.get(owner, set())


def build_symbol_visibility(conn: Any) -> SymbolVisibility:
    """Read declared namespaces from File nodes and imported ones from edges.

    Parsers record the namespaces a file declares (C# ``namespace``,
    Java/Kotlin/Scala ``package``, PHP ``namespace``) on the File node.
    """
    declared: dict[str, set[str]] = {}
    for row in conn.execute(
        "SELECT file_path, extra FROM nodes WHERE kind = 'File' AND extra LIKE '%namespaces%'"
    ).fetchall():
        try:
            extra = json.loads(row["extra"] or "{}")
        except (json.JSONDecodeError, TypeError):
            continue
        namespaces = extra.get("namespaces")
        if not isinstance(namespaces, list):
            continue
        for namespace in namespaces:
            if not isinstance(namespace, str):
                continue
            key = normalize_namespace(namespace)
            if key:
                declared.setdefault(row["file_path"], set()).add(key)

    imported: dict[str, set[str]] = {}
    if declared:
        for row in conn.execute(
            "SELECT DISTINCT file_path, target_qualified FROM edges WHERE kind = 'IMPORTS_FROM'"
        ).fetchall():
            target = row["target_qualified"]
            if not is_namespace_candidate(target):
                continue
            key = normalize_namespace(target)
            if not key:
                continue
            entry = imported.setdefault(row["file_path"], set())
            entry.add(key)
            # `using A.B.Type` / `use A\B\Type` names a symbol inside `A.B`.
            parent = key.rpartition(".")[0]
            if parent:
                entry.add(parent)

    class_files: dict[str, set[str]] = {}
    for row in conn.execute("SELECT name, file_path FROM nodes WHERE kind = 'Class'").fetchall():
        class_files.setdefault(row["name"], set()).add(row["file_path"])
    return SymbolVisibility(declared=declared, imported=imported, class_files=class_files)


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


def is_namespace_candidate(target: str) -> bool:
    """True when *target* could name a namespace rather than a file.

    ``looks_like_file_target`` is too strict here: it treats a backslash as a
    directory separator, but PHP writes namespaces as ``App\\Util``.
    """
    if "/" in target:
        return False
    suffix = target.rpartition(".")[2].lower()
    return f".{suffix}" not in _FILE_TARGET_SUFFIXES


def is_plausible_bare_edge(
    source_file: str,
    target_file: str,
    import_targets: dict[str, set[str]],
    visibility: SymbolVisibility | None = None,
    target_qualified: str = "",
) -> bool:
    """Return True when *source_file* may refer to a symbol in *target_file*."""
    if not source_file or not target_file:
        return False
    if _file_is_visible(source_file, target_file, import_targets, visibility):
        return True
    if visibility is None or not target_qualified:
        return False
    # Reaching the class declaration is enough; the definition may live in a
    # file nobody imports directly.
    return any(
        _file_is_visible(source_file, declaring, import_targets, visibility)
        for declaring in visibility.declaring_files(target_qualified)
    )


def _file_is_visible(
    source_file: str,
    target_file: str,
    import_targets: dict[str, set[str]],
    visibility: SymbolVisibility | None,
) -> bool:
    if source_file == target_file:
        return True
    if target_file in import_targets.get(source_file, set()):
        return True
    return visibility is not None and visibility.can_see(source_file, target_file)


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
    visibility: SymbolVisibility | None = None,
) -> str | None:
    imported = [
        qn
        for qn in candidates
        if is_plausible_bare_edge(
            source_file,
            node_file_from_qualified(qn),
            import_targets,
            visibility,
            qn,
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
    visibility = build_symbol_visibility(conn)
    resolved = 0
    for edge in bare_edges:
        bare_name = edge["target_qualified"]
        candidates = _bare_name_candidates(conn, bare_name)
        if not candidates:
            continue

        src_file = node_file_from_qualified(edge["source_qualified"], edge["file_path"])
        qualified = _resolve_via_imports(candidates, src_file, import_targets, visibility)
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
    visibility = build_symbol_visibility(conn)
    resolved = 0
    demoted = 0
    for edge in bare_edges:
        bare_name = edge["target_qualified"]
        candidates = _bare_name_candidates(conn, bare_name, kinds=("Class",))
        src_file = node_file_from_qualified(edge["source_qualified"], edge["file_path"])
        qualified = _resolve_via_imports(candidates, src_file, import_targets, visibility)

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
