"""Heuristic test-coverage helpers used by graph query and review tools."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Callable, TypedDict

from .graph import GraphNode, node_to_dict
from .state_types import ChangeNodeRecord

_TEST_FILE_PARTS = ("/tests/", "/test/", "/__tests__/")
_TEST_FILE_SUFFIXES = (
    "_test.py",
    "_tests.py",
    ".test.js",
    ".test.ts",
    ".test.tsx",
    ".spec.js",
    ".spec.ts",
    ".spec.tsx",
    "_test.rs",
    "_tests.rs",
)
_NON_TEST_HELPER_NAMES = {
    "setup",
    "teardown",
    "setup_method",
    "teardown_method",
    "setup_class",
    "teardown_class",
    "setup_module",
    "teardown_module",
    "setUp".casefold(),
    "tearDown".casefold(),
}
_MODULE_MARKER_SKIP = frozenset({"src", "lib", "pkg", "internal", "tests", "test"})


class CoverageRecord(ChangeNodeRecord, total=False):
    confidence: str
    evidence: list[str]
    coverage_source: str


class ScanState(TypedDict):
    candidates: list[tuple[GraphNode, str, str]]
    import_edges_by_source: dict[str, list[Any]]
    token_cache: dict[str, tuple[list[str], str]]
    source_cache: dict[str, list[str]]


def is_test_file_path(file_path: str) -> bool:
    """Return whether *file_path* looks like a test artifact path."""
    normalized = file_path.replace("\\", "/")
    name = normalized.rsplit("/", 1)[-1]
    return (
        any(part in normalized for part in _TEST_FILE_PARTS)
        or name.startswith("test_")
        or name in {"tests.rs", "test.rs"}
        or name.endswith(_TEST_FILE_SUFFIXES)
    )


def _identifier_tokens(value: str) -> list[str]:
    camel_spaced = re.sub(r"([a-z])([A-Z])", r"\1_\2", value)
    camel_spaced = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", camel_spaced)
    tokens = re.findall(r"[a-z0-9]+", camel_spaced.casefold().replace("_", " "))
    return [token for token in tokens if token and token not in {"test", "tests"}]


def _squashed_identifier(value: str) -> str:
    return "".join(_identifier_tokens(value))


_word_pattern_cache: dict[str, re.Pattern[str]] = {}


def _word_boundary_pattern(value: str) -> re.Pattern[str]:
    cached = _word_pattern_cache.get(value)
    if cached is not None:
        return cached
    escaped = re.escape(value.casefold())
    pattern = re.compile(rf"(?<![a-z0-9]){escaped}(?![a-z0-9])")
    if len(_word_pattern_cache) < 4096:
        _word_pattern_cache[value] = pattern
    return pattern


def _contains_word(haystack: str, needle: str) -> bool:
    if not needle:
        return False
    # Fast path: a word-boundary match requires the plain substring, and the
    # substring scan is a C-level search (~100ns) vs a regex (~1-2µs). The
    # regex still guards the result, so semantics are unchanged.
    cf_haystack = haystack.casefold()
    if needle.casefold() not in cf_haystack:
        return False
    return _word_boundary_pattern(needle).search(cf_haystack) is not None


def build_scan_state(store: Any) -> ScanState:
    """Build a per-analysis candidate scan state.

    ``infer_tests_for_node`` / ``has_coverage_evidence`` used to re-fetch and
    re-filter the full candidate list (and re-parse every import edge) for
    every changed function. Reviewing N changed functions therefore cost
    N full-graph scans. Building the candidate list, the import-edge map, and
    the token/source caches once per analysis pass removes that N×.

    Each candidate is stored with its precomputed casefolded path and identity
    strings so repeated scoring loops do not rebuild them per target.
    """
    candidates = store.get_nodes_by_kind(["Test", "Function", "Class"])
    test_like: list[tuple[GraphNode, str, str]] = []
    candidate_keys: list[str] = []
    for candidate in candidates:
        if not _is_test_like_node(candidate):
            continue
        path_cf = candidate.file_path.replace("\\", "/").casefold()
        identity_cf = (
            f"{candidate.qualified_name} {candidate.name} {candidate.file_path}".casefold()
        )
        test_like.append((candidate, path_cf, identity_cf))
        candidate_keys.append(candidate.file_path)
        candidate_keys.append(candidate.qualified_name)
    return {
        "candidates": test_like,
        "import_edges_by_source": _build_import_edges_by_source(store, candidate_keys),
        "token_cache": {},
        "source_cache": {},
    }


def _is_test_like_node(node: GraphNode) -> bool:
    name = node.name.casefold()
    qn = node.qualified_name.casefold()
    if node.kind == "Function" and not node.is_test and name in _NON_TEST_HELPER_NAMES:
        return False
    if node.kind == "Function" and not node.is_test:
        return (
            name.startswith(("test_", "test")) or "::test" in qn or ".test." in qn or ".spec." in qn
        )
    return (
        bool(node.is_test)
        or node.kind == "Test"
        or name.startswith(("test_", "test"))
        or ".test." in qn
        or "::test" in qn
        or is_test_file_path(node.file_path)
    )


def _load_source_lines(store: Any, file_path: str) -> list[str]:
    try:
        path = store.resolve_file_path(file_path)
    except (AttributeError, TypeError):
        path = Path(file_path)
    try:
        return path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []


def _span_text(
    store: Any,
    node: GraphNode,
    source_cache: dict[str, list[str]] | None = None,
) -> str:
    file_path = node.file_path
    if source_cache is not None and file_path in source_cache:
        lines = source_cache[file_path]
    else:
        lines = _load_source_lines(store, file_path)
        if source_cache is not None:
            source_cache[file_path] = lines
    if not lines or node.line_start <= 0 or node.line_end < node.line_start:
        return ""
    return "\n".join(lines[node.line_start - 1 : min(node.line_end, len(lines))]).casefold()


def _coverage_record(
    node: GraphNode,
    *,
    confidence: str,
    evidence: list[str],
    source: str,
) -> CoverageRecord:
    return {
        **node_to_dict(node),
        "confidence": confidence,
        "evidence": evidence,
        "coverage_source": source,
    }


def _target_file_stem(target: GraphNode) -> str:
    return Path(target.file_path.replace("\\", "/")).stem.casefold()


def _target_module_markers(target: GraphNode) -> set[str]:
    markers: set[str] = set()
    path = Path(target.file_path.replace("\\", "/"))
    stem = path.stem.casefold()
    if stem:
        markers.add(stem)
        if stem.startswith("test_"):
            markers.add(stem[5:])
        elif stem.endswith("_test"):
            markers.add(stem[:-5])
    parent = path.parent.name.casefold()
    if parent and parent not in _MODULE_MARKER_SKIP:
        markers.add(parent)
    return markers


def _build_import_edges_by_source(store: Any, candidate_keys: list[str]) -> dict[str, list[Any]]:
    """Batch-load IMPORTS_FROM edges for candidate source keys.

    The old per-candidate ``get_edges_by_source`` loop issued one SQL query
    (and JSON-parsed every row) per candidate — tens of thousands of queries
    per ``infer_tests_for_node`` call on larger graphs. A single kind-filtered
    query returns only the IMPORTS_FROM rows the heuristic needs; when the
    store does not expose it, fall back to a batched endpoint query.
    """
    if not candidate_keys:
        return {}
    get_by_kind: Callable[[str], list[Any]] | None = getattr(store, "get_edges_by_kind", None)
    if get_by_kind is not None:
        try:
            edges = get_by_kind("IMPORTS_FROM")
        except Exception:  # pragma: no cover - defensive for backend parity drift
            edges = []
        wanted = set(candidate_keys)
        by_source: dict[str, list[Any]] = {}
        for edge in edges:
            src = getattr(edge, "source_qualified", "")
            if src in wanted:
                by_source.setdefault(src, []).append(edge)
        return by_source
    try:
        outgoing, _ = store.get_edges_by_endpoints(list(candidate_keys))
    except Exception:  # pragma: no cover - defensive for backend parity drift
        return {}
    return {
        key: [edge for edge in edges if getattr(edge, "kind", None) == "IMPORTS_FROM"]
        for key, edges in outgoing.items()
    }


def _candidate_references_target_module(
    store: Any,
    target: GraphNode,
    candidate: GraphNode,
    import_edges_by_source: dict[str, list[Any]] | None = None,
    *,
    target_markers: set[str] | None = None,
    target_stem: str | None = None,
    target_path_cf: str | None = None,
    candidate_path_cf: str | None = None,
    candidate_identity_cf: str | None = None,
) -> tuple[bool, list[str]]:
    evidence: list[str] = []
    candidate_path = candidate_path_cf
    if candidate_path is None:
        candidate_path = candidate.file_path.replace("\\", "/").casefold()
    candidate_identity = candidate_identity_cf
    if candidate_identity is None:
        candidate_identity = (
            f"{candidate.qualified_name} {candidate.name} {candidate.file_path}".casefold()
        )
    target_path = target_path_cf
    if target_path is None:
        target_path = target.file_path.replace("\\", "/").casefold()

    if target_markers is None:
        target_markers = _target_module_markers(target)
    # Substring fast path (both sides already casefolded) before the
    # word-boundary regex; most candidates share no marker at all.
    for marker in sorted(target_markers, key=len, reverse=True):
        if marker in candidate_path or marker in candidate_identity:
            if _contains_word(candidate_path, marker) or _contains_word(candidate_identity, marker):
                evidence.append("test artifact references target module/file")
                return True, evidence

    if target_stem is None:
        target_stem = _target_file_stem(target)
    if target_stem and (
        candidate_path.startswith(f"{target_path}.")
        or candidate_path.startswith(f"{target_path}_")
        or _contains_word(Path(candidate_path).name, target_stem)
    ):
        evidence.append("test file co-located with target module")
        return True, evidence

    if import_edges_by_source is None:
        import_edges_by_source = {}
    for source_key in dict.fromkeys([candidate.file_path, candidate.qualified_name]):
        for edge in import_edges_by_source.get(source_key, []):
            import_target = edge.target_qualified.replace("\\", "/").casefold()
            if import_target == target_path or import_target.startswith(f"{target_path}::"):
                evidence.append("test file imports target module")
                return True, evidence

    return False, evidence


def _name_matches_target_symbol(
    target_name: str,
    candidate_text: str,
    *,
    allowed_suffix_tokens: set[str] | None = None,
    token_cache: dict[str, tuple[list[str], str]] | None = None,
) -> bool:
    def tokens_for(value: str) -> tuple[list[str], str]:
        if token_cache is not None and value in token_cache:
            return token_cache[value]
        tokens = _identifier_tokens(value)
        squashed = _squashed_identifier(value)
        if token_cache is not None:
            token_cache[value] = (tokens, squashed)
        return tokens, squashed

    target_cf = target_name.casefold()
    candidate_cf = candidate_text.casefold()
    if target_cf == candidate_cf:
        return True

    for prefix in ("test_", "test"):
        if candidate_cf.startswith(prefix):
            remainder = candidate_cf[len(prefix) :].lstrip("_")
            if remainder == target_cf:
                return True

    if tokens_for(target_name)[1] == tokens_for(candidate_text)[1]:
        return True

    target_tokens = tokens_for(target_name)[0]
    candidate_tokens = tokens_for(candidate_text)[0]
    if not target_tokens or len(target_tokens) > len(candidate_tokens):
        return False

    suffix_allow = allowed_suffix_tokens or set()
    for index in range(len(candidate_tokens) - len(target_tokens) + 1):
        if candidate_tokens[index : index + len(target_tokens)] != target_tokens:
            continue
        before = candidate_tokens[:index]
        after = candidate_tokens[index + len(target_tokens) :]
        if before:
            continue
        if not after:
            return True
        if all(token in suffix_allow for token in after):
            return True
    return False


def _candidate_symbol_text(candidate: GraphNode) -> str:
    return candidate.name


def _candidate_score(
    store: Any,
    target: GraphNode,
    candidate: GraphNode,
    target_tokens: list[str],
    target_squashed: str,
    import_edges_by_source: dict[str, list[Any]] | None = None,
    source_cache: dict[str, list[str]] | None = None,
    token_cache: dict[str, tuple[list[str], str]] | None = None,
    *,
    target_markers: set[str] | None = None,
    target_stem: str | None = None,
    target_path_cf: str | None = None,
    candidate_path_cf: str | None = None,
    candidate_identity_cf: str | None = None,
) -> tuple[int, str, list[str]]:
    del target_tokens, target_squashed
    evidence: list[str] = []
    candidate_identity = candidate_identity_cf
    if candidate_identity is None:
        candidate_identity = (
            f"{candidate.qualified_name} {candidate.name} {candidate.file_path}".casefold()
        )
    candidate_symbol = _candidate_symbol_text(candidate)
    module_linked, module_evidence = _candidate_references_target_module(
        store,
        target,
        candidate,
        import_edges_by_source=import_edges_by_source,
        target_markers=target_markers,
        target_stem=target_stem,
        target_path_cf=target_path_cf,
        candidate_path_cf=candidate_path_cf,
        candidate_identity_cf=candidate_identity_cf,
    )
    suffix_allow = _target_module_markers(target) if target_markers is None else target_markers

    if module_linked:
        evidence.extend(module_evidence)
        if _name_matches_target_symbol(
            target.name,
            candidate_symbol,
            allowed_suffix_tokens=suffix_allow,
            token_cache=token_cache,
        ):
            evidence.append("test node name references target symbol")
            return 80, "medium", evidence

        span = _span_text(store, candidate, source_cache=source_cache)
        if span and _name_matches_target_symbol(
            target.name,
            span,
            allowed_suffix_tokens=suffix_allow,
            token_cache=token_cache,
        ):
            evidence.append("test source references target symbol")
            return 65, "medium", evidence

    target_file_stem = _target_file_stem(target) if target_stem is None else target_stem
    if target_file_stem and _contains_word(candidate_identity, target_file_stem):
        evidence.append("test node name references target file stem")
        return 35, "low", evidence
    if _name_matches_target_symbol(
        target.name,
        candidate_symbol,
        token_cache=token_cache,
    ):
        evidence.append("test node name resembles target symbol (no module link)")
        return 25, "low", evidence

    return 0, "low", evidence


def infer_tests_for_node(
    store: Any,
    target: GraphNode,
    *,
    limit: int = 25,
    minimum_confidence: str = "medium",
    _scan_state: ScanState | None = None,
) -> list[CoverageRecord]:
    """Infer tests for *target* from graph edges, names, and local test source.

    Direct ``TESTED_BY`` edges are treated as high-confidence facts. Naming and
    source-reference matches are marked as heuristic evidence so callers can
    distinguish strong coverage from useful leads.

    ``_scan_state`` (built by :func:`build_scan_state`) shares the candidate
    list, import-edge map, and token/source caches across many targets in one
    review pass so the per-target cost is one scoring loop instead of a full
    scan.
    """
    min_rank = {"low": 0, "medium": 1, "high": 2}.get(minimum_confidence, 0)
    confidence_rank = {"low": 0, "medium": 1, "high": 2}
    results: dict[str, tuple[int, CoverageRecord]] = {}

    direct_edges = []
    seen_direct_edge_ids: set[int] = set()
    for edge in store.get_edges_by_source(target.qualified_name):
        if edge.kind != "TESTED_BY":
            continue
        if edge.id in seen_direct_edge_ids:
            continue
        seen_direct_edge_ids.add(edge.id)
        direct_edges.append(edge)
    if not direct_edges:
        for edge in store.get_edges_by_source(target.name):
            if edge.kind != "TESTED_BY":
                continue
            if edge.id in seen_direct_edge_ids:
                continue
            seen_direct_edge_ids.add(edge.id)
            direct_edges.append(edge)
    direct_targets = [edge.target_qualified for edge in direct_edges]
    direct_nodes = store.get_nodes_by_qualified_names(direct_targets)
    for edge in direct_edges:
        test_node = direct_nodes.get(edge.target_qualified)
        if test_node is None:
            continue
        record = _coverage_record(
            test_node,
            confidence="high",
            evidence=["TESTED_BY edge"],
            source="graph_edge",
        )
        results[test_node.qualified_name] = (100, record)

    if _scan_state is None:
        _scan_state = build_scan_state(store)
    candidates = _scan_state["candidates"]
    import_edges_by_source = _scan_state["import_edges_by_source"]
    source_cache = _scan_state["source_cache"]
    token_cache = _scan_state["token_cache"]

    target_tokens = _identifier_tokens(target.name)
    target_squashed = _squashed_identifier(target.name)
    target_markers = _target_module_markers(target)
    target_stem = _target_file_stem(target)
    target_path_cf = target.file_path.replace("\\", "/").casefold()
    # When the caller only needs "does any test cover this" (limit=1), the
    # maximum possible score is 80, so the first 80-scoring candidate is a
    # final answer: stop scanning instead of scoring all 40k+ candidates.
    early_exit_on_max = limit <= 1 and min_rank <= confidence_rank["medium"]
    for candidate, candidate_path_cf, candidate_identity_cf in candidates:
        if candidate.qualified_name == target.qualified_name:
            continue
        score, confidence, evidence = _candidate_score(
            store,
            target,
            candidate,
            target_tokens,
            target_squashed,
            import_edges_by_source=import_edges_by_source,
            source_cache=source_cache,
            token_cache=token_cache,
            target_markers=target_markers,
            target_stem=target_stem,
            target_path_cf=target_path_cf,
            candidate_path_cf=candidate_path_cf,
            candidate_identity_cf=candidate_identity_cf,
        )
        if score <= 0 or confidence_rank[confidence] < min_rank:
            continue
        current = results.get(candidate.qualified_name)
        if current is not None and current[0] >= score:
            continue
        record = _coverage_record(
            candidate,
            confidence=confidence,
            evidence=evidence,
            source="heuristic",
        )
        results[candidate.qualified_name] = (score, record)
        if early_exit_on_max and score >= 80:
            break

    return [
        record
        for _, record in sorted(
            results.values(),
            key=lambda item: (-item[0], item[1]["qualified_name"]),
        )[:limit]
    ]


def has_coverage_evidence(
    store: Any,
    target: GraphNode,
    *,
    minimum_confidence: str = "medium",
    caller_depth: int = 2,
    _seen: set[str] | None = None,
    _scan_state: ScanState | None = None,
) -> bool:
    """Return whether *target* has direct or credible heuristic test evidence.

    ``_scan_state`` may be shared by :func:`build_scan_state` across many
    targets in one review pass to avoid re-scanning the full candidate list
    per changed function.
    """
    if infer_tests_for_node(
        store,
        target,
        limit=1,
        minimum_confidence=minimum_confidence,
        _scan_state=_scan_state,
    ):
        return True
    if caller_depth <= 0 or not target.name.startswith("_"):
        return False

    seen = set() if _seen is None else set(_seen)
    if target.qualified_name in seen:
        return False
    seen.add(target.qualified_name)

    caller_edges = [
        edge for edge in store.get_edges_by_target(target.qualified_name) if edge.kind == "CALLS"
    ]
    caller_nodes = store.get_nodes_by_qualified_names(
        [edge.source_qualified for edge in caller_edges]
    )
    for caller in caller_nodes.values():
        if has_coverage_evidence(
            store,
            caller,
            minimum_confidence=minimum_confidence,
            caller_depth=caller_depth - 1,
            _seen=seen,
            _scan_state=_scan_state,
        ):
            return True
    return False
