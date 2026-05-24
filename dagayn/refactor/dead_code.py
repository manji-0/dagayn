"""Dead code detection for graph-powered refactoring."""

from __future__ import annotations

import functools
import logging
import re
from typing import Any, Optional

from ..flows import _has_framework_decorator, _matches_entry_name
from ..graph import GraphStore, _sanitize_name

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
_STRUCTURAL_CLASS_ROLES = frozenset(
    {"interface", "trait", "abstract_class", "abstract_type", "implementation"}
)
_VALUE_CONTAINER_CLASS_ROLES = frozenset({"struct", "enum"})
_VALUE_CONTAINER_DERIVE_TRAITS = frozenset({"Serialize", "Deserialize"})


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


def _is_structural_type_node(node: Any) -> bool:
    if node.kind != "Class":
        return False
    extra = node.extra if isinstance(node.extra, dict) else {}
    role = extra.get("type_role")
    if role in _STRUCTURAL_CLASS_ROLES:
        return True
    if extra.get("is_contract") or extra.get("is_abstract"):
        return True
    if node.language == "python":
        file_path = node.file_path.replace("\\", "/")
        if node.name.endswith("Protocol") or file_path.endswith("/_protocol.py"):
            return True
    return False


def _dead_code_confidence(node: Any) -> str:
    extra = node.extra if isinstance(node.extra, dict) else {}
    role = extra.get("type_role")
    if role in _VALUE_CONTAINER_CLASS_ROLES:
        return "low"
    derive_traits = extra.get("derive_traits")
    if isinstance(derive_traits, (list, tuple, set)) and any(
        trait in _VALUE_CONTAINER_DERIVE_TRAITS for trait in derive_traits
    ):
        return "low"
    return "medium"


def _survives_dead_code_node_filters(
    node: Any,
    type_ref_names: set[str],
    class_bases: dict[str, list[str]],
) -> bool:
    if node.language == "markdown":
        return False
    if node.is_test or _is_test_file(node.file_path):
        return False
    if node.language == "rust" and node.parent_name and "tests" in node.parent_name.split("::"):
        return False
    if node.file_path.endswith(".d.ts"):
        return False
    if node.name.startswith("__") and node.name.endswith("__"):
        return False
    if node.name == "constructor" and node.parent_name:
        return False
    if _is_entry_point(node):
        return False
    if node.kind == "Class" and node.name in type_ref_names:
        return False
    if node.kind == "Class" and _has_framework_decorator(node):
        return False
    if _is_structural_type_node(node):
        return False

    check_qn = (
        node.qualified_name
        if node.kind == "Class"
        else (node.qualified_name.rsplit(".", 1)[0] if node.parent_name else None)
    )
    is_framework_class = bool(
        check_qn and set(class_bases.get(check_qn, [])) & _FRAMEWORK_BASE_CLASSES
    )
    if node.kind == "Class":
        if is_framework_class:
            return False
        if any(node.name.endswith(s) for s in _CDK_CLASS_SUFFIXES):
            return False
    if node.kind == "Function" and is_framework_class:
        return False
    if (
        node.kind == "Function"
        and node.parent_name
        and any(node.parent_name.endswith(s) for s in _CDK_CLASS_SUFFIXES)
    ):
        return False

    decorators = node.extra.get("decorators", ())
    if isinstance(decorators, (list, tuple)) and decorators:
        if node.kind in ("Function", "Test"):
            if any(
                d in ("property", "abstractmethod", "classmethod", "staticmethod")
                or d.endswith(".abstractmethod")
                or d.startswith("HostListener")
                for d in decorators
            ):
                return False
        if node.kind == "Class" and any("dataclass" in d for d in decorators):
            return False

    return True


