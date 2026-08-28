"""Dead code detection for graph-powered refactoring."""

from __future__ import annotations

import functools
import logging
import re
from pathlib import Path
from typing import Any, Optional

from ..cross_artifact import (
    cross_artifact_role,
    edge_extra,
    is_cross_artifact,
    is_reportable_bridge,
    is_unresolved_target,
)
from ..entry_point_heuristics import has_framework_decorator, matches_entry_name
from ..graph import GraphEdge, GraphNode, GraphStore, _sanitize_name

logger = logging.getLogger(__name__)

type DeadValue = Any
type DeadPayload = dict[str, DeadValue]

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
_VALUE_CONTAINER_CLASS_ROLES = frozenset({"struct", "enum", "record"})
_VALUE_CONTAINER_DERIVE_TRAITS = frozenset({"Serialize", "Deserialize"})

# Configuration / manifest languages are not executable deletion targets.
_CONFIG_ARTIFACT_LANGUAGES = frozenset(
    {
        "terraform",
        "hcl",
        "json",
        "yaml",
        "toml",
    }
)


@functools.lru_cache(maxsize=4096)
def _path_segments(file_path: str) -> tuple[str, ...]:
    parts = file_path.replace("\\", "/").split("/")
    return tuple(
        p
        for p in parts[:-1]
        if len(p) >= _MIN_PKG_SEGMENT_LEN and p not in ("home", "src", "lib", "app")
    )


_TYPE_IDENT_RE = re.compile(r"[A-Z][A-Za-z0-9_]*")


def _load_source_lines(store: GraphStore, file_path: str) -> list[str]:
    try:
        path = store.resolve_file_path(file_path)
    except (AttributeError, TypeError):
        path = Path(file_path)
    try:
        return path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError):
        return []


def _source_line(lines: list[str], line_number: int | None) -> str:
    if line_number is None or line_number <= 0 or line_number > len(lines):
        return ""
    return lines[line_number - 1].strip()


def _is_source_public_api_candidate(node: Any, lines: list[str]) -> bool:
    line = _source_line(lines, node.line_start)
    if not line:
        return False
    public_markers = (
        "pub ",
        "pub(",
        "public ",
        "export ",
        "export default ",
        "export async ",
        "export function ",
        "export class ",
        "export interface ",
        "export const ",
        "export let ",
        "export var ",
    )
    if line.startswith(public_markers):
        return True
    if node.language in {"typescript", "tsx", "javascript", "vue", "svelte"}:
        return " export " in f" {line} " or line.startswith("exports.")
    return False


def _is_bridge_export_candidate(node: Any, lines: list[str]) -> bool:
    if node.language != "rust":
        return False
    line_number = node.line_start
    if not isinstance(line_number, int) or line_number <= 0 or not lines:
        return False

    target_idx = min(line_number - 1, len(lines) - 1)
    for idx in range(target_idx, -1, -1):
        line = lines[idx]
        if not line.lstrip().startswith("impl "):
            continue
        window = "\n".join(lines[max(0, idx - 5) : idx + 1])
        if "#[pymethods]" not in window:
            continue
        depth = 0
        for scoped_line in lines[idx : target_idx + 1]:
            depth += scoped_line.count("{")
            depth -= scoped_line.count("}")
        if depth > 0:
            return True
    return False


def _is_public_api_candidate(node: Any, lines: list[str]) -> bool:
    return _is_source_public_api_candidate(node, lines) or _is_bridge_export_candidate(node, lines)


def _collect_type_referenced_names(store: GraphStore) -> set[str]:
    funcs = store.get_nodes_by_kind(kinds=["Function", "Test"])
    names: set[str] = set()
    for f in funcs:
        for text in (f.params, f.return_type):
            if text:
                names.update(_TYPE_IDENT_RE.findall(text))
    return names


def _is_entry_point(node: Any) -> bool:
    if has_framework_decorator(node):
        return True
    if matches_entry_name(node):
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


