"""Community-driven refactoring suggestions."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from ..graph import GraphStore, _sanitize_name
from . import concerns
from .dead_code import find_dead_code

logger = logging.getLogger(__name__)

type SuggestionValue = Any
type SuggestionPayload = dict[str, SuggestionValue]

_PRODUCTION_LANGUAGES = frozenset(
    {
        "python",
        "rust",
        "javascript",
        "typescript",
        "tsx",
        "java",
        "go",
        "ruby",
        "php",
        "c",
        "cpp",
        "csharp",
        "swift",
        "kotlin",
        "scala",
        "dart",
        "lua",
        "luau",
        "julia",
        "r",
        "elixir",
        "solidity",
        "vue",
        "bash",
        "terraform",
    }
)

_FUNCTION_SPLIT_MIN_LINES = 60
_FUNCTION_SPLIT_MIN_BRANCHES = 12
_FUNCTION_SPLIT_MIN_OUTGOING_CALLS = 22
_CLASS_SPLIT_MIN_LINES = 120
_CLASS_SPLIT_MIN_BRANCHES = 20
_CLASS_SPLIT_ABSOLUTE_LINES = 250
_MOVE_MIN_CALLERS = 2
_MOVE_MEDIUM_CONFIDENCE_CALLERS = 4
_MOVE_HIGH_CONFIDENCE_CALLERS = 8
_DOCUMENT_MIN_LINES = 60
_LOW_COMMENT_RATIO = 0.01
_FUNCTION_CONCERN_SPLIT_SCORE = 0.65


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


def _source_span(lines: list[str], start: int | None, end: int | None) -> list[str]:
    if not lines or start is None or end is None:
        return []
    if start <= 0 or end < start:
        return []
    return lines[start - 1 : min(end, len(lines))]


def _is_test_file_path(file_path: str) -> bool:
    normalized = file_path.replace("\\", "/")
    name = normalized.rsplit("/", 1)[-1]
    return (
        "/tests/" in normalized
        or "/__tests__/" in normalized
        or name in {"tests.rs", "test.rs"}
        or name.endswith("_tests.rs")
        or name.endswith("_test.rs")
        or name.startswith("test_")
        or ".test." in name
        or ".spec." in name
    )


def _is_test_artifact(record: SuggestionPayload, lines: list[str]) -> bool:
    if record.get("is_test"):
        return True
    file_path = str(record.get("file", ""))
    if _is_test_file_path(file_path):
        return True
    return _is_rust_cfg_test_candidate(record, lines)


def _is_source_public_api_candidate(record: SuggestionPayload, lines: list[str]) -> bool:
    line = _source_line(lines, record.get("line"))
    if not line:
        return False
    language = str(record.get("language", ""))
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
    if language in {"typescript", "tsx", "javascript", "vue", "svelte"}:
        return " export " in f" {line} " or line.startswith("exports.")
    return False


def _is_bridge_export_candidate(record: SuggestionPayload, lines: list[str]) -> bool:
    language = str(record.get("language", ""))
    if language != "rust":
        return False
    line_number = record.get("line")
    if not isinstance(line_number, int) or line_number <= 0:
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


def _is_external_api_candidate(record: SuggestionPayload, lines: list[str]) -> bool:
    return _is_source_public_api_candidate(record, lines) or _is_bridge_export_candidate(
        record, lines
    )


def _is_rust_cfg_test_candidate(record: SuggestionPayload, lines: list[str]) -> bool:
    if record.get("language") != "rust":
        return False
    line_number = record.get("line")
    if not isinstance(line_number, int) or line_number <= 0:
        return False

    target_idx = min(line_number - 1, len(lines) - 1)
    for idx in range(target_idx, -1, -1):
        line = lines[idx]
        if "mod tests" not in line:
            continue
        window = "\n".join(lines[max(0, idx - 3) : idx + 1])
        if "#[cfg(test)]" not in window:
            continue
        depth = 0
        for scoped_line in lines[idx : target_idx + 1]:
            depth += scoped_line.count("{")
            depth -= scoped_line.count("}")
        if depth > 0:
            return True
    return False


def _dead_code_category(record: SuggestionPayload) -> str:
    language = record.get("language")
    file_path = str(record.get("file", ""))
    if language == "markdown":
        return "documentation"
    if record.get("public_api_candidate"):
        return "public_api"
    if record.get("test_artifact") or _is_test_file_path(file_path):
        return "test"
    if "/fixtures/" in file_path or file_path.startswith("tests/fixtures/"):
        return "fixture"
    if language in _PRODUCTION_LANGUAGES:
        return "executable"
    return "unknown"


def _evidence_sort_value(suggestion: SuggestionPayload) -> float:
    evidence = suggestion.get("evidence", {})
    if suggestion.get("type") == "split" and isinstance(evidence, dict):
        return -float(evidence.get("split_pressure", 0.0))
    if suggestion.get("type") == "document" and isinstance(evidence, dict):
        return float(evidence.get("comment_ratio", 1.0))
    return 0.0


def _suggestion_sort_key(
    suggestion: SuggestionPayload,
) -> tuple[int, int, int, int, int, float, str]:
    category_rank = {
        "executable": 0,
        "unknown": 1,
        "fixture": 2,
        "test": 3,
        "public_api": 4,
        "documentation": 5,
    }
    priority_rank = {"high": 0, "medium": 1, "low": 2}
    confidence_rank = {"high": 0, "medium": 1, "low": 2}
    risk_rank = {"low": 0, "medium": 1, "high": 2}
    type_rank = {"split": 0, "move": 1, "document": 2, "remove": 3}
    symbols = suggestion.get("symbols", [])
    symbol = str(symbols[0]) if isinstance(symbols, list) and symbols else ""
    return (
        category_rank.get(suggestion.get("category", "unknown"), 1),
        priority_rank.get(suggestion.get("priority", "medium"), 1),
        confidence_rank.get(suggestion.get("confidence", "medium"), 1),
        risk_rank.get(suggestion.get("estimated_risk", "medium"), 1),
        type_rank.get(suggestion.get("type", "unknown"), 2),
        _evidence_sort_value(suggestion),
        symbol,
    )


def _execution_plan_for_suggestion(suggestion: SuggestionPayload) -> SuggestionPayload:
    stype = str(suggestion.get("type", "unknown"))
    affected_files = [str(path) for path in suggestion.get("affected_files", [])]
    required_tests = [f"Run tests covering {path}" for path in affected_files[:3]] or [
        "Run the narrowest tests that cover the affected symbol"
    ]

    if stype == "split":
        return {
            "why_now": "The symbol crosses size or complexity thresholds in the evidence block.",
            "minimum_steps": [
                "Identify one cohesive responsibility to extract first.",
                "Move that responsibility behind a private helper or collaborator.",
                "Run focused tests and inspect detect_changes before continuing.",
            ],
            "safety_checks": [
                "Keep public names and call signatures stable in the first pass.",
                "Avoid moving unrelated logic while extracting the first responsibility.",
            ],
            "required_tests": required_tests,
            "rollback": (
                "Revert the extraction commit if focused tests or impact review widen unexpectedly."
            ),
            "defer_if": [
                "The symbol is public API and no caller contract is documented.",
                "No focused tests or reliable manual checks exist for the behavior.",
            ],
        }

    if stype == "move":
        return {
            "why_now": "Callers are concentrated in another community according to graph evidence.",
            "minimum_steps": [
                "Inspect all listed callers and imports.",
                "Move the symbol without changing behavior.",
                "Run tests for both source and target communities.",
            ],
            "safety_checks": [
                "Check for dynamic imports or framework registration.",
                "Preserve public re-export paths when downstream callers may exist.",
            ],
            "required_tests": required_tests,
            "rollback": "Move the symbol back if imports, packaging, or external callers break.",
            "defer_if": [
                "Unknown callers are present.",
                "The target community boundary is not stable.",
            ],
        }

    if stype == "remove":
        return {
            "why_now": "The graph found no callers, importers, references, tests, or subclasses.",
            "minimum_steps": [
                (
                    "Search for runtime registration, reflection, generated references, "
                    "and docs mentions."
                ),
                "Delete the smallest candidate first.",
                "Run focused tests and detect_changes before deleting more candidates.",
            ],
            "safety_checks": [
                "Treat public API, fixtures, and plugin entry points as high-risk.",
                "Verify generated code and downstream package exports manually.",
            ],
            "required_tests": required_tests,
            "rollback": "Restore the symbol if any dynamic or downstream reference appears.",
            "defer_if": [
                "The symbol is public API.",
                "The only evidence is absence from the graph and dynamic use is plausible.",
            ],
        }

    if stype == "document":
        return {
            "why_now": "The symbol is public or complex but has low explanation density.",
            "minimum_steps": [
                "Document contracts, invariants, and non-obvious edge cases.",
                "Avoid comments that restate individual statements.",
                "Run docs or lint checks if the repository provides them.",
            ],
            "safety_checks": [
                "Keep documentation close to the behavior it constrains.",
                "Update related Markdown references when the contract is user-facing.",
            ],
            "required_tests": ["Run formatting or lint checks for touched files"],
            "rollback": "Remove or tighten comments that drift from behavior during review.",
            "defer_if": [
                "The code is about to be rewritten.",
                "The intended contract is still unresolved.",
            ],
        }

    return {
        "why_now": "The suggestion has graph evidence but no specialized plan.",
        "minimum_steps": [
            "Inspect evidence, make the smallest safe change, then run focused tests."
        ],
        "safety_checks": [
            "Verify public APIs, generated code, and dynamic dispatch before editing."
        ],
        "required_tests": required_tests,
        "rollback": "Revert if impact review grows beyond the intended scope.",
        "defer_if": ["Evidence is ambiguous or the affected contract is unknown."],
    }


def _work_pack_for_suggestion(
    suggestion: SuggestionPayload,
    execution_plan: SuggestionPayload,
) -> SuggestionPayload:
    affected_files = [str(path) for path in suggestion.get("affected_files", [])]
    evidence = suggestion.get("evidence", {})
    split_pressure = (
        float(evidence.get("split_pressure", 0.0)) if isinstance(evidence, dict) else 0.0
    )
    stype = str(suggestion.get("type", "unknown"))
    estimated_size = "small"
    if len(affected_files) > 1 or split_pressure >= 10 or stype == "split":
        estimated_size = "medium"
    if split_pressure >= 25 or (stype == "split" and len(affected_files) > 2):
        estimated_size = "large"

    primary_file = affected_files[0] if affected_files else None
    owner_scope = Path(primary_file).parent.as_posix() if primary_file else None
    required_tests = list(execution_plan.get("required_tests", []))
    blast_radius = {
        "affected_file_count": len(affected_files),
        "affected_files": affected_files[:5],
        "symbol_count": len(suggestion.get("symbols", [])),
        "estimated_risk": suggestion.get("estimated_risk", "medium"),
    }
    documentation_obligations = []
    if stype in {"move", "remove", "split"}:
        documentation_obligations.append(
            "Check docs_for/implementations_of for authored contracts before editing."
        )
    if stype == "document":
        documentation_obligations.append(
            "Prefer contract, invariant, and edge-case documentation over line commentary."
        )
    return {
        "owner_scope": owner_scope,
        "primary_file": primary_file,
        "estimated_size": estimated_size,
        "blast_radius": blast_radius,
        "required_tests": required_tests,
        "documentation_obligations": documentation_obligations,
        "safe_first_commit": execution_plan.get("minimum_steps", ["Inspect evidence first."])[0],
        "rollback_path": execution_plan.get("rollback"),
        "defer_conditions": list(execution_plan.get("defer_if", [])),
        "first_commit": execution_plan.get("minimum_steps", ["Inspect evidence first."])[0],
        "verification_commands": required_tests,
        "success_criteria": [
            (
                "Public behavior and call signatures are unchanged unless the suggestion "
                "explicitly says otherwise."
            ),
            "Focused tests pass for the affected file or package.",
            'review_tool(mode="changes") does not show unexpected blast-radius growth.',
        ],
        "risk_controls": execution_plan.get("safety_checks", []),
    }


def _attach_execution_plan(suggestion: SuggestionPayload) -> SuggestionPayload:
    execution_plan = _execution_plan_for_suggestion(suggestion)
    enriched = {**suggestion, "execution_plan": execution_plan}
    return {**enriched, "work_pack": _work_pack_for_suggestion(enriched, execution_plan)}


def _is_public_api_node(node: Any, store: GraphStore, source_cache: dict[str, list[str]]) -> bool:
    record = {
        "language": node.language,
        "file": node.file_path,
        "line": node.line_start,
        "is_test": node.is_test,
    }
    lines = source_cache.setdefault(node.file_path, _load_source_lines(store, node.file_path))
    return _is_external_api_candidate(record, lines)


def _split_metrics(
    kind: str,
    length: int,
    branches: int,
    outgoing_calls: int,
    concern_profile: SuggestionPayload | None = None,
) -> SuggestionPayload | None:
    """Return split evidence when a code unit crosses kind-specific thresholds.

    Thresholds are intentionally kind-specific. Functions usually become hard to
    reason about through local branching and collaborators. Classes can be large
    containers where class-level call edges are sparse, so they need either high
    branch density or a much higher absolute size threshold.
    """
    if kind == "Function":
        length_ratio = length / _FUNCTION_SPLIT_MIN_LINES
        branch_ratio = branches / _FUNCTION_SPLIT_MIN_BRANCHES
        call_ratio = outgoing_calls / _FUNCTION_SPLIT_MIN_OUTGOING_CALLS
        concern_profile = concern_profile or {}
        concern_score = float(concern_profile.get("score", 0.0) or 0.0)
        concern_ratio = concern_score / _FUNCTION_CONCERN_SPLIT_SCORE
        secondary_ratio = max(branch_ratio, call_ratio, concern_ratio)
        if length_ratio < 1 or secondary_ratio < 1:
            return None
        reason_codes = ["large_function"]
        if branch_ratio >= 1:
            reason_codes.append("branch_heavy")
        if call_ratio >= 1:
            reason_codes.append("many_collaborators")
        if concern_ratio >= 1:
            reason_codes.append("function_concern_pressure")
            for reason in concern_profile.get("reason_codes", []):
                if reason not in reason_codes:
                    reason_codes.append(str(reason))
        return {
            "line_count": length,
            "branch_count": branches,
            "outgoing_call_count": outgoing_calls,
            "line_threshold": _FUNCTION_SPLIT_MIN_LINES,
            "branch_threshold": _FUNCTION_SPLIT_MIN_BRANCHES,
            "outgoing_call_threshold": _FUNCTION_SPLIT_MIN_OUTGOING_CALLS,
            "concern_score_threshold": _FUNCTION_CONCERN_SPLIT_SCORE,
            "split_pressure": round(length_ratio + secondary_ratio, 2),
            "reason_codes": reason_codes,
            "concern_separation": concern_profile,
        }

    if kind == "Class":
        length_ratio = length / _CLASS_SPLIT_MIN_LINES
        branch_ratio = branches / _CLASS_SPLIT_MIN_BRANCHES
        absolute_ratio = length / _CLASS_SPLIT_ABSOLUTE_LINES
        if not ((length_ratio >= 1 and branch_ratio >= 1) or absolute_ratio >= 1):
            return None
        reason_codes = ["large_class"]
        if branch_ratio >= 1:
            reason_codes.append("branch_heavy")
        if absolute_ratio >= 1:
            reason_codes.append("very_large_class")
        return {
            "line_count": length,
            "branch_count": branches,
            "outgoing_call_count": outgoing_calls,
            "line_threshold": _CLASS_SPLIT_MIN_LINES,
            "branch_threshold": _CLASS_SPLIT_MIN_BRANCHES,
            "absolute_line_threshold": _CLASS_SPLIT_ABSOLUTE_LINES,
            "split_pressure": round(max(length_ratio + branch_ratio, absolute_ratio), 2),
            "reason_codes": reason_codes,
        }

    return None


def _move_confidence(caller_count: int) -> str:
    if caller_count >= _MOVE_HIGH_CONFIDENCE_CALLERS:
        return "high"
    if caller_count >= _MOVE_MEDIUM_CONFIDENCE_CALLERS:
        return "medium"
    return "low"


def _structural_suggestions(
    store: GraphStore,
    excluded_qns: set[str],
    *,
    node_community: dict[str, int] | None = None,
) -> list[SuggestionPayload]:
    nodes = store.get_nodes_by_kind(["Function", "Class"])
    qns = [node.qualified_name for node in nodes]
    outgoing_by_qn, _ = store.get_edges_by_endpoints(qns)
    node_community = node_community or {}
    source_cache: dict[str, list[str]] = {}
    suggestions: list[SuggestionPayload] = []

    for node in nodes:
        if node.qualified_name in excluded_qns:
            continue
        lines = source_cache.setdefault(node.file_path, _load_source_lines(store, node.file_path))
        record = {
            "language": node.language,
            "file": node.file_path,
            "line": node.line_start,
            "is_test": node.is_test,
        }
        if _is_test_artifact(record, lines):
            continue

        span = _source_span(lines, node.line_start, node.line_end)
        length = node.line_end - node.line_start + 1
        outgoing_edges = outgoing_by_qn.get(node.qualified_name, [])
        outgoing_calls = sum(1 for edge in outgoing_edges if edge.kind == "CALLS")
        branches = concerns.branch_count(span)
        comments = concerns.comment_line_count(span)
        comment_ratio = comments / max(length, 1)
        is_public_api = _is_external_api_candidate(record, lines)
        concern_profile = concerns.function_concern_profile(
            node,
            span,
            outgoing_edges,
            node_community=node_community,
            branch_count_value=branches,
            comment_line_count_value=comments,
        )
        split_evidence = _split_metrics(
            node.kind,
            length,
            branches,
            outgoing_calls,
            concern_profile,
        )
        is_complex = split_evidence is not None

        if is_complex:
            suggestions.append(
                {
                    "type": "split",
                    "description": f"Split large {node.kind.lower()} '{_sanitize_name(node.name)}'",
                    "symbols": [_sanitize_name(node.qualified_name)],
                    "rationale": (
                        "The code unit is large and has complexity or concern-separation "
                        "pressure, so extraction or decomposition may reduce maintenance risk."
                    ),
                    "priority": "medium",
                    "confidence": "medium",
                    "category": "executable",
                    "estimated_risk": "medium",
                    "affected_files": [node.file_path],
                    "reason_codes": split_evidence["reason_codes"],
                    "evidence": split_evidence,
                    "verification_steps": [
                        "Identify cohesive sub-responsibilities before extracting code.",
                        "Run tests that cover the affected code path after splitting.",
                    ],
                }
            )

        if (
            (is_public_api or is_complex)
            and length >= _DOCUMENT_MIN_LINES
            and comment_ratio <= _LOW_COMMENT_RATIO
        ):
            reason_codes = ["low_explanation_density"]
            if is_public_api:
                reason_codes.append("public_api_candidate")
            if is_complex:
                reason_codes.append("complexity_candidate")
            suggestions.append(
                {
                    "type": "document",
                    "description": (
                        f"Document intent and invariants for '{_sanitize_name(node.name)}'"
                    ),
                    "symbols": [_sanitize_name(node.qualified_name)],
                    "rationale": (
                        "The code unit is public or complex, but has very low explanation density."
                    ),
                    "priority": "low",
                    "confidence": "medium",
                    "category": "public_api" if is_public_api else "executable",
                    "estimated_risk": "low",
                    "affected_files": [node.file_path],
                    "reason_codes": reason_codes,
                    "evidence": {
                        "line_count": length,
                        "comment_line_count": comments,
                        "comment_ratio": round(comment_ratio, 3),
                        "line_threshold": _DOCUMENT_MIN_LINES,
                        "comment_ratio_threshold": _LOW_COMMENT_RATIO,
                        "public_api_candidate": is_public_api,
                        "complexity_candidate": is_complex,
                    },
                    "verification_steps": [
                        "Add comments only for contracts, invariants, and non-obvious edge cases.",
                        "Avoid comments that restate the implementation line by line.",
                    ],
                }
            )

    return suggestions


def suggest_refactorings(store: GraphStore) -> list[SuggestionPayload]:
    """Produce community-driven refactoring suggestions.

    Currently four categories:
    - **move**: Functions in Community A only called by Community B.
    - **remove**: Dead code (no callers, tests, or importers and not entry points).
    - **split**: Large complex functions/classes worth decomposing.
    - **document**: Public or complex code with low explanation density.

    Returns:
        List of suggestion dicts with type, description, symbols, rationale.
    """
    suggestions: list[SuggestionPayload] = []

    community_rows = store.get_communities_list()
    node_community: dict[str, int] = {}

    if community_rows:
        members_by_id = store.get_all_community_member_qns()
        for crow in community_rows:
            cid = crow["id"]
            member_qns = members_by_id.get(cid, [])
            for qn in member_qns:
                node_community[qn] = cid

        community_names: dict[int, str] = {r["id"]: r["name"] for r in community_rows}

        all_funcs = store.get_nodes_by_kind(["Function"])
        func_qns = [fnode.qualified_name for fnode in all_funcs]
        _, incoming_by_qn = store.get_edges_by_endpoints(func_qns)

        source_cache: dict[str, list[str]] = {}

        for fnode in all_funcs:
            f_community = node_community.get(fnode.qualified_name)
            if f_community is None:
                continue
            if _is_public_api_node(fnode, store, source_cache):
                continue

            incoming_calls = [
                e for e in incoming_by_qn.get(fnode.qualified_name, []) if e.kind == "CALLS"
            ]
            if len(incoming_calls) < _MOVE_MIN_CALLERS:
                continue

            caller_communities = set()
            unknown_caller_community = False
            for edge in incoming_calls:
                c_community = node_community.get(edge.source_qualified)
                if c_community is not None:
                    caller_communities.add(c_community)
                else:
                    unknown_caller_community = True
            if unknown_caller_community:
                continue

            if len(caller_communities) == 1:
                target_community = next(iter(caller_communities))
                if target_community != f_community:
                    caller_count = len(incoming_calls)
                    confidence = _move_confidence(caller_count)
                    src_name = community_names.get(f_community, f"community-{f_community}")
                    tgt_name = community_names.get(
                        target_community, f"community-{target_community}"
                    )
                    suggestions.append(
                        {
                            "type": "move",
                            "description": (
                                f"Move '{_sanitize_name(fnode.name)}' from "
                                f"'{src_name}' to '{tgt_name}'"
                            ),
                            "symbols": [_sanitize_name(fnode.qualified_name)],
                            "rationale": (
                                f"Function is in community '{src_name}' but only "
                                f"called by members of community '{tgt_name}'."
                            ),
                            "priority": "medium",
                            "confidence": confidence,
                            "category": "executable",
                            "estimated_risk": "medium",
                            "affected_files": [fnode.file_path],
                            "reason_codes": [
                                "single_external_caller_community",
                                "no_unknown_callers",
                                "private_candidate",
                            ],
                            "evidence": {
                                "incoming_call_count": caller_count,
                                "caller_community_count": len(caller_communities),
                                "unknown_caller_count": 0,
                                "minimum_call_threshold": _MOVE_MIN_CALLERS,
                                "medium_confidence_threshold": _MOVE_MEDIUM_CONFIDENCE_CALLERS,
                                "high_confidence_threshold": _MOVE_HIGH_CONFIDENCE_CALLERS,
                                "source_community": src_name,
                                "target_community": tgt_name,
                            },
                            "verification_steps": [
                                "Review imports and call sites before moving the function.",
                                "Run tests for both source and target communities.",
                            ],
                        }
                    )

    dead = find_dead_code(store)
    dead_qns = {d["qualified_name"] for d in dead}
    source_cache: dict[str, list[str]] = {}
    for d in dead:
        evidence = d.get("evidence", {})
        lines = source_cache.setdefault(d["file"], _load_source_lines(store, d["file"]))
        if _is_test_artifact(d, lines):
            continue
        if _is_external_api_candidate(d, lines):
            reason_codes = list(d.get("reason_codes", []))
            if "public_api_candidate" not in reason_codes:
                reason_codes.append("public_api_candidate")
            d = {
                **d,
                "confidence": "low",
                "public_api_candidate": True,
                "reason_codes": reason_codes,
            }
        category = _dead_code_category(d)
        kind_name = str(d.get("kind", "")).lower()
        name = str(d.get("name", ""))
        estimated_risk = "high" if category == "public_api" else "medium"
        verification_steps = [
            "Search for runtime registration or dynamic dispatch before deleting.",
            "Run the tests that cover the affected file or package.",
        ]
        if category == "public_api":
            verification_steps.insert(
                0,
                "Verify crate-level and downstream API consumers before deleting.",
            )
        suggestions.append(
            {
                "type": "remove",
                "description": f"Remove unused {kind_name} '{name}'",
                "symbols": [d["qualified_name"]],
                "rationale": (
                    "No callers, test references, importers, references, or subclasses "
                    "were found in the graph."
                ),
                "priority": "low",
                "confidence": d.get("confidence", "medium"),
                "category": category,
                "estimated_risk": estimated_risk,
                "affected_files": [d["file"]],
                "reason_codes": d.get("reason_codes", []),
                "evidence": evidence,
                "verification_steps": verification_steps,
            }
        )

    suggestions.extend(_structural_suggestions(store, dead_qns, node_community=node_community))
    suggestions = [_attach_execution_plan(suggestion) for suggestion in suggestions]
    suggestions.sort(key=_suggestion_sort_key)
    logger.info("suggest_refactorings: produced %d suggestions", len(suggestions))
    return suggestions
