"""PreToolUse search enrichment for Claude Code hooks.

Intercepts Grep/Glob/Bash/Read tool calls and enriches them with
structural context from the code knowledge graph: callers, callees,
execution flows, community membership, and test coverage.
"""

from __future__ import annotations

import json
import logging
import os
import re
import sys
from pathlib import Path
from typing import Any

from .graph import GraphNode
from .paths import get_db_path

logger = logging.getLogger(__name__)

type EnrichmentValue = Any
type EnrichmentPayload = dict[str, EnrichmentValue]

# Flags that consume the next token in grep/rg commands
_RG_FLAGS_WITH_VALUES = frozenset(
    {
        "-e",
        "-f",
        "-m",
        "-A",
        "-B",
        "-C",
        "-g",
        "--glob",
        "-t",
        "--type",
        "--include",
        "--exclude",
        "--max-count",
        "--max-depth",
        "--max-filesize",
        "--color",
        "--colors",
        "--context-separator",
        "--field-match-separator",
        "--path-separator",
        "--replace",
        "--sort",
        "--sortr",
    }
)


def extract_pattern(tool_name: str, tool_input: EnrichmentPayload) -> str | None:
    """Extract a search pattern from a tool call's input.

    Returns None if no meaningful pattern can be extracted.
    """
    if tool_name == "Grep":
        return tool_input.get("pattern")

    if tool_name == "Glob":
        raw = tool_input.get("pattern", "")
        # Extract meaningful name from glob: "**/auth*.ts" -> "auth"
        # Skip pure extension globs like "**/*.ts"
        match = re.search(r"[*/]([a-zA-Z][a-zA-Z0-9_]{2,})", raw)
        return match.group(1) if match else None

    if tool_name == "Bash":
        cmd = tool_input.get("command", "")
        if not re.search(r"\brg\b|\bgrep\b", cmd):
            return None
        tokens = cmd.split()
        found_cmd = False
        skip_next = False
        for token in tokens:
            if skip_next:
                skip_next = False
                continue
            if not found_cmd:
                if re.search(r"\brg$|\bgrep$", token):
                    found_cmd = True
                continue
            if token.startswith("-"):
                if token in _RG_FLAGS_WITH_VALUES:
                    skip_next = True
                continue
            cleaned = token.strip("'\"")
            return cleaned if len(cleaned) >= 3 else None
        return None

    return None


def _make_relative(file_path: str, repo_root: str) -> str:
    """Make a file path relative to repo_root for display."""
    try:
        return str(Path(file_path).relative_to(repo_root))
    except ValueError:
        return file_path


def _get_community_names_for_nodes(conn: Any, nodes: list[GraphNode]) -> dict[int, str]:
    """Fetch community names for a batch of nodes."""
    node_ids = [node.id for node in nodes]
    if not node_ids:
        return {}

    community_by_node: dict[int, int] = {}
    for i in range(0, len(node_ids), 450):
        batch = node_ids[i : i + 450]
        placeholders = ",".join("?" for _ in batch)
        rows = conn.execute(  # nosec B608
            f"SELECT id, community_id FROM nodes WHERE id IN ({placeholders})",
            batch,
        ).fetchall()
        for row in rows:
            if row["community_id"]:
                community_by_node[row["id"]] = row["community_id"]

    community_ids = list(dict.fromkeys(community_by_node.values()))
    if not community_ids:
        return {}

    name_by_id: dict[int, str] = {}
    for i in range(0, len(community_ids), 450):
        batch = community_ids[i : i + 450]
        placeholders = ",".join("?" for _ in batch)
        rows = conn.execute(  # nosec B608
            f"SELECT id, name FROM communities WHERE id IN ({placeholders})",
            batch,
        ).fetchall()
        name_by_id.update({row["id"]: row["name"] for row in rows})

    return {
        node_id: name_by_id[community_id]
        for node_id, community_id in community_by_node.items()
        if community_id in name_by_id
    }