def _has_value_container_metadata(extra: DeadPayload) -> bool:
    if extra.get("container_role") == "data_container":
        return True
    if extra.get("value_semantics") is True:
        return True
    role = extra.get("type_role")
    if role in _VALUE_CONTAINER_CLASS_ROLES:
        return True
    derive_traits = extra.get("derive_traits")
    return isinstance(derive_traits, (list, tuple, set)) and any(
        trait in _VALUE_CONTAINER_DERIVE_TRAITS for trait in derive_traits
    )


def _dead_code_confidence(
    node: Any,
    *,
    public_api_candidate: bool,
    name_definition_count: int,
    source_available: bool,
    reachable_via_cross_artifact: bool = False,
) -> str:
    extra = node.extra if isinstance(node.extra, dict) else {}
    if public_api_candidate or _has_value_container_metadata(extra) or reachable_via_cross_artifact:
        return "low"
    if name_definition_count > 1 or not source_available:
        return "low"
    return "medium"


def _cross_artifact_symbol_name(edge: Any) -> str | None:
    """Return the handler/entrypoint symbol referenced by a CROSS_ARTIFACT edge."""
    if not is_cross_artifact(edge):
        return None
    extra = edge_extra(edge)
    sym = extra.get("original_symbol_name")
    if isinstance(sym, str) and sym:
        return sym
    target = str(getattr(edge, "target_qualified", "") or "")
    if target.startswith("<unresolved:") and target.endswith(">"):
        return target[len("<unresolved:") : -1]
    return None


def _maps_entrypoint_symbol_matches_node(edge: Any, node: Any) -> bool:
    """True when an unresolved maps_entrypoint bridge plausibly names *node*."""
    if cross_artifact_role(edge) != "maps_entrypoint":
        return False
    sym = _cross_artifact_symbol_name(edge)
    if not sym:
        return False
    if "." in sym:
        _, _, attr = sym.rpartition(".")
        return node.name == attr
    return node.name == sym


def _incoming_cross_artifact_reachability(
    incoming: list[GraphEdge],
    *,
    unresolved_entrypoints: list[GraphEdge],
    node: GraphNode,
) -> tuple[bool, bool]:
    """Return ``(has_reportable_reference, has_unresolved_entrypoint_match)``."""
    has_reportable = False
    for edge in incoming:
        if not is_cross_artifact(edge):
            continue
        if is_reportable_bridge(edge):
            has_reportable = True
            break

    has_unresolved_entrypoint = any(
        is_unresolved_target(edge) and _maps_entrypoint_symbol_matches_node(edge, node)
        for edge in unresolved_entrypoints
    )
    return has_reportable, has_unresolved_entrypoint


def _is_value_container_type_node(node: Any) -> bool:
    if node.kind != "Class":
        return False
    extra = node.extra if isinstance(node.extra, dict) else {}
    return _has_value_container_metadata(extra)


