"""Wiki generation from community structure.

Generates markdown pages for each detected community and an index page,
providing a navigable documentation wiki for the codebase architecture.
"""

from __future__ import annotations

import logging
import re
import sqlite3
from collections import Counter
from pathlib import Path
from typing import Any

from ._scope import build_node_scope_maps
from .architecture import compute_sdp_metrics, find_adp_violations, find_sdp_violations
from .communities import get_communities
from .flows import get_flows
from .graph import GraphStore, _sanitize_name
from .sap import compute_sap_metrics

logger = logging.getLogger(__name__)


def _slugify(name: str) -> str:
    """Convert a community name to a safe filename slug."""
    slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
    return slug[:80] or "unnamed"


def _build_architecture_metrics_context(store: GraphStore) -> dict[str, Any]:
    """Precompute package-level architecture metrics for wiki rendering."""
    qualified_to_scope, _name_to_scope = build_node_scope_maps(store, "package")

    try:
        sdp_metrics = compute_sdp_metrics(store, granularity="package")
        sdp_violations = find_sdp_violations(store, granularity="package")
        adp_violations = find_adp_violations(store, granularity="package")
        sap_metrics = compute_sap_metrics(store, scope_kind="package")
    except (sqlite3.Error, OSError, RuntimeError, ValueError, TypeError) as exc:
        logger.warning("wiki: architecture metrics unavailable: %s", exc)
        return {
            "available": False,
            "error": str(exc),
            "qualified_to_scope": qualified_to_scope,
        }

    return {
        "available": True,
        "qualified_to_scope": qualified_to_scope,
        "sdp_metrics_by_scope": {m["name"]: m for m in sdp_metrics},
        "sdp_violations": sdp_violations,
        "adp_violations": adp_violations,
        "sap_metrics_by_scope": {m["scope_key"]: m for m in sap_metrics},
    }


def _format_metric(value: Any) -> str:
    if isinstance(value, float):
        return f"{value:.2f}"
    return str(value)


def _sap_notes(metric: dict[str, Any]) -> str:
    notes = list(metric.get("notes", []))
    abstractness = metric.get("abstractness", 0.0)
    instability = metric.get("instability", 0.0)
    distance = metric.get("distance", 0.0)

    if distance >= 0.5 and abstractness <= 0.2 and instability <= 0.2:
        notes.append("zone-of-pain")
    elif distance >= 0.5 and abstractness >= 0.8 and instability >= 0.8:
        notes.append("zone-of-uselessness")

    return ", ".join(dict.fromkeys(notes)) or "-"


def _community_package_scopes(
    community: dict[str, Any],
    metrics_context: dict[str, Any],
) -> list[str]:
    qualified_to_scope = metrics_context.get("qualified_to_scope", {})
    member_qns = community.get("members", [])
    scopes = {qualified_to_scope[qn] for qn in member_qns if qn in qualified_to_scope}
    return sorted(scopes)