def _get_flow_names_for_nodes(conn: Any, nodes: list[GraphNode]) -> dict[int, list[str]]:
    """Fetch up to three flow names for each node in a batch."""
    node_ids = [node.id for node in nodes]
    if not node_ids:
        return {}

    out: dict[int, list[str]] = {node_id: [] for node_id in node_ids}
    for i in range(0, len(node_ids), 450):
        batch = node_ids[i : i + 450]
        placeholders = ",".join("?" for _ in batch)
        rows = conn.execute(  # nosec B608
            "SELECT fm.node_id, f.name FROM flow_memberships fm "
            "JOIN flows f ON fm.flow_id = f.id "
            f"WHERE fm.node_id IN ({placeholders}) "
            "ORDER BY fm.node_id, f.criticality DESC",
            batch,
        ).fetchall()
        for row in rows:
            names = out.setdefault(row["node_id"], [])
            if len(names) < 3:
                names.append(row["name"])
    return out


def _prepare_context_for_nodes(nodes: list[GraphNode], store: Any, conn: Any) -> EnrichmentPayload:
    qns = [node.qualified_name for node in nodes]
    outgoing, incoming = store.get_edges_by_endpoints(qns)

    related_qns: set[str] = set()
    for node in nodes:
        qn = node.qualified_name
        for edge in incoming.get(qn, []):
            if edge.kind == "CALLS":
                related_qns.add(edge.source_qualified)
        for edge in outgoing.get(qn, []):
            if edge.kind in ("CALLS", "TESTED_BY"):
                related_qns.add(edge.target_qualified)

    related_nodes = store.get_nodes_by_qualified_names(list(related_qns)) if related_qns else {}
    return {
        "incoming": incoming,
        "outgoing": outgoing,
        "related_nodes": related_nodes,
        "community_names": _get_community_names_for_nodes(conn, nodes),
        "flow_names": _get_flow_names_for_nodes(conn, nodes),
    }


def _format_node_context(
    node: GraphNode,
    context: EnrichmentPayload,
    repo_root: str,
) -> list[str]:
    """Format a single node's structural context as plain text lines.

    Every interpolated name goes through ``_sanitize_name``. Names come from
    arbitrary source code, and this is the one path that feeds them *directly*
    into the model's context (as ``hookSpecificOutput.additionalContext``) --
    every other derived-layer module already sanitizes for exactly that reason.
    """
    from .graph import GraphNode, _sanitize_name

    assert isinstance(node, GraphNode)

    qn = node.qualified_name
    loc = _make_relative(node.file_path, repo_root)
    if node.line_start:
        loc = f"{loc}:{node.line_start}"

    header = f"{_sanitize_name(node.name)} ({loc})"

    # Community
    cname = context["community_names"].get(node.id)
    if cname:
        header += f" [{_sanitize_name(cname)}]"

    lines = [header]
    incoming = context["incoming"].get(qn, [])
    outgoing = context["outgoing"].get(qn, [])
    related_nodes = context["related_nodes"]

    # Callers (max 5, deduplicated)
    callers: list[str] = []
    seen: set[str] = set()
    for e in incoming:
        if e.kind == "CALLS" and len(callers) < 5:
            c = related_nodes.get(e.source_qualified)
            if c and c.name not in seen:
                seen.add(c.name)
                callers.append(_sanitize_name(c.name))
    if callers:
        lines.append(f"  Called by: {', '.join(callers)}")

    # Callees (max 5, deduplicated)
    callees: list[str] = []
    seen.clear()
    for e in outgoing:
        if e.kind == "CALLS" and len(callees) < 5:
            c = related_nodes.get(e.target_qualified)
            if c and c.name not in seen:
                seen.add(c.name)
                callees.append(_sanitize_name(c.name))
    if callees:
        lines.append(f"  Calls: {', '.join(callees)}")

    # Execution flows
    flow_names = context["flow_names"].get(node.id, [])
    if flow_names:
        lines.append(f"  Flows: {', '.join(_sanitize_name(f) for f in flow_names)}")

    # Tests
    tests: list[str] = []
    for e in outgoing:
        if e.kind == "TESTED_BY" and len(tests) < 3:
            t = related_nodes.get(e.target_qualified)
            if t:
                tests.append(_sanitize_name(t.name))
    if tests:
        lines.append(f"  Tests: {', '.join(tests)}")

    return lines


