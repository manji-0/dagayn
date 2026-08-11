"""Change impact analysis for code review.

Maps git/svn diffs to affected functions, flows, communities, and test coverage
gaps. Produces risk-scored, priority-ordered review guidance.
"""

from __future__ import annotations

import functools
import hashlib
import json
import logging
import os
import re
import subprocess
from pathlib import Path, PurePosixPath
from typing import Any

from .constants import SECURITY_KEYWORDS as _SECURITY_KEYWORDS
from .coverage import has_coverage_evidence
from .flows import get_affected_flows
from .graph import GraphEdge, GraphNode, GraphStore, _sanitize_name, edge_to_dict, node_to_dict
from .parser import CodeParser
from .parser._base.types import EdgeInfo, NodeInfo

logger = logging.getLogger(__name__)


# Sentinel that distinguishes "not provided" from an explicit ``None`` for
# parameters where ``None`` is a meaningful value (e.g. a node having no
# community is represented as ``None``).
class _UnsetType:
    pass


_UNSET = _UnsetType()

_GIT_TIMEOUT = int(os.environ.get("CRG_GIT_TIMEOUT", "30"))  # seconds, configurable

_SAFE_GIT_REF = re.compile(r"^[A-Za-z0-9_.~^/@{}\-]+$")
_SAFE_SVN_REV = re.compile(r"^r?\d+(:r?\d+|:HEAD|:BASE|:COMMITTED)?$", re.IGNORECASE)


# ---------------------------------------------------------------------------
# 1. parse_git_diff_ranges / parse_svn_diff_ranges
# ---------------------------------------------------------------------------


def parse_git_diff_ranges(
    repo_root: str,
    base: str = "HEAD~1",
) -> dict[str, list[tuple[int, int]]]:
    """Run ``git diff --unified=0`` and extract changed line ranges per file.

    Args:
        repo_root: Absolute path to the repository root.
        base: Git ref to diff against (default: ``HEAD~1``).

    Returns:
        Mapping of file paths to lists of ``(start_line, end_line)`` tuples.
        Returns an empty dict on error.
    """
    if not _SAFE_GIT_REF.match(base):
        logger.warning("Invalid git ref rejected: %s", base)
        return {}
    try:
        result = subprocess.run(
            ["git", "diff", "--unified=0", base, "--"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=repo_root,
            timeout=_GIT_TIMEOUT,
        )
        if result.returncode != 0:
            logger.warning("git diff failed (rc=%d): %s", result.returncode, result.stderr[:200])
            return {}
    except (OSError, subprocess.SubprocessError) as exc:
        logger.warning("git diff error: %s", exc)
        return {}

    return _parse_unified_diff(result.stdout)


def parse_svn_diff_ranges(
    repo_root: str,
    rev_range: str | None = None,
) -> dict[str, list[tuple[int, int]]]:
    """Run ``svn diff`` and extract changed line ranges per file.

    Args:
        repo_root: Absolute path to the SVN working copy root.
        rev_range: Optional SVN revision range in ``rXXX:HEAD`` format.
            When *None*, diffs the working copy against BASE (local changes).

    Returns:
        Mapping of file paths to lists of ``(start_line, end_line)`` tuples.
        Returns an empty dict on error.
    """
    cmd = ["svn", "diff", "--non-interactive"]
    if rev_range:
        if not _SAFE_SVN_REV.match(rev_range):
            logger.warning("Invalid SVN revision range rejected: %s", rev_range)
            return {}
        cmd.extend(["-r", rev_range])
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=repo_root,
            timeout=_GIT_TIMEOUT,
        )
        if result.returncode != 0:
            logger.warning("svn diff failed (rc=%d): %s", result.returncode, result.stderr[:200])
            return {}
    except (OSError, subprocess.SubprocessError) as exc:
        logger.warning("svn diff error: %s", exc)
        return {}

    return _parse_unified_diff(result.stdout)