def _render_architecture_metrics_section(
    community: dict[str, Any],
    metrics_context: dict[str, Any],
) -> list[str]:
    lines: list[str] = ["## Architecture Metrics", ""]

    if not metrics_context.get("available", False):
        error = metrics_context.get("error", "unknown error")
        lines.append(f"Architecture metrics unavailable: {error}")
        lines.append("")
        return lines

    scopes = _community_package_scopes(community, metrics_context)
    if not scopes:
        lines.append("No package scopes detected for this community.")
        lines.append("")
        return lines

    scope_set = set(scopes)
    lines.append(
        "Package-level ADP/SDP/SAP results filtered to scopes represented by this community."
    )
    lines.append("")
    lines.append(f"- **Package scopes covered**: {len(scopes)}")

    sdp_metrics = [
        metrics_context["sdp_metrics_by_scope"][scope]
        for scope in scopes
        if scope in metrics_context["sdp_metrics_by_scope"]
    ]
    if sdp_metrics:
        avg_i = sum(m["instability"] for m in sdp_metrics) / len(sdp_metrics)
        most_unstable = max(sdp_metrics, key=lambda m: m["instability"])
        lines.append(f"- **Average instability**: {avg_i:.2f}")
        lines.append(
            "- **Most unstable scope**: "
            f"`{_sanitize_name(most_unstable['name'])}` "
            f"(I={most_unstable['instability']:.2f}, "
            f"Ca={most_unstable['ca']}, Ce={most_unstable['ce']})"
        )
    else:
        lines.append("- **Average instability**: n/a")
    lines.append("")

    lines.append("### Stable Dependencies")
    lines.append("")
    if sdp_metrics:
        lines.append("| Scope | Ca | Ce | I |")
        lines.append("|-------|---:|---:|---:|")
        for metric in sorted(sdp_metrics, key=lambda m: m["instability"], reverse=True)[:10]:
            lines.append(
                f"| `{_sanitize_name(metric['name'])}` | {metric['ca']} | {metric['ce']} "
                f"| {_format_metric(metric['instability'])} |"
            )
    else:
        lines.append("No SDP metrics available for covered scopes.")
    lines.append("")

    sdp_violations = [
        v
        for v in metrics_context["sdp_violations"]
        if v.get("source") in scope_set or v.get("target") in scope_set
    ]
    if sdp_violations:
        lines.append("#### SDP Violations")
        lines.append("")
        lines.append("| Source | Target | Delta |")
        lines.append("|--------|--------|------:|")
        for violation in sdp_violations[:10]:
            lines.append(
                f"| `{_sanitize_name(violation['source'])}` | "
                f"`{_sanitize_name(violation['target'])}` | "
                f"{_format_metric(violation['delta'])} |"
            )
        lines.append("")

    lines.append("### Stable Abstractions")
    lines.append("")
    sap_metrics = [
        metrics_context["sap_metrics_by_scope"][scope]
        for scope in scopes
        if scope in metrics_context["sap_metrics_by_scope"]
    ]
    if sap_metrics:
        lines.append("| Scope | A | I | D | Na/Nt | Notes |")
        lines.append("|-------|---:|---:|---:|------:|-------|")
        for metric in sorted(sap_metrics, key=lambda m: m["distance"], reverse=True)[:10]:
            lines.append(
                f"| `{_sanitize_name(metric['scope_key'])}` "
                f"| {_format_metric(metric['abstractness'])} "
                f"| {_format_metric(metric['instability'])} "
                f"| {_format_metric(metric['distance'])} "
                f"| {metric['na']}/{metric['nt']} "
                f"| {_sap_notes(metric)} |"
            )
    else:
        lines.append("No SAP metrics available for covered scopes.")
    lines.append("")

    lines.append("### Acyclic Dependencies")
    lines.append("")
    adp_violations = [
        v for v in metrics_context["adp_violations"] if set(v.get("nodes", [])) & scope_set
    ]
    if adp_violations:
        lines.append(f"{len(adp_violations)} package-level cycle(s) touch this community.")
        lines.append("")
        lines.append("| Cycle | Length | Severity |")
        lines.append("|-------|-------:|---------:|")
        for violation in adp_violations[:10]:
            cycle = " -> ".join(f"`{_sanitize_name(node)}`" for node in violation["nodes"])
            lines.append(f"| {cycle} | {violation['length']} | {violation['severity']} |")
    else:
        lines.append("No package-level dependency cycles touch this community.")
    lines.append("")

    return lines