def enrich_search(pattern: str, repo_root: str) -> str:
    """Search the graph for pattern and return enriched context."""
    from .graph import GraphStore
    from .write_lock import graph_read_lock

    db_path = get_db_path(Path(repo_root))
    if not db_path.exists():
        return ""

    with graph_read_lock(db_path):
        store = GraphStore(db_path)
        try:
            conn = store._conn

            fts_results = store.fts_query(pattern, limit=8)
            if not fts_results.hits:
                return ""

            nodes_by_id = store.get_nodes_by_ids([node_id for node_id, _score in fts_results.hits])
            selected = []
            for node_id, _score in fts_results.hits:
                if len(selected) >= 5:
                    break
                node = nodes_by_id.get(node_id)
                if node and not node.is_test:
                    selected.append(node)

            if not selected:
                return ""

            context = _prepare_context_for_nodes(selected, store, conn)
            all_lines: list[str] = []
            for node in selected:
                node_lines = _format_node_context(node, context, repo_root)
                all_lines.extend(node_lines)
                all_lines.append("")
            count = len(selected)

            if not all_lines:
                return ""

            header = f'[dagayn] {count} symbol(s) matching "{pattern}":\n'
            return header + "\n".join(all_lines)
        finally:
            store.close()


def enrich_file_read(file_path: str, repo_root: str) -> str:
    """Enrich a file read with structural context for functions in that file."""
    from .graph import GraphStore
    from .write_lock import graph_read_lock

    db_path = get_db_path(Path(repo_root))
    if not db_path.exists():
        return ""

    with graph_read_lock(db_path):
        store = GraphStore(db_path)
        try:
            conn = store._conn
            nodes = store.get_nodes_by_file(file_path)
            if not nodes:
                # Try with resolved path
                try:
                    resolved = str(Path(file_path).resolve())
                    nodes = store.get_nodes_by_file(resolved)
                except (OSError, ValueError):
                    pass
            if not nodes:
                return ""

            # Filter to functions/classes/types (skip File nodes), limit to 10
            interesting = [n for n in nodes if n.kind in ("Function", "Class", "Type", "Test")][:10]

            if not interesting:
                return ""

            all_lines: list[str] = []
            context = _prepare_context_for_nodes(interesting, store, conn)
            for node in interesting:
                node_lines = _format_node_context(node, context, repo_root)
                all_lines.extend(node_lines)
                all_lines.append("")

            rel_path = _make_relative(file_path, repo_root)
            header = f"[dagayn] {len(interesting)} symbol(s) in {rel_path}:\n"
            return header + "\n".join(all_lines)
        finally:
            store.close()


def run_hook() -> None:
    """Entry point for the enrich CLI subcommand.

    Reads Claude Code hook JSON from stdin, extracts the search pattern,
    queries the graph, and outputs hookSpecificOutput JSON to stdout.
    """
    try:
        hook_input = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        return

    tool_name = hook_input.get("tool_name", "")
    tool_input = hook_input.get("tool_input", {})
    cwd = hook_input.get("cwd", os.getcwd())

    # Find repo root by walking up from cwd
    from .incremental import find_project_root

    repo_root = str(find_project_root(Path(cwd)))
    db_path = get_db_path(Path(repo_root))
    if not db_path.exists():
        return

    # Dispatch
    context = ""
    if tool_name == "Read":
        fp = tool_input.get("file_path", "")
        if fp:
            context = enrich_file_read(fp, repo_root)
    else:
        pattern = extract_pattern(tool_name, tool_input)
        if not pattern or len(pattern) < 3:
            return
        context = enrich_search(pattern, repo_root)

    if not context:
        return

    response = {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": context,
        }
    }
    json.dump(response, sys.stdout)