def _diff_ranges_cache_stamp(repo_root: str) -> str:
    """Return a cheap stamp so cached diff ranges invalidate when the tree changes."""
    root = Path(repo_root)
    if (root / ".svn").exists():
        return _svn_diff_cache_stamp(root)
    return _git_diff_cache_stamp(root)


def _git_diff_cache_stamp(root: Path) -> str:
    head = ""
    porcelain = ""
    try:
        head_result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            cwd=str(root),
            timeout=_GIT_TIMEOUT,
        )
        if head_result.returncode == 0:
            head = head_result.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        pass
    try:
        status_result = subprocess.run(
            ["git", "status", "--porcelain", "--untracked-files=all"],
            capture_output=True,
            text=True,
            cwd=str(root),
            timeout=_GIT_TIMEOUT,
        )
        if status_result.returncode == 0:
            porcelain = status_result.stdout
    except (OSError, subprocess.SubprocessError):
        pass
    if not head and not porcelain:
        return "0"
    status_hash = hashlib.sha256(porcelain.encode()).hexdigest()[:16]
    return f"{head}:{status_hash}"


def _svn_diff_cache_stamp(root: Path) -> str:
    revision = ""
    status = ""
    try:
        rev_result = subprocess.run(
            ["svn", "info", "--show-item", "revision", "--non-interactive"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=str(root),
            timeout=_GIT_TIMEOUT,
        )
        if rev_result.returncode == 0:
            revision = rev_result.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        pass
    try:
        status_result = subprocess.run(
            ["svn", "status", "--non-interactive"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=str(root),
            timeout=_GIT_TIMEOUT,
        )
        if status_result.returncode == 0:
            status = status_result.stdout
    except (OSError, subprocess.SubprocessError):
        pass
    status_hash = hashlib.sha256(status.encode()).hexdigest()[:16]
    return f"{revision}:{status_hash}"


def parse_diff_ranges(
    repo_root: str,
    base: str = "HEAD~1",
) -> dict[str, list[tuple[int, int]]]:
    """Auto-detect VCS and return changed line ranges per file.

    Dispatches to :func:`parse_git_diff_ranges` for Git repositories and
    :func:`parse_svn_diff_ranges` for SVN working copies.

    Args:
        repo_root: Absolute path to the repository/working-copy root.
        base: For Git: the ref to diff against (default ``HEAD~1``).
              For SVN: an optional revision range (e.g. ``"r100:HEAD"``);
              when *base* is not a valid SVN revision, working-copy changes
              (``svn diff``) are used instead.
    """
    root = str(Path(repo_root).resolve())
    stamp = _diff_ranges_cache_stamp(root)
    cached = _parse_diff_ranges_cached(root, base, stamp)
    return {path: list(ranges) for path, ranges in cached}


@functools.lru_cache(maxsize=64)
def _parse_diff_ranges_cached(
    repo_root: str,
    base: str,
    cache_stamp: str,
) -> tuple[tuple[str, tuple[tuple[int, int], ...]], ...]:
    """Cached diff range parser with an immutable result payload."""
    root_path = Path(repo_root)
    if (root_path / ".svn").exists():
        rev_range = base if _SAFE_SVN_REV.match(base) else None
        ranges = parse_svn_diff_ranges(repo_root, rev_range)
    else:
        ranges = parse_git_diff_ranges(repo_root, base)
    return tuple((path, tuple(path_ranges)) for path, path_ranges in sorted(ranges.items()))


def _decode_git_quoted_path(text: str) -> str:
    """Decode a git C-style quoted path (``core.quotePath``)."""
    if not (text.startswith('"') and text.endswith('"')):
        return text

    inner = text[1:-1]
    out = bytearray()
    i = 0
    while i < len(inner):
        ch = inner[i]
        if ch != "\\":
            out.extend(ch.encode("utf-8"))
            i += 1
            continue
        i += 1
        if i >= len(inner):
            break
        esc = inner[i]
        if esc in ('\\', '"'):
            out.extend(esc.encode("ascii"))
            i += 1
        elif esc in "01234567":
            octal = esc
            i += 1
            for _ in range(2):
                if i < len(inner) and inner[i] in "01234567":
                    octal += inner[i]
                    i += 1
                else:
                    break
            out.append(int(octal, 8))
        elif esc == "n":
            out.append(ord("\n"))
            i += 1
        elif esc == "t":
            out.append(ord("\t"))
            i += 1
        elif esc == "r":
            out.append(ord("\r"))
            i += 1
        else:
            out.extend(esc.encode("ascii"))
            i += 1
    return out.decode("utf-8", errors="replace")


def _parse_plus_plus_path(line: str) -> str | None:
    """Return the new-file path from a ``+++`` header, or ``None`` when absent."""
    if not line.startswith("+++"):
        return None
    raw = line[4:].strip()
    if not raw or raw == "/dev/null":
        return None
    path = _decode_git_quoted_path(raw)
    if "\t" in path:
        path = path.split("\t", 1)[0]
    if path.startswith("b/"):
        return path[2:]
    return None


def _parse_unified_diff(diff_text: str) -> dict[str, list[tuple[int, int]]]:
    """Parse unified diff output into file -> line-range mappings.

    Handles the ``@@ -old,count +new,count @@`` hunk header format.
    """
    ranges: dict[str, list[tuple[int, int]]] = {}
    current_file: str | None = None

    # Match "@@ ... +start,count @@" or "@@ ... +start @@"
    hunk_pattern = re.compile(r"^@@ .+? \+(\d+)(?:,(\d+))? @@")

    for line in diff_text.splitlines():
        if line.startswith("+++"):
            current_file = _parse_plus_plus_path(line)
            continue

        hunk_match = hunk_pattern.match(line)
        if hunk_match and current_file is not None:
            start = int(hunk_match.group(1))
            count = int(hunk_match.group(2)) if hunk_match.group(2) else 1
            if count == 0:
                # Pure deletion hunk (no lines added); still note the position.
                end = start
            else:
                end = start + count - 1
            ranges.setdefault(current_file, []).append((start, end))

    return ranges


# ---------------------------------------------------------------------------
# 2. map_changes_to_nodes
# ---------------------------------------------------------------------------


def map_changes_to_nodes(
    store: GraphStore,
    changed_ranges: dict[str, list[tuple[int, int]]],
) -> list[GraphNode]:
    """Find graph nodes whose line ranges overlap the changed lines.

    Args:
        store: The graph store.
        changed_ranges: Mapping of file paths to ``(start, end)`` tuples.

    Returns:
        Deduplicated list of overlapping graph nodes.
    """
    seen: set[str] = set()
    result: list[GraphNode] = []

    nodes_by_file = _get_nodes_for_files_boundary_aware(store, list(changed_ranges))
    for file_path, ranges in changed_ranges.items():
        # Try the path as-is, then also try all nodes to match relative paths.
        nodes = list(nodes_by_file.get(file_path, []))
        if not nodes:
            # The graph may store absolute paths; try a suffix match.
            matched_paths = store.get_files_matching(file_path)
            if matched_paths:
                matched_nodes = _get_nodes_for_files_boundary_aware(store, matched_paths)
                for mp in matched_paths:
                    nodes.extend(matched_nodes.get(mp, []))

        for node in nodes:
            if node.qualified_name in seen:
                continue
            if node.line_start is None or node.line_end is None:
                continue
            # Check overlap with any changed range.
            for start, end in ranges:
                if node.line_start <= end and node.line_end >= start:
                    result.append(node)
                    seen.add(node.qualified_name)
                    break

    return result


def _get_nodes_for_files_boundary_aware(
    store: GraphStore,
    file_paths: list[str],
) -> dict[str, list[GraphNode]]:
    rust_batch = getattr(store, "get_nodes_by_files", None)
    if callable(rust_batch) and type(store).__module__.startswith("dagayn._core"):
        return rust_batch(file_paths)
    return {file_path: store.get_nodes_by_file(file_path) for file_path in file_paths}


def _git_show_file(repo_root: str, base: str, rel_path: str) -> bytes | None:
    if not _SAFE_GIT_REF.match(base):
        return None
    try:
        result = subprocess.run(
            ["git", "show", f"{base}:{rel_path}"],
            capture_output=True,
            cwd=repo_root,
            timeout=_GIT_TIMEOUT,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    return result.stdout


def _repo_relative_path(file_path: str, repo_root: str | None) -> str:
    if repo_root is None:
        return file_path
    path = Path(file_path)
    if path.is_absolute():
        try:
            return path.relative_to(Path(repo_root).resolve()).as_posix()
        except ValueError:
            return path.name
    return PurePosixPath(file_path).as_posix()


def _node_qn(node: NodeInfo) -> str:
    if node.kind == "File":
        return node.file_path
    if node.parent_name:
        return f"{node.file_path}::{node.parent_name}.{node.name}"
    return f"{node.file_path}::{node.name}"


def _edge_info_signature(edge: EdgeInfo) -> tuple[str, str, str, str]:
    return (edge.kind, edge.source, edge.target, edge.file_path)


def _edge_signature(edge: GraphEdge) -> tuple[str, str, str, str]:
    return (edge.kind, edge.source_qualified, edge.target_qualified, edge.file_path)


def _base_entity_sets(
    repo_root: str | None,
    base: str,
    changed_nodes: list[GraphNode],
) -> tuple[set[str], set[tuple[str, str, str, str]]]:
    """Parse base-ref file contents and return node/edge identity sets."""
    if repo_root is None:
        return set(), set()

    parser = CodeParser()
    base_node_qns: set[str] = set()
    base_edge_signatures: set[tuple[str, str, str, str]] = set()
    display_paths_by_rel: dict[str, str] = {}
    for node in changed_nodes:
        rel_path = _repo_relative_path(node.file_path, repo_root)
        display_paths_by_rel.setdefault(rel_path, node.file_path)

    for rel_path, display_path in display_paths_by_rel.items():
        source = _git_show_file(repo_root, base, rel_path)
        if source is None:
            continue
        try:
            nodes, edges = parser.parse_bytes(Path(display_path), source)
        except RuntimeError as exc:
            logger.debug("Could not parse base file %s at %s: %s", rel_path, base, exc)
            continue
        base_node_qns.update(_node_qn(node) for node in nodes)
        base_edge_signatures.update(_edge_info_signature(edge) for edge in edges)

    return base_node_qns, base_edge_signatures


def _dedupe_edges(edges: list[GraphEdge]) -> list[GraphEdge]:
    seen: set[tuple[str, str, str, str]] = set()
    result: list[GraphEdge] = []
    for edge in edges:
        signature = _edge_signature(edge)
        if signature in seen:
            continue
        seen.add(signature)
        result.append(edge)
    return result


# ---------------------------------------------------------------------------
# 3. compute_risk_score
# ---------------------------------------------------------------------------


def compute_risk_score(
    store: GraphStore,
    node: GraphNode,
    *,
    inbound_edges: list[GraphEdge] | None = None,
    flow_criticalities: list[float] | None = None,
    flow_count: int | None = None,
    node_community_id: int | None | _UnsetType = _UNSET,
    caller_community_ids: dict[str, int | None] | None = None,
    transitive_test_count: int | None = None,
) -> float:
    """Compute a review-priority score (0.0 - 1.0) for a single node.

    The historical field name is ``risk_score``.  The value is useful for
    ranking review attention, not as a standalone changeability metric.

    Scoring factors:
      - Flow participation: 0.05 per flow membership, capped at 0.25
      - Community crossing: 0.05 per caller from a different community, capped at 0.15
      - Test coverage: 0.30 (untested) scaling down to 0.05 (5+ TESTED_BY edges)
      - Security sensitivity: 0.20 if name matches security keywords
      - Caller count: callers / 20, capped at 0.10

    Optional pre-fetched arguments let :func:`analyze_changes` issue a
    handful of batch queries up front and avoid an N+1 pattern when
    scoring many nodes.
    """
    score = 0.0

    # --- Flow participation (cap 0.25), weighted by criticality ---
    if flow_criticalities is None:
        flow_criticalities = store.get_flow_criticalities_for_node(node.id)
    if flow_criticalities:
        score += min(sum(flow_criticalities), 0.25)
    else:
        if flow_count is None:
            flow_count = store.count_flow_memberships(node.id)
        score += min(flow_count * 0.05, 0.25)

    # --- Community crossing (cap 0.15) ---
    if inbound_edges is None:
        inbound_edges = store.get_edges_by_target(node.qualified_name)
    caller_edges = [e for e in inbound_edges if e.kind == "CALLS"]

    cross_community = 0
    if isinstance(node_community_id, _UnsetType):
        node_cid = store.get_node_community_id(node.id)
    else:
        node_cid = node_community_id

    if node_cid is not None and caller_edges:
        caller_qns = [edge.source_qualified for edge in caller_edges]
        if caller_community_ids is not None:
            cid_map = {qn: caller_community_ids.get(qn) for qn in caller_qns}
        else:
            cid_map = store.get_community_ids_by_qualified_names(caller_qns)
        for cid in cid_map.values():
            if cid is not None and cid != node_cid:
                cross_community += 1
    score += min(cross_community * 0.05, 0.15)

    # --- Test coverage (direct + transitive) ---
    if transitive_test_count is None:
        transitive_test_count = len(store.get_transitive_tests(node.qualified_name))
    score += 0.30 - (min(transitive_test_count / 5.0, 1.0) * 0.25)

    # --- Security sensitivity ---
    name_lower = node.name.lower()
    qn_lower = node.qualified_name.lower()
    if any(kw in name_lower or kw in qn_lower for kw in _SECURITY_KEYWORDS):
        score += 0.20

    # --- Caller count (cap 0.10) ---
    caller_count = len(caller_edges)
    score += min(caller_count / 20.0, 0.10)

    return round(min(max(score, 0.0), 1.0), 4)


def _annotate_review_priority_semantics(result: dict[str, Any]) -> dict[str, Any]:
    """Add explicit review-priority aliases while preserving risk_score fields."""
    score = result.get("risk_score", 0.0)
    result.setdefault("review_priority_score", score)
    result.setdefault(
        "score_semantics",
        {
            "risk_score": "legacy alias for review_priority_score",
            "review_priority_score": (
                "review triage ranking that combines flows, callers, tests, "
                "security keywords, and community crossing; not a changeability score"
            ),
        },
    )
    for item in result.get("changed_functions", []) or []:
        if isinstance(item, dict) and "risk_score" in item:
            item.setdefault("review_priority_score", item["risk_score"])
    for item in result.get("review_priorities", []) or []:
        if isinstance(item, dict) and "risk_score" in item:
            item.setdefault("review_priority_score", item["risk_score"])
    return result


# ---------------------------------------------------------------------------
# 4. analyze_changes
# ---------------------------------------------------------------------------


def analyze_changes(
    store: GraphStore,
    changed_files: list[str],
    changed_ranges: dict[str, list[tuple[int, int]]] | None = None,
    repo_root: str | None = None,
    base: str = "HEAD~1",
    include_heuristic_test_gap_evidence: bool = True,
    heuristic_test_gap_node_limit: int | None = None,
) -> dict[str, Any]:
    """Analyze changes and produce risk-scored review guidance.

    Args:
        store: The graph store.
        changed_files: List of changed file paths.
        changed_ranges: Optional pre-parsed diff ranges. If not provided and
            ``repo_root`` is given, they are computed via the detected VCS
            (Git or SVN).
        repo_root: Repository root (for git/svn diff).
        base: Git ref or SVN revision range to diff against.
        include_heuristic_test_gap_evidence: Whether test gap suppression may
            use naming/source-reference heuristics. Direct ``TESTED_BY`` edges
            are always honored.
        heuristic_test_gap_node_limit: Optional cap for heuristic test gap
            checks. ``None`` preserves the historical full scan.

    Returns:
        Dict with ``summary``, ``risk_score``, ``changed_functions``,
        ``affected_flows``, ``test_gaps``, and ``review_priorities``.
    """
    # Compute changed ranges if not provided.
    if changed_ranges is None and repo_root is not None:
        changed_ranges = parse_diff_ranges(repo_root, base)

    rust_analyze = getattr(store, "analyze_changes_json", None)
    if callable(rust_analyze) and include_heuristic_test_gap_evidence:
        try:
            return _annotate_review_priority_semantics(
                json.loads(rust_analyze(changed_files, json.dumps(changed_ranges or {})))
            )
        except (RuntimeError, ValueError, TypeError, json.JSONDecodeError) as exc:
            raise RuntimeError("Rust change analysis failed") from exc

    # Map changes to nodes.
    if changed_ranges:
        changed_nodes = map_changes_to_nodes(store, changed_ranges)
        ranged_files = set(changed_ranges)
        files_without_ranges = [fp for fp in changed_files if fp not in ranged_files]
        if files_without_ranges:
            nodes_by_file = _get_nodes_for_files_boundary_aware(store, files_without_ranges)
            for fp in files_without_ranges:
                changed_nodes.extend(nodes_by_file.get(fp, []))
    else:
        # Fallback: all nodes in changed files.
        changed_nodes = []
        nodes_by_file = _get_nodes_for_files_boundary_aware(store, changed_files)
        for fp in changed_files:
            changed_nodes.extend(nodes_by_file.get(fp, []))

    # Filter to functions/tests for risk scoring (skip File nodes).
    changed_funcs = [n for n in changed_nodes if n.kind in ("Function", "Test", "Class")]

    # --- Batch prefetches shared between scoring and test-gap detection ---
    func_ids = [n.id for n in changed_funcs]
    func_qns = [n.qualified_name for n in changed_funcs]

    flow_crit_map = store.get_flow_criticalities_for_nodes(func_ids)
    nodes_needing_count = [nid for nid, vals in flow_crit_map.items() if not vals]
    flow_count_map = (
        store.count_flow_memberships_for_nodes(nodes_needing_count) if nodes_needing_count else {}
    )
    node_cid_map = store.get_community_ids_by_node_ids(func_ids)
    outbound_map, inbound_map = store.get_edges_by_endpoints(func_qns)
    relevant_edges = _dedupe_edges(
        [edge for edges in outbound_map.values() for edge in edges]
        + [edge for edges in inbound_map.values() for edge in edges]
    )
    base_node_qns, base_edge_signatures = _base_entity_sets(repo_root, base, changed_funcs)
    has_base_snapshot = repo_root is not None

    # Caller communities: collect every CALLS source seen across all nodes
    # and resolve in a single batch.
    all_caller_qns: set[str] = set()
    for edges in inbound_map.values():
        for e in edges:
            if e.kind == "CALLS":
                all_caller_qns.add(e.source_qualified)
    caller_cid_map = (
        store.get_community_ids_by_qualified_names(list(all_caller_qns)) if all_caller_qns else {}
    )

    # Compute per-node risk scores.
    node_risks: list[dict[str, Any]] = []
    for node in changed_funcs:
        risk = compute_risk_score(
            store,
            node,
            inbound_edges=inbound_map.get(node.qualified_name, []),
            flow_criticalities=flow_crit_map.get(node.id, []),
            flow_count=flow_count_map.get(node.id, 0),
            node_community_id=node_cid_map.get(node.id),
            caller_community_ids=caller_cid_map,
        )
        node_risks.append(
            {
                **node_to_dict(node),
                "risk_score": risk,
                "review_priority_score": risk,
                "change_status": (
                    "existing"
                    if node.qualified_name in base_node_qns
                    else "added"
                    if has_base_snapshot
                    else "unknown"
                ),
            }
        )

    # Overall risk score: max of individual risks, or 0.
    overall_risk = max((nr["risk_score"] for nr in node_risks), default=0.0)

    # Affected flows.
    affected = get_affected_flows(store, changed_files)

    # Detect test gaps: reuse the inbound edges already fetched above.
    test_gaps: list[dict[str, Any]] = []
    heuristic_gap_checks = 0
    heuristic_gap_eligible_count = 0
    for node in changed_funcs:
        if node.is_test:
            continue
        if node.language == "markdown":
            continue
        heuristic_gap_eligible_count += 1
        tested = outbound_map.get(node.qualified_name, [])
        has_direct_coverage = any(e.kind == "TESTED_BY" for e in tested)
        has_heuristic_coverage = False
        can_check_heuristic_gap = include_heuristic_test_gap_evidence and (
            heuristic_test_gap_node_limit is None
            or heuristic_gap_checks < heuristic_test_gap_node_limit
        )
        if can_check_heuristic_gap:
            heuristic_gap_checks += 1
            has_heuristic_coverage = has_coverage_evidence(store, node)
        if not has_direct_coverage and not has_heuristic_coverage:
            test_gaps.append(
                {
                    "name": _sanitize_name(node.name),
                    "qualified_name": _sanitize_name(node.qualified_name),
                    "file": node.file_path,
                    "kind": node.kind,
                    "language": node.language,
                    "line_start": node.line_start,
                    "line_end": node.line_end,
                    "change_status": (
                        "existing"
                        if node.qualified_name in base_node_qns
                        else "added"
                        if has_base_snapshot
                        else "unknown"
                    ),
                    "coverage_confidence": "none",
                }
            )

    changed_edges = [
        {
            **edge_to_dict(edge),
            "change_status": (
                "existing"
                if _edge_signature(edge) in base_edge_signatures
                else "added"
                if has_base_snapshot
                else "unknown"
            ),
        }
        for edge in relevant_edges
    ]
    node_status_counts = {
        "existing": sum(1 for node in node_risks if node["change_status"] == "existing"),
        "added": sum(1 for node in node_risks if node["change_status"] == "added"),
        "unknown": sum(1 for node in node_risks if node["change_status"] == "unknown"),
    }
    edge_status_counts = {
        "existing": sum(1 for edge in changed_edges if edge["change_status"] == "existing"),
        "added": sum(1 for edge in changed_edges if edge["change_status"] == "added"),
        "unknown": sum(1 for edge in changed_edges if edge["change_status"] == "unknown"),
    }

    # Review priorities: top 10 by risk score.
    review_priorities = sorted(node_risks, key=lambda x: x["risk_score"], reverse=True)[:10]

    # Build summary.
    summary_parts = [
        f"Analyzed {len(changed_files)} changed file(s):",
        f"  - {len(changed_funcs)} changed function(s)/class(es)",
        f"    - nodes: {node_status_counts['existing']} existing, "
        f"{node_status_counts['added']} added",
        f"    - edges: {edge_status_counts['existing']} existing, "
        f"{edge_status_counts['added']} added",
        f"  - {affected['total']} affected flow(s)",
        f"  - {len(test_gaps)} test gap(s)",
        f"  - Review priority score: {overall_risk:.2f}",
    ]
    if test_gaps:
        gap_names = [g["name"] for g in test_gaps[:5]]
        summary_parts.append(f"  - Untested: {', '.join(gap_names)}")

    return _annotate_review_priority_semantics(
        {
            "summary": "\n".join(summary_parts),
            "risk_score": overall_risk,
            "review_priority_score": overall_risk,
            "changed_functions": node_risks,
            "changed_edges": changed_edges,
            "change_entity_summary": {
                "nodes": node_status_counts,
                "edges": edge_status_counts,
                "base": base if has_base_snapshot else None,
            },
            "affected_flows": affected["affected_flows"],
            "test_gaps": test_gaps,
            "test_gap_evidence": {
                "direct_tested_by_edges": True,
                "heuristic_suppression_enabled": include_heuristic_test_gap_evidence,
                "heuristic_checked_node_count": heuristic_gap_checks,
                "heuristic_eligible_node_count": heuristic_gap_eligible_count,
                "heuristic_truncated": (
                    include_heuristic_test_gap_evidence
                    and heuristic_test_gap_node_limit is not None
                    and heuristic_gap_checks < heuristic_gap_eligible_count
                ),
            },
            "review_priorities": review_priorities,
        }
    )