def _generate_community_page(
    store: GraphStore,
    community: dict[str, Any],
    metrics_context: dict[str, Any] | None = None,
) -> str:
    """Build markdown content for a single community.

    Includes: heading, overview (size, cohesion, language), members table
    (top 50), execution flows through the community, and dependencies.

    Args:
        store: The graph store.
        community: Community dict from get_communities().

    Returns:
        Markdown string for the community page.
    """
    name = community["name"]
    size = community["size"]
    cohesion = community.get("cohesion", 0.0)
    lang = community.get("dominant_language", "")
    description = community.get("description", "")

    lines: list[str] = []
    lines.append(f"# {name}")
    lines.append("")

    # Overview section
    lines.append("## Overview")
    lines.append("")
    if description:
        lines.append(f"{description}")
        lines.append("")
    lines.append(f"- **Size**: {size} nodes")
    lines.append(f"- **Cohesion**: {cohesion:.4f}")
    if lang:
        lines.append(f"- **Dominant Language**: {lang}")
    lines.append("")

    if metrics_context is None:
        metrics_context = _build_architecture_metrics_context(store)
    lines.extend(_render_architecture_metrics_section(community, metrics_context))

    # Members table (top 50)
    member_qns = community.get("members", [])
    lines.append("## Members")
    lines.append("")
    if member_qns:
        lines.append("| Name | Kind | File | Lines |")
        lines.append("|------|------|------|-------|")

        # Fetch node details for members (limit to 50)
        member_nodes = store.get_nodes_by_qualified_names(list(member_qns[:50]))
        member_count = 0
        for qn in member_qns[:50]:
            node = member_nodes.get(qn)
            if node and node.kind != "File":
                node_name = _sanitize_name(node.name)
                lines.append(
                    f"| {node_name} | {node.kind} | {node.file_path} "
                    f"| {node.line_start}-{node.line_end} |"
                )
                member_count += 1

        if not member_count:
            # Remove the table headers if no members were added
            lines.pop()  # header separator
            lines.pop()  # header
            lines.append("No non-file members found.")

        if len(member_qns) > 50:
            lines.append("")
            lines.append(f"*... and {len(member_qns) - 50} more members.*")
    else:
        lines.append("No members found.")
    lines.append("")

    # Execution flows through community
    lines.append("## Execution Flows")
    lines.append("")
    member_set = set(member_qns)
    try:
        all_flows = get_flows(store, sort_by="criticality", limit=200)
        flow_qns_by_id = store.get_flow_qualified_names_for_flows(
            [int(flow["id"]) for flow in all_flows]
        )
        community_flows: list[dict] = []
        for flow in all_flows:
            # Check if this flow passes through any community member
            flow_qns = flow_qns_by_id.get(int(flow["id"]), set())
            if flow_qns & member_set:
                community_flows.append(flow)

        if community_flows:
            for flow in community_flows[:10]:
                flow_name = _sanitize_name(flow.get("name", "unnamed"))
                criticality = flow.get("criticality", 0.0)
                depth = flow.get("depth", 0)
                lines.append(f"- **{flow_name}** (criticality: {criticality:.2f}, depth: {depth})")
            if len(community_flows) > 10:
                lines.append(f"- *... and {len(community_flows) - 10} more flows.*")
        else:
            lines.append("No execution flows pass through this community.")
    except sqlite3.OperationalError as exc:
        logger.debug("wiki: flows table unavailable: %s", exc)
        lines.append("Execution flow data not available.")
    lines.append("")

    # Dependencies (cross-community edges)
    lines.append("## Dependencies")
    lines.append("")
    try:
        outgoing_targets: Counter[str] = Counter()
        incoming_sources: Counter[str] = Counter()
        if member_qns:
            qns = list(member_qns)

            # Outgoing: source is a member
            for t in store.get_outgoing_targets(qns):
                if t not in member_set:
                    outgoing_targets[t] += 1

            # Incoming: target is a member
            for s in store.get_incoming_sources(qns):
                if s not in member_set:
                    incoming_sources[s] += 1

        if outgoing_targets:
            lines.append("### Outgoing")
            lines.append("")
            for target, count in outgoing_targets.most_common(15):
                lines.append(f"- `{_sanitize_name(target)}` ({count} edge(s))")
            lines.append("")

        if incoming_sources:
            lines.append("### Incoming")
            lines.append("")
            for source, count in incoming_sources.most_common(15):
                lines.append(f"- `{_sanitize_name(source)}` ({count} edge(s))")
            lines.append("")

        if not outgoing_targets and not incoming_sources:
            lines.append("No cross-community dependencies detected.")
            lines.append("")
    except sqlite3.OperationalError as exc:
        logger.debug("wiki: dependency edges unavailable: %s", exc)
        lines.append("Dependency data not available.")
        lines.append("")

    return "\n".join(lines)