def _survives_dead_code_node_filters(
    node: Any,
    type_ref_names: set[str],
    class_bases: dict[str, list[str]],
) -> bool:
    if node.language in _CONFIG_ARTIFACT_LANGUAGES or node.language == "markdown":
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
    if node.kind == "Class" and has_framework_decorator(node):
        return False
    if _is_structural_type_node(node):
        return False
    if _is_value_container_type_node(node):
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
    public_api_candidate: bool,
    name_definition_count: int,
    source_available: bool,
    reachable_via_cross_artifact: bool = False,
) -> DeadPayload:
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
    if public_api_candidate:
        reason_codes.append("public_api_candidate")
    if reachable_via_cross_artifact:
        reason_codes.append("reachable_via_cross_artifact")
    if name_definition_count > 1:
        reason_codes.append("ambiguous_symbol_name")
    if not source_available:
        reason_codes.append("source_unavailable")

    caveats = [
        "Static analysis can miss runtime dispatch, plugin registration, reflection, "
        "and dynamic imports."
    ]
    if public_api_candidate:
        caveats.append(
            "Public API symbols may be consumed outside the indexed graph; verify "
            "downstream users before deleting."
        )
    if reachable_via_cross_artifact:
        caveats.append(
            "An unresolved manifest or Terraform entrypoint bridge references this "
            "symbol; verify runtime wiring before deleting."
        )

    return {
        "name": _sanitize_name(node.name),
        "qualified_name": _sanitize_name(node.qualified_name),
        "kind": node.kind,
        "file": node.file_path,
        "line": node.line_start,
        "language": node.language,
        "confidence": _dead_code_confidence(
            node,
            public_api_candidate=public_api_candidate,
            name_definition_count=name_definition_count,
            source_available=source_available,
            reachable_via_cross_artifact=reachable_via_cross_artifact,
        ),
        "public_api_candidate": public_api_candidate,
        "reason_codes": reason_codes,
        "evidence": {
            "caller_count": caller_count,
            "test_ref_count": test_ref_count,
            "importer_count": importer_count,
            "reference_count": reference_count,
            "subclass_count": subclass_count,
            "name_definition_count": name_definition_count,
            "source_available": source_available,
            "reachable_via_cross_artifact": reachable_via_cross_artifact,
        },
        "caveats": caveats,
    }


def _has_callers_via_base_method(
    store: GraphStore,
    node: Any,
    class_bases: dict[str, list[str]],
) -> bool:
    if node.kind != "Function" or not node.parent_name:
        return False

    method_suffix = "." + node.name
    if not node.qualified_name.endswith(method_suffix):
        return False

    class_qn = node.qualified_name[: -len(method_suffix)]
    for base_name in class_bases.get(class_qn, []):
        for base_node in store.get_nodes_by_parent_and_name(
            base_name,
            node.name,
            ["Function", "Test"],
        ):
            if store.has_edge_to_target(base_node.qualified_name, "CALLS"):
                return True
    return False


class _DeadCodeLookups:
    """Batched reference preloads shared by the dead-code analysis pass."""

    __slots__ = (
        "surviving",
        "class_bases",
        "class_inherits_targets",
        "importer_files",
        "name_counts",
        "incoming_by_qn",
        "tested_by_source_qn",
        "bare_calls_by_name",
        "bare_tested_by_name",
        "bare_inherits_by_name",
        "suffix_calls_by_name",
        "base_nodes_map",
        "unresolved_entrypoint_by_name",
    )

    def __init__(
        self,
        *,
        surviving: list[GraphNode],
        class_bases: dict[str, list[str]],
        class_inherits_targets: dict[str, list[str]],
        importer_files: dict[str, set[str]],
        name_counts: dict[str, int],
        incoming_by_qn: dict[str, list[GraphEdge]],
        tested_by_source_qn: dict[str, list[GraphEdge]],
        bare_calls_by_name: dict[str, list[GraphEdge]],
        bare_tested_by_name: dict[str, list[GraphEdge]],
        bare_inherits_by_name: dict[str, list[GraphEdge]],
        suffix_calls_by_name: dict[str, list[GraphEdge]],
        base_nodes_map: dict[str, GraphNode],
        unresolved_entrypoint_by_name: dict[str, list[GraphEdge]],
    ) -> None:
        self.surviving = surviving
        self.class_bases = class_bases
        self.class_inherits_targets = class_inherits_targets
        self.importer_files = importer_files
        self.name_counts = name_counts
        self.incoming_by_qn = incoming_by_qn
        self.tested_by_source_qn = tested_by_source_qn
        self.bare_calls_by_name = bare_calls_by_name
        self.bare_tested_by_name = bare_tested_by_name
        self.bare_inherits_by_name = bare_inherits_by_name
        self.suffix_calls_by_name = suffix_calls_by_name
        self.base_nodes_map = base_nodes_map
        self.unresolved_entrypoint_by_name = unresolved_entrypoint_by_name


