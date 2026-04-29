"""Dead code detection for graph-powered refactoring."""

from __future__ import annotations

import functools
import logging
import re
from typing import Any, Optional

from ..flows import _has_framework_decorator, _matches_entry_name
from ..graph import GraphStore

logger = logging.getLogger(__name__)

_FRAMEWORK_BASE_CLASSES = frozenset(
    {
        "Base",
        "DeclarativeBase",
        "Model",
        "BaseModel",
        "BaseSettings",
        "db.Model",
        "TableBase",
        "Stack",
        "NestedStack",
        "Construct",
        "Resource",
    }
)

_CDK_CLASS_SUFFIXES = ("Stack", "Construct", "Pipeline", "Resources", "Layer")

_MOCK_NAME_RE = re.compile(
    r"^(mock[A-Z_]|Mock[A-Z]|createMock[A-Z])|"
    r"(Mock|Stub|Fake|Spy)$",
    re.IGNORECASE,
)

_TEST_FILE_RE = re.compile(
    r"([\\/]__tests__[\\/]|\.spec\.[jt]sx?$|\.test\.[jt]sx?$|[\\/]test_[^/\\]*\.py$"
    r"|[\\/]e2e[_-]?tests?[\\/]|[\\/]test[_-]utils?[\\/])",
)


def _is_test_file(file_path: str) -> bool:
    return bool(_TEST_FILE_RE.search(file_path))


_MIN_PKG_SEGMENT_LEN = 4


@functools.lru_cache(maxsize=4096)
def _path_segments(file_path: str) -> tuple[str, ...]:
    parts = file_path.replace("\\", "/").split("/")
    return tuple(
        p
        for p in parts[:-1]
        if len(p) >= _MIN_PKG_SEGMENT_LEN and p not in ("home", "src", "lib", "app")
    )


_TYPE_IDENT_RE = re.compile(r"[A-Z][A-Za-z0-9_]*")


def _collect_type_referenced_names(store: GraphStore) -> set[str]:
    funcs = store.get_nodes_by_kind(kinds=["Function", "Test"])
    names: set[str] = set()
    for f in funcs:
        for text in (f.params, f.return_type):
            if text:
                names.update(_TYPE_IDENT_RE.findall(text))
    return names


def _is_entry_point(node: Any) -> bool:
    if _has_framework_decorator(node):
        return True
    if _matches_entry_name(node):
        return True
    return False


def _is_plausible_caller(
    edge_file: str,
    node_file: str,
    node_name: str,
    importer_files: dict[str, set[str]],
    name_counts: dict[str, int],
) -> bool:
    """A bare-name edge is plausible if it comes from the same file,
    from a file that has an IMPORTS_FROM edge whose target matches
    the node's file path, or the name is globally unique (no ambiguity)."""
    if edge_file == node_file:
        return True
    if node_name and name_counts.get(node_name, 0) == 1:
        return True
    for imp_target in importer_files.get(edge_file, ()):
        imp_path = imp_target.split("::")[0] if "::" in imp_target else imp_target
        if imp_path.endswith("/__init__.py"):
            imp_dir = imp_path[:-12]
            if node_file.startswith(imp_dir + "/"):
                return True
        if imp_path.startswith(node_file) or node_file.startswith(imp_path + "/"):
            return True
        for imp2 in importer_files.get(imp_target, ()):
            imp2_path = imp2.split("::")[0] if "::" in imp2 else imp2
            if imp2_path.endswith("/__init__.py"):
                imp2_dir = imp2_path[:-12]
                if node_file.startswith(imp2_dir + "/"):
                    return True
            if imp2_path.startswith(node_file) or node_file.startswith(imp2_path + "/"):
                return True
        if not imp_target.startswith("/"):
            for seg in _path_segments(node_file):
                if seg in imp_target:
                    return True
    return False