def generate_wiki(
    store: GraphStore,
    wiki_dir: str | Path,
    force: bool = False,
) -> dict[str, Any]:
    """Generate a markdown wiki from the community structure.

    For each community, generates a markdown page. Also generates an
    index.md with links to all community pages.

    Args:
        store: The graph store.
        wiki_dir: Directory to write wiki pages into.
        force: If True, regenerate all pages even if content unchanged.

    Returns:
        Dict with pages_generated, pages_updated, pages_unchanged counts.
    """
    wiki_path = Path(wiki_dir)
    wiki_path.mkdir(parents=True, exist_ok=True)

    communities = get_communities(store)

    pages_generated = 0
    pages_updated = 0
    pages_unchanged = 0

    page_entries: list[tuple[str, str, int]] = []  # (slug, name, size)
    metrics_context = _build_architecture_metrics_context(store)

    # Track slugs we've already used in THIS run so two communities that
    # slugify to the same filename don't overwrite each other (#222 follow-up).
    # Previously "Data Processing" and "data processing" both became
    # "data-processing.md", causing silent data loss and inflated "updated"
    # counters (each collision was counted as an update while only one file
    # made it to disk).
    used_slugs: set[str] = set()

    for comm in communities:
        name = comm["name"]
        base_slug = _slugify(name)
        slug = base_slug
        suffix = 2
        while slug in used_slugs:
            slug = f"{base_slug}-{suffix}"
            suffix += 1
        used_slugs.add(slug)

        filename = f"{slug}.md"
        filepath = wiki_path / filename

        content = _generate_community_page(store, comm, metrics_context=metrics_context)

        if filepath.exists() and not force:
            existing = filepath.read_text(encoding="utf-8", errors="replace")
            if existing == content:
                pages_unchanged += 1
                page_entries.append((slug, name, comm["size"]))
                continue

        already_existed = filepath.exists()
        filepath.write_text(content, encoding="utf-8")
        if already_existed:
            pages_updated += 1
        else:
            pages_generated += 1
        page_entries.append((slug, name, comm["size"]))

    # Generate index.md
    index_lines: list[str] = []
    index_lines.append("# Code Wiki")
    index_lines.append("")
    index_lines.append(
        "Auto-generated documentation from the code knowledge graph community structure."
    )
    index_lines.append("")
    index_lines.append(f"**Total communities**: {len(communities)}")
    index_lines.append("")
    index_lines.append("## Communities")
    index_lines.append("")
    index_lines.append("| Community | Size | Link |")
    index_lines.append("|-----------|------|------|")
    for slug, name, size in sorted(page_entries, key=lambda x: x[1]):
        index_lines.append(f"| {name} | {size} | [{slug}.md]({slug}.md) |")
    index_lines.append("")

    index_content = "\n".join(index_lines)
    index_path = wiki_path / "index.md"

    if index_path.exists() and not force:
        existing_index = index_path.read_text(encoding="utf-8", errors="replace")
        if existing_index == index_content:
            pages_unchanged += 1
        else:
            index_path.write_text(index_content, encoding="utf-8")
            pages_updated += 1
    else:
        index_path.write_text(index_content, encoding="utf-8")
        pages_generated += 1

    # Delete pages no longer backed by a community. Nothing removed them before,
    # so every re-detect left a fresh generation of orphans (community sub-names
    # are not stable), the directory grew without bound, and `get_wiki_page`
    # happily served a page documenting deleted code.
    written = {index_path.name, *(f"{slug}.md" for slug in used_slugs)}
    pages_removed: list[str] = []
    for existing in wiki_path.glob("*.md"):
        if existing.name in written:
            continue
        try:
            existing.unlink()
        except OSError:
            logger.warning("Could not remove orphaned wiki page %s", existing)
            continue
        pages_removed.append(existing.name)
    if pages_removed:
        logger.info("Removed %d orphaned wiki page(s)", len(pages_removed))

    return {
        "pages_generated": pages_generated,
        "pages_updated": pages_updated,
        "pages_unchanged": pages_unchanged,
        "pages_removed": pages_removed,
    }


def get_wiki_page(wiki_dir: str | Path, page_name: str) -> str | None:
    """Retrieve a specific wiki page by community name.

    Args:
        wiki_dir: Directory containing wiki pages.
        page_name: Community name (will be slugified for filename lookup).

    Returns:
        Page content as a string, or None if the page does not exist.
    """
    wiki_path = Path(wiki_dir)
    slug = _slugify(page_name)
    filepath = wiki_path / f"{slug}.md"

    if filepath.is_file():
        return filepath.read_text(encoding="utf-8", errors="replace")

    # Fallback: try exact filename match — with path traversal protection
    exact_path = (wiki_path / page_name).resolve()
    if exact_path.is_file() and exact_path.is_relative_to(wiki_path.resolve()):
        return exact_path.read_text(encoding="utf-8", errors="replace")

    # No substring fallback: ``slug in p.stem`` matched unrelated pages (a query
    # for "auth" returning "auth-legacy-sub3"), which is worse than a miss --
    # the caller cannot tell it got a different community's page.
    return None