def _collect_dead_code_context(
    store: GraphStore,
    kind: Optional[str],
    file_pattern: Optional[str],
) -> _DeadCodeLookups:
    """Collect candidate nodes and batch-preload every lookup the analysis needs.

    Keeps all SQL out of the per-node analysis loop: node-level filters,
    incoming edge maps, bare-name edges, base-method nodes, and unresolved
    entrypoint edges are loaded once up front.
    """
    candidates = store.get_nodes_by_kind(
        kinds=[kind] if kind else ["Function", "Class"],
        file_pattern=file_pattern,
    )

    type_ref_names = _collect_type_referenced_names(store)

    class_bases: dict[str, list[str]] = {}
    class_inherits_targets: dict[str, list[str]] = {}
    for edge in store.get_edges_by_kind("INHERITS"):
        target = edge.target_qualified
        base = target.rsplit("::", 1)[-1] if "::" in target else target
        class_bases.setdefault(edge.source_qualified, []).append(base)
        class_inherits_targets.setdefault(edge.source_qualified, []).append(target)

    importer_files: dict[str, set[str]] = {}
    for edge in store.get_edges_by_kind("IMPORTS_FROM"):
        importer_files.setdefault(edge.file_path, set()).add(edge.target_qualified)

    name_counts = store.count_nodes_by_name(["Function", "Class"], include_tests=False)

    # ---------------------------------------------------------------------------
    # Pass 1: SQL-free pre-filter using only node data + preloaded dicts.
    # Collects candidates that survive all node-level exclusion rules so we
    # can batch-preload their incoming edges before the main analysis pass.
    # ---------------------------------------------------------------------------
    surviving: list[GraphNode] = []
    for node in candidates:
        if _survives_dead_code_node_filters(node, type_ref_names, class_bases):
            surviving.append(node)

    # ---------------------------------------------------------------------------
    # Batch preloads for the main analysis pass
    # ---------------------------------------------------------------------------

    # Collect all QNs we need incoming edges for (primary + parent::name form)
    incoming_qns: list[str] = []
    survivor_names_set: set[str] = set()
    for node in surviving:
        incoming_qns.append(node.qualified_name)
        survivor_names_set.add(node.name)
        if node.parent_name:
            incoming_qns.append(f"{node.parent_name}::{node.name}")

    incoming_by_qn = store.get_edges_by_targets(incoming_qns)
    # TESTED_BY edges are directed from covered production symbol to test symbol.
    tested_by_source_qn = store.get_edges_by_sources(incoming_qns, ["TESTED_BY"])

    # Bare-name edges: CALLS/INHERITS edges whose target is an unqualified name,
    # and TESTED_BY edges whose source is one.
    survivor_names_list = list(survivor_names_set)
    bare_calls_by_name: dict[str, list[GraphEdge]] = {}
    bare_inherits_by_name: dict[str, list[GraphEdge]] = {}
    for target_name, edges in store.get_edges_by_targets(
        survivor_names_list,
        ["CALLS", "INHERITS"],
    ).items():
        for edge in edges:
            if edge.kind == "CALLS":
                bare_calls_by_name.setdefault(target_name, []).append(edge)
            else:
                bare_inherits_by_name.setdefault(target_name, []).append(edge)
    bare_tested_by_name = store.get_edges_by_sources(survivor_names_list, ["TESTED_BY"])

    # Qualified CALLS edges indexed by normalized target_name. This replaces
    # suffix LIKE scans over target_qualified when matching by bare symbol name.
    suffix_calls_by_name = store.get_edges_by_target_names(
        survivor_names_list,
        kind="CALLS",
        qualified_only=True,
    )

    # Preload base-method nodes for the abstractmethod check (lines ~250-268).
    # Compute all candidate (base_class_qn.method_name) keys from class_inherits_targets.
    base_method_qns_set: set[str] = set()
    for node in surviving:
        if node.kind == "Function" and node.parent_name:
            parent_qn = node.qualified_name.rsplit(".", 1)[0]
            for base_cls_qn in class_inherits_targets.get(parent_qn, []):
                base_method_qns_set.add(f"{base_cls_qn}.{node.name}")
                base_method_qns_set.add(f"{node.file_path}::{base_cls_qn}.{node.name}")
    base_nodes_map: dict[str, GraphNode] = {}
    if base_method_qns_set:
        for qn, n in store.get_nodes_by_qualified_names(list(base_method_qns_set)).items():
            base_nodes_map[qn] = n

    unresolved_entrypoint_by_name: dict[str, list[GraphEdge]] = {}
    for edge in store.get_edges_by_kind("CROSS_ARTIFACT", unresolved_target_only=True):
        if cross_artifact_role(edge) != "maps_entrypoint":
            continue
        sym = _cross_artifact_symbol_name(edge)
        if not sym:
            continue
        key = sym.rpartition(".")[2] if "." in sym else sym
        unresolved_entrypoint_by_name.setdefault(key, []).append(edge)

    return _DeadCodeLookups(
        surviving=surviving,
        class_bases=class_bases,
        class_inherits_targets=class_inherits_targets,
        importer_files=importer_files,
        name_counts=name_counts,
        incoming_by_qn=incoming_by_qn,
        tested_by_source_qn=tested_by_source_qn,
        bare_calls_by_name=bare_calls_by_name,
        bare_tested_by_name=bare_tested_by_name,
        bare_inherits_by_name=bare_inherits_by_name,
        suffix_calls_by_name=suffix_calls_by_name,
        base_nodes_map=base_nodes_map,
        unresolved_entrypoint_by_name=unresolved_entrypoint_by_name,
    )