def _dead_code_record(
    node: Any,
    *,
    caller_count: int,
    test_ref_count: int,
    importer_count: int,
    reference_count: int,
    subclass_count: int,
) -> dict[str, Any]:
    reason_codes = []
    if caller_count == 0:
        reason_codes.append("no_callers")
    if test_ref_count == 0:
        reason_codes.append("no_test_references")
    if importer_count == 0:
        reason_codes.append("no_importers")
    if reference_count == 0:
        reason_codes.append("no_references")
    if node.kind == "Class" and subclass_count == 0:
        reason_codes.append("no_subclasses")

    return {
        "name": _sanitize_name(node.name),
        "qualified_name": _sanitize_name(node.qualified_name),
        "kind": node.kind,
        "file": node.file_path,
        "line": node.line_start,
        "language": node.language,
        "confidence": _dead_code_confidence(node),
        "reason_codes": reason_codes,
        "evidence": {
            "caller_count": caller_count,
            "test_ref_count": test_ref_count,
            "importer_count": importer_count,
            "reference_count": reference_count,
            "subclass_count": subclass_count,
        },
        "caveats": [
            "Static analysis can miss runtime dispatch, plugin registration, reflection, "
            "and dynamic imports."
        ],
    }


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
    candidates = store.get_nodes_by_kind(
        kinds=[kind] if kind else ["Function", "Class"],
        file_pattern=file_pattern,
    )

    type_ref_names = _collect_type_referenced_names(store)

    class_bases: dict[str, list[str]] = {}
    class_inherits_targets: dict[str, list[str]] = {}
    conn = store._conn
    for row in conn.execute(
        "SELECT source_qualified, target_qualified FROM edges WHERE kind = 'INHERITS'"
    ).fetchall():
        base = row[1].rsplit("::", 1)[-1] if "::" in row[1] else row[1]
        class_bases.setdefault(row[0], []).append(base)
        class_inherits_targets.setdefault(row[0], []).append(row[1])

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

    # ---------------------------------------------------------------------------
    # Pass 1: SQL-free pre-filter using only node data + preloaded dicts.
    # Collects candidates that survive all node-level exclusion rules so we
    # can batch-preload their incoming edges before the main analysis pass.
    # ---------------------------------------------------------------------------
    surviving: list[Any] = []
    for node in candidates:
        if _survives_dead_code_node_filters(node, type_ref_names, class_bases):
            surviving.append(node)

    # ---------------------------------------------------------------------------
    # Batch preloads for the main analysis pass
    # ---------------------------------------------------------------------------
    batch_size = 450

    # Collect all QNs we need incoming edges for (primary + parent::name form)
    incoming_qns: list[str] = []
    survivor_names_set: set[str] = set()
    for node in surviving:
        incoming_qns.append(node.qualified_name)
        survivor_names_set.add(node.name)
        if node.parent_name:
            incoming_qns.append(f"{node.parent_name}::{node.name}")

    # Incoming edges indexed by target_qualified
    incoming_by_qn: dict[str, list[Any]] = {}
    for i in range(0, len(incoming_qns), batch_size):
        chunk = incoming_qns[i : i + batch_size]
        placeholders = ",".join("?" for _ in chunk)
        for row in conn.execute(  # nosec B608
            f"SELECT * FROM edges WHERE target_qualified IN ({placeholders})",
            chunk,
        ).fetchall():
            edge = store._row_to_edge(row)
            incoming_by_qn.setdefault(row["target_qualified"], []).append(edge)

    # TESTED_BY edges are directed from covered production symbol to test symbol.
    tested_by_source_qn: dict[str, list[Any]] = {}
    for i in range(0, len(incoming_qns), batch_size):
        chunk = incoming_qns[i : i + batch_size]
        placeholders = ",".join("?" for _ in chunk)
        for row in conn.execute(  # nosec B608
            f"SELECT * FROM edges WHERE source_qualified IN ({placeholders}) "
            "AND kind = 'TESTED_BY'",
            chunk,
        ).fetchall():
            edge = store._row_to_edge(row)
            tested_by_source_qn.setdefault(row["source_qualified"], []).append(edge)

    # Bare-name edges for CALLS/TESTED_BY/INHERITS
    bare_calls_by_name: dict[str, list[Any]] = {}
    bare_tested_by_name: dict[str, list[Any]] = {}
    bare_inherits_by_name: dict[str, list[Any]] = {}
    survivor_names_list = list(survivor_names_set)
    for i in range(0, len(survivor_names_list), batch_size):
        chunk = survivor_names_list[i : i + batch_size]
        placeholders = ",".join("?" for _ in chunk)
        for row in conn.execute(  # nosec B608
            f"SELECT * FROM edges WHERE target_qualified IN ({placeholders}) "
            f"AND kind IN ('CALLS', 'INHERITS')",
            chunk,
        ).fetchall():
            edge = store._row_to_edge(row)
            kind, tgt = row["kind"], row["target_qualified"]
            if kind == "CALLS":
                bare_calls_by_name.setdefault(tgt, []).append(edge)
            else:
                bare_inherits_by_name.setdefault(tgt, []).append(edge)
        for row in conn.execute(  # nosec B608
            f"SELECT * FROM edges WHERE source_qualified IN ({placeholders}) "
            "AND kind = 'TESTED_BY'",
            chunk,
        ).fetchall():
            edge = store._row_to_edge(row)
            bare_tested_by_name.setdefault(row["source_qualified"], []).append(edge)

    # Qualified CALLS edges indexed by normalized target_name. This replaces
    # suffix LIKE scans over target_qualified when matching by bare symbol name.
    suffix_calls_by_name: dict[str, list[Any]] = {}
    for i in range(0, len(survivor_names_list), batch_size):
        chunk = survivor_names_list[i : i + batch_size]
        placeholders = ",".join("?" for _ in chunk)
        for row in conn.execute(  # nosec B608
            f"SELECT * FROM edges WHERE target_name IN ({placeholders}) "
            "AND kind = 'CALLS' AND target_qualified != target_name",
            chunk,
        ).fetchall():
            edge = store._row_to_edge(row)
            suffix_calls_by_name.setdefault(row["target_name"], []).append(edge)

    # Preload base-method nodes for the abstractmethod check (lines ~250-268).
    # Compute all candidate (base_class_qn.method_name) keys from class_inherits_targets.
    base_method_qns_set: set[str] = set()
    for node in surviving:
        if node.kind == "Function" and node.parent_name:
            parent_qn = node.qualified_name.rsplit(".", 1)[0]
            for base_cls_qn in class_inherits_targets.get(parent_qn, []):
                base_method_qns_set.add(f"{base_cls_qn}.{node.name}")
                base_method_qns_set.add(f"{node.file_path}::{base_cls_qn}.{node.name}")
    base_nodes_map: dict[str, Any] = {}
    if base_method_qns_set:
        for qn, n in store.get_nodes_by_qualified_names(list(base_method_qns_set)).items():
            base_nodes_map[qn] = n

    # ---------------------------------------------------------------------------
    # Pass 2: main dead-code analysis using preloaded data (no SQL in the loop)
    # ---------------------------------------------------------------------------
    dead: list[dict[str, Any]] = []

    for node in surviving:
        # Abstractmethod-in-base check: uses class_inherits_targets + preloaded nodes
        if node.kind == "Function" and node.parent_name:
            parent_qn = node.qualified_name.rsplit(".", 1)[0]
            base_class_names = class_inherits_targets.get(parent_qn, [])
            for base_name in base_class_names:
                base_method_qn = f"{base_name}.{node.name}"
                base_node = base_nodes_map.get(base_method_qn)
                if base_node is None:
                    base_method_qn2 = f"{node.file_path}::{base_name}.{node.name}"
                    base_node = base_nodes_map.get(base_method_qn2)
                if base_node is not None:
                    base_decos = base_node.extra.get("decorators", ())
                    if isinstance(base_decos, (list, tuple)) and any(
                        "abstractmethod" in d for d in base_decos
                    ):
                        break
            else:
                base_name = None
            if base_name is not None:
                continue

        incoming = list(incoming_by_qn.get(node.qualified_name, []))
        if not any(e.kind == "CALLS" for e in incoming) and node.parent_name:
            class_qn = f"{node.parent_name}::{node.name}"
            incoming = incoming + incoming_by_qn.get(class_qn, [])
        if not any(e.kind == "CALLS" for e in incoming):
            all_bare = bare_calls_by_name.get(node.name, []) + suffix_calls_by_name.get(
                node.name, []
            )
            all_bare = [
                e
                for e in all_bare
                if _is_plausible_caller(
                    e.file_path, node.file_path, node.name, importer_files, name_counts
                )
            ]
            incoming = incoming + all_bare
        tested_by_edges = list(tested_by_source_qn.get(node.qualified_name, []))
        if node.parent_name:
            class_qn = f"{node.parent_name}::{node.name}"
            tested_by_edges.extend(tested_by_source_qn.get(class_qn, []))
        if not tested_by_edges:
            bare_tb = [
                e
                for e in bare_tested_by_name.get(node.name, [])
                if _is_plausible_caller(
                    e.file_path, node.file_path, node.name, importer_files, name_counts
                )
            ]
            tested_by_edges.extend(bare_tb)
        if node.kind == "Class" and not any(e.kind == "INHERITS" for e in incoming):
            incoming = incoming + bare_inherits_by_name.get(node.name, [])

        has_callers = any(e.kind == "CALLS" for e in incoming)
        has_test_refs = bool(tested_by_edges)
        has_importers = any(e.kind == "IMPORTS_FROM" for e in incoming)
        has_references = any(e.kind == "REFERENCES" for e in incoming)
        has_subclasses = any(e.kind == "INHERITS" for e in incoming)

        no_refs = not (
            has_callers or has_test_refs or has_importers or has_references or has_subclasses
        )
        if node.kind == "Class" and no_refs:
            bare_prefix = node.name + "."
            member_calls = store.count_edges_by_target_name_prefix(bare_prefix, kind="CALLS")
            if member_calls > 0:
                has_callers = True

        caller_count = sum(1 for e in incoming if e.kind == "CALLS")
        test_ref_count = len(tested_by_edges)
        importer_count = sum(1 for e in incoming if e.kind == "IMPORTS_FROM")
        reference_count = sum(1 for e in incoming if e.kind == "REFERENCES")
        subclass_count = sum(1 for e in incoming if e.kind == "INHERITS")

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
                    _dead_code_record(
                        node,
                        caller_count=caller_count,
                        test_ref_count=test_ref_count,
                        importer_count=importer_count,
                        reference_count=reference_count,
                        subclass_count=subclass_count,
                    )
                )

    logger.info("find_dead_code: found %d dead symbols", len(dead))
    return dead