def find_dead_code(
    store: GraphStore,
    kind: Optional[str] = None,
    file_pattern: Optional[str] = None,
) -> list[dict[str, Any]]:
    """Find functions/classes with no callers, no test refs, no importers, and no references.

    Entry points (functions matching framework decorators or conventional name
    patterns like ``main``, ``test_*``, ``handle_*``) are excluded.

    .. note::

        **Caveats — dynamic dispatch patterns.**  Static analysis cannot track
        all runtime-determined call patterns.  Functions registered via fully
        dynamic keys (``map[computedKey()] = fn``), ``Reflect.apply``, or
        runtime ``require()`` may still appear as dead code.  Treat results as
        hints, especially for TypeScript projects that use map-based dispatch,
        plugin registries, or dynamic requires.

    Args:
        store: The GraphStore instance.
        kind: Optional filter (e.g. ``"Function"`` or ``"Class"``).
        file_pattern: Optional file-path substring filter.

    Returns:
        List of dead-code dicts with name, qualified_name, kind, file, line,
        and a top-level ``caveats`` note.
    """
    from ..graph import _sanitize_name

    candidates = store.get_nodes_by_kind(
        kinds=[kind] if kind else ["Function", "Class"],
        file_pattern=file_pattern,
    )

    type_ref_names = _collect_type_referenced_names(store)

    class_bases: dict[str, list[str]] = {}
    conn = store._conn
    for row in conn.execute(
        "SELECT source_qualified, target_qualified FROM edges WHERE kind = 'INHERITS'"
    ).fetchall():
        base = row[1].rsplit("::", 1)[-1] if "::" in row[1] else row[1]
        class_bases.setdefault(row[0], []).append(base)

    importer_files: dict[str, set[str]] = {}
    for row in conn.execute(
        "SELECT file_path, target_qualified FROM edges WHERE kind = 'IMPORTS_FROM'"
    ).fetchall():
        importer_files.setdefault(row[0], set()).add(row[1])

    name_counts: dict[str, int] = {}
    for row in conn.execute(
        "SELECT name, COUNT(*) FROM nodes "
        "WHERE kind IN ('Function', 'Class') AND is_test = 0 "
        "GROUP BY name"
    ).fetchall():
        name_counts[row[0]] = row[1]

    dead: list[dict[str, Any]] = []

    for node in candidates:
        if node.is_test or _is_test_file(node.file_path):
            continue

        if node.file_path.endswith(".d.ts"):
            continue

        if node.name.startswith("__") and node.name.endswith("__"):
            continue

        if node.name == "constructor" and node.parent_name:
            continue

        if node.is_test or _is_test_file(node.file_path):
            if _MOCK_NAME_RE.search(node.name):
                continue

        if _is_entry_point(node):
            continue

        if node.kind == "Class" and node.name in type_ref_names:
            continue

        if node.kind == "Class" and _has_framework_decorator(node):
            continue

        _is_framework_class = False
        _check_qn = (
            node.qualified_name
            if node.kind == "Class"
            else (node.qualified_name.rsplit(".", 1)[0] if node.parent_name else None)
        )
        if _check_qn:
            outgoing = store.get_edges_by_source(_check_qn)
            base_names = {
                e.target_qualified.rsplit("::", 1)[-1] for e in outgoing if e.kind == "INHERITS"
            }
            if base_names & _FRAMEWORK_BASE_CLASSES:
                _is_framework_class = True
        if node.kind == "Class":
            if _is_framework_class:
                continue
            if any(node.name.endswith(s) for s in _CDK_CLASS_SUFFIXES):
                continue
        if node.kind == "Function" and _is_framework_class:
            continue
        if (
            node.kind == "Function"
            and node.parent_name
            and any(node.parent_name.endswith(s) for s in _CDK_CLASS_SUFFIXES)
        ):
            continue

        decorators = node.extra.get("decorators", ())
        if isinstance(decorators, (list, tuple)) and decorators:
            if node.kind in ("Function", "Test"):
                if any(
                    d in ("property", "abstractmethod", "classmethod", "staticmethod")
                    or d.endswith(".abstractmethod")
                    or d.startswith("HostListener")
                    for d in decorators
                ):
                    continue
            if node.kind == "Class":
                if any("dataclass" in d for d in decorators):
                    continue

        if node.kind == "Function" and node.parent_name:
            parent_qn = node.qualified_name.rsplit(".", 1)[0]
            parent_edges = store.get_edges_by_source(parent_qn)
            base_class_names = [e.target_qualified for e in parent_edges if e.kind == "INHERITS"]
            for base_name in base_class_names:
                base_method_qn = f"{base_name}.{node.name}"
                base_nodes = store.get_node(base_method_qn)
                if base_nodes is None:
                    base_method_qn2 = node.file_path + "::" + base_name + "." + node.name
                    base_nodes = store.get_node(base_method_qn2)
                if base_nodes is not None:
                    base_decos = base_nodes.extra.get("decorators", ())
                    if isinstance(base_decos, (list, tuple)) and any(
                        "abstractmethod" in d for d in base_decos
                    ):
                        break
            else:
                base_name = None
            if base_name is not None:
                continue

        incoming = store.get_edges_by_target(node.qualified_name)
        if not any(e.kind == "CALLS" for e in incoming) and node.parent_name:
            class_qn = f"{node.parent_name}::{node.name}"
            incoming = incoming + store.get_edges_by_target(class_qn)
        if not any(e.kind == "CALLS" for e in incoming):
            bare = store.search_edges_by_target_name(node.name, kind="CALLS")
            suffix_rows = conn.execute(
                "SELECT * FROM edges WHERE kind = 'CALLS' AND target_qualified LIKE ?",
                (f"%::{node.name}",),
            ).fetchall()
            suffix_edges = [store._row_to_edge(r) for r in suffix_rows]
            all_bare = bare + suffix_edges
            all_bare = [
                e
                for e in all_bare
                if _is_plausible_caller(
                    e.file_path, node.file_path, node.name, importer_files, name_counts
                )
            ]
            incoming = incoming + all_bare
        if not any(e.kind == "TESTED_BY" for e in incoming):
            bare_tb = store.search_edges_by_target_name(node.name, kind="TESTED_BY")
            bare_tb = [
                e
                for e in bare_tb
                if _is_plausible_caller(
                    e.file_path, node.file_path, node.name, importer_files, name_counts
                )
            ]
            incoming = incoming + bare_tb
        if node.kind == "Class" and not any(e.kind == "INHERITS" for e in incoming):
            bare_inh = store.search_edges_by_target_name(node.name, kind="INHERITS")
            incoming = incoming + bare_inh
        has_callers = any(e.kind == "CALLS" for e in incoming)
        has_test_refs = any(e.kind == "TESTED_BY" for e in incoming)
        has_importers = any(e.kind == "IMPORTS_FROM" for e in incoming)
        has_references = any(e.kind == "REFERENCES" for e in incoming)
        has_subclasses = any(e.kind == "INHERITS" for e in incoming)

        no_refs = not (
            has_callers or has_test_refs or has_importers or has_references or has_subclasses
        )
        if node.kind == "Class" and no_refs:
            member_prefix = node.qualified_name + "."
            bare_prefix = node.name + "."
            member_calls = conn.execute(
                "SELECT COUNT(*) FROM edges WHERE kind = 'CALLS'"
                " AND (target_qualified LIKE ? OR target_qualified LIKE ?)",
                (f"%{member_prefix}%", f"%{bare_prefix}%"),
            ).fetchone()[0]
            if member_calls > 0:
                has_callers = True

        if not (has_callers or has_test_refs or has_importers or has_references or has_subclasses):
            if node.kind == "Function" and node.parent_name and not has_callers:
                method_suffix = "." + node.name
                if node.qualified_name.endswith(method_suffix):
                    class_qn = node.qualified_name[: -len(method_suffix)]
                    for base_name in class_bases.get(class_qn, []):
                        rows = conn.execute(
                            "SELECT n.qualified_name FROM nodes n "
                            "WHERE n.parent_name = ? AND n.name = ? "
                            "AND n.kind IN ('Function', 'Test')",
                            (base_name, node.name),
                        ).fetchall()
                        for (base_method_qn,) in rows:
                            if conn.execute(
                                "SELECT 1 FROM edges "
                                "WHERE target_qualified = ? AND kind = 'CALLS' "
                                "LIMIT 1",
                                (base_method_qn,),
                            ).fetchone():
                                has_callers = True
                                break
                        if has_callers:
                            break

            if not has_callers:
                dead.append(
                    {
                        "name": _sanitize_name(node.name),
                        "qualified_name": _sanitize_name(node.qualified_name),
                        "kind": node.kind,
                        "file": node.file_path,
                        "line": node.line_start,
                    }
                )

    logger.info("find_dead_code: found %d dead symbols", len(dead))
    return dead