def _node_dead_code_evidence(
    store: GraphStore,
    node: GraphNode,
    lookups: _DeadCodeLookups,
    source_cache: dict[str, list[str]],
) -> Optional[DeadPayload]:
    """Resolve every reference kind for one survivor node.

    Returns the ``_dead_code_record`` kwargs when the node has no reference
    at all, or ``None`` when the node is reachable. Pure decision logic: no
    graph query is issued here except the Class member-call and base-method
    checks.
    """
    # Abstractmethod-in-base check: uses class_inherits_targets + preloaded nodes
    if node.kind == "Function" and node.parent_name:
        parent_qn = node.qualified_name.rsplit(".", 1)[0]
        base_class_names = lookups.class_inherits_targets.get(parent_qn, [])
        for base_name in base_class_names:
            base_method_qn = f"{base_name}.{node.name}"
            base_node = lookups.base_nodes_map.get(base_method_qn)
            if base_node is None:
                base_method_qn2 = f"{node.file_path}::{base_name}.{node.name}"
                base_node = lookups.base_nodes_map.get(base_method_qn2)
            if base_node is not None:
                base_decos = base_node.extra.get("decorators", ())
                if isinstance(base_decos, (list, tuple)) and any(
                    "abstractmethod" in d for d in base_decos
                ):
                    break
        else:
            base_name = None
        if base_name is not None:
            return None

    incoming = list(lookups.incoming_by_qn.get(node.qualified_name, []))
    if not any(e.kind == "CALLS" for e in incoming) and node.parent_name:
        class_qn = f"{node.parent_name}::{node.name}"
        incoming = incoming + lookups.incoming_by_qn.get(class_qn, [])
    if not any(e.kind == "CALLS" for e in incoming):
        all_bare = lookups.bare_calls_by_name.get(node.name, []) + lookups.suffix_calls_by_name.get(
            node.name, []
        )
        all_bare = [
            e
            for e in all_bare
            if _is_plausible_caller(
                e.file_path,
                node.file_path,
                node.name,
                lookups.importer_files,
                lookups.name_counts,
            )
        ]
        incoming = incoming + all_bare
    tested_by_edges = list(lookups.tested_by_source_qn.get(node.qualified_name, []))
    if node.parent_name:
        class_qn = f"{node.parent_name}::{node.name}"
        tested_by_edges.extend(lookups.tested_by_source_qn.get(class_qn, []))
    if not tested_by_edges:
        bare_tb = [
            e
            for e in lookups.bare_tested_by_name.get(node.name, [])
            if _is_plausible_caller(
                e.file_path,
                node.file_path,
                node.name,
                lookups.importer_files,
                lookups.name_counts,
            )
        ]
        tested_by_edges.extend(bare_tb)
    if node.kind == "Class" and not any(e.kind == "INHERITS" for e in incoming):
        incoming = incoming + lookups.bare_inherits_by_name.get(node.name, [])

    has_callers = any(e.kind == "CALLS" for e in incoming)
    has_test_refs = bool(tested_by_edges)
    has_importers = any(e.kind == "IMPORTS_FROM" for e in incoming)
    has_references = any(e.kind == "REFERENCES" for e in incoming)
    has_subclasses = any(e.kind == "INHERITS" for e in incoming)
    caller_count = sum(1 for e in incoming if e.kind == "CALLS")
    test_ref_count = len(tested_by_edges)
    importer_count = sum(1 for e in incoming if e.kind == "IMPORTS_FROM")
    reference_count = sum(1 for e in incoming if e.kind == "REFERENCES")
    subclass_count = sum(1 for e in incoming if e.kind == "INHERITS")
    has_reportable_cross_artifact, has_unresolved_entrypoint = (
        _incoming_cross_artifact_reachability(
            incoming,
            unresolved_entrypoints=lookups.unresolved_entrypoint_by_name.get(node.name, []),
            node=node,
        )
    )
    if has_reportable_cross_artifact:
        has_references = True
        reference_count += sum(
            1 for e in incoming if is_cross_artifact(e) and is_reportable_bridge(e)
        )

    no_refs = not (
        has_callers
        or has_test_refs
        or has_importers
        or has_references
        or has_subclasses
        or has_unresolved_entrypoint
    )
    if node.kind == "Class" and no_refs:
        bare_prefix = node.name + "."
        member_calls = store.count_edges_by_target_name_prefix(bare_prefix, kind="CALLS")
        if member_calls > 0:
            has_callers = True

    if not (
        has_callers
        or has_test_refs
        or has_importers
        or has_references
        or has_subclasses
        or has_unresolved_entrypoint
    ):
        if not has_callers and _has_callers_via_base_method(store, node, lookups.class_bases):
            has_callers = True

        if not (
            has_callers
            or has_test_refs
            or has_importers
            or has_references
            or has_subclasses
            or has_unresolved_entrypoint
        ):
            lines = source_cache.setdefault(
                node.file_path, _load_source_lines(store, node.file_path)
            )
            source_available = bool(lines)
            public_api_candidate = _is_public_api_candidate(node, lines)
            return {
                "caller_count": caller_count,
                "test_ref_count": test_ref_count,
                "importer_count": importer_count,
                "reference_count": reference_count,
                "subclass_count": subclass_count,
                "public_api_candidate": public_api_candidate,
                "name_definition_count": lookups.name_counts.get(node.name, 0),
                "source_available": source_available,
                "reachable_via_cross_artifact": has_unresolved_entrypoint,
            }

    return None


def find_dead_code(
    store: GraphStore,
    kind: Optional[str] = None,
    file_pattern: Optional[str] = None,
) -> list[DeadPayload]:
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
    lookups = _collect_dead_code_context(store, kind, file_pattern)

    # ---------------------------------------------------------------------------
    # Pass 2: main dead-code analysis using preloaded data (no SQL in the loop)
    # ---------------------------------------------------------------------------
    dead: list[DeadPayload] = []
    source_cache: dict[str, list[str]] = {}

    for node in lookups.surviving:
        evidence = _node_dead_code_evidence(store, node, lookups, source_cache)
        if evidence is None:
            continue
        dead.append(
            _dead_code_record(
                node,
                **evidence,
            )
        )

    logger.info("find_dead_code: found %d dead symbols", len(dead))
    return dead
