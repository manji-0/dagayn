"""Entry-point reachable-set detection, tracing, and criticality scoring.

Detects entry points in the codebase (functions with no incoming CALLS edges,
framework-decorated handlers, and conventional name patterns), traces the
forward CALLS reachable set via BFS, scores each set for criticality, and
persists results to the ``flows`` / ``flow_memberships`` tables.

A stored flow is a **reachable set**, not an ordered execution path. ``path`` /
``steps`` are BFS visit order of that set. Truncation at ``max_depth`` or
``max_nodes`` is recorded on the flow.
"""

from __future__ import annotations

import json
import logging
import re
from collections import deque
from typing import Any, Optional

from .constants import SECURITY_KEYWORDS as _SECURITY_KEYWORDS
from .graph import (
    FlowAdjacency,
    GraphNode,
    GraphStore,
    _sanitize_name,
    store_write_transaction,
)

logger = logging.getLogger(__name__)

FLOW_KIND_REACHABLE_SET = "reachable_set"
DEFAULT_FLOW_MAX_DEPTH = 15
DEFAULT_FLOW_MAX_NODES = 512

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# Decorator patterns that indicate a function is a framework entry point.
_FRAMEWORK_DECORATOR_PATTERNS: list[re.Pattern[str]] = [
    # Python web frameworks
    re.compile(r"app\.(get|post|put|delete|patch|route|websocket|on_event)", re.IGNORECASE),
    re.compile(r"router\.(get|post|put|delete|patch|route)", re.IGNORECASE),
    re.compile(r"blueprint\.(route|before_request|after_request)", re.IGNORECASE),
    re.compile(r"(before|after)_(request|response)", re.IGNORECASE),
    # CLI frameworks
    re.compile(r"click\.(command|group)", re.IGNORECASE),
    re.compile(r"\w+\.(command|group)\b", re.IGNORECASE),  # Click subgroups: @mygroup.command()
    # Pydantic validators/serializers
    re.compile(r"(field|model)_(serializer|validator)", re.IGNORECASE),
    # Task queues
    re.compile(r"(celery\.)?(task|shared_task|periodic_task)", re.IGNORECASE),
    # Django
    re.compile(r"receiver", re.IGNORECASE),
    re.compile(r"api_view", re.IGNORECASE),
    re.compile(r"\baction\b", re.IGNORECASE),
    # Testing
    re.compile(r"pytest\.(fixture|mark)"),
    re.compile(r"(override_settings|modify_settings)", re.IGNORECASE),
    # SQLAlchemy / event systems
    re.compile(r"(event\.)?listens_for", re.IGNORECASE),
    # Java Spring
    re.compile(r"(Get|Post|Put|Delete|Patch|RequestMapping)Mapping", re.IGNORECASE),
    re.compile(r"(Scheduled|EventListener|Bean|Configuration)", re.IGNORECASE),
    # JS/TS frameworks
    re.compile(r"(Component|Injectable|Controller|Module|Guard|Pipe)", re.IGNORECASE),
    re.compile(r"(Subscribe|Mutation|Query|Resolver)", re.IGNORECASE),
    # Express / Koa / Hono route handlers
    re.compile(r"(app|router)\.(get|post|put|delete|patch|use|all)\b"),
    # Android lifecycle
    re.compile(r"@(Override|OnLifecycleEvent|Composable)", re.IGNORECASE),
    # Kotlin coroutines / Android ViewModel
    re.compile(r"(HiltViewModel|AndroidEntryPoint|Inject)", re.IGNORECASE),
    # AI/agent frameworks (pydantic-ai, langchain, etc.)
    re.compile(r"\w+\.(tool|tool_plain|system_prompt|result_validator)\b", re.IGNORECASE),
    re.compile(r"^tool\b"),  # bare @tool (LangChain, etc.)
    # Middleware and exception handlers (Starlette, FastAPI, Sanic)
    re.compile(r"\w+\.(middleware|exception_handler|on_exception)\b", re.IGNORECASE),
    # Generic route decorator (Flask blueprints: @bp.route, @auth_bp.route, etc.)
    re.compile(r"\w+\.route\b", re.IGNORECASE),
]

# Name patterns that indicate conventional entry points.
_ENTRY_NAME_PATTERNS: list[re.Pattern[str]] = [
    re.compile(r"^main$"),
    re.compile(r"^__main__$"),
    re.compile(r"^test_"),
    re.compile(r"^Test[A-Z]"),
    re.compile(r"^on_"),
    re.compile(r"^handle_"),
    # Lambda / serverless handler functions (wired via config, not code calls)
    re.compile(r"^handler$"),
    re.compile(r"^handle$"),
    re.compile(r"^lambda_handler$"),
    # Alembic migration entry points
    re.compile(r"^upgrade$"),
    re.compile(r"^downgrade$"),
    # FastAPI lifecycle / dependency injection
    re.compile(r"^lifespan$"),
    re.compile(r"^get_db$"),
    # Android Activity/Fragment lifecycle
    re.compile(r"^on(Create|Start|Resume|Pause|Stop|Destroy|Bind|Receive)"),
    # Servlet / JAX-RS
    re.compile(r"^do(Get|Post|Put|Delete)$"),
    # Python BaseHTTPRequestHandler
    re.compile(r"^do_(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)$"),
    re.compile(r"^log_message$"),
    # Express middleware signature
    re.compile(r"^(middleware|errorHandler)$"),
    # Angular lifecycle hooks
    re.compile(
        r"^ng(OnInit|OnChanges|OnDestroy|DoCheck"
        r"|AfterContentInit|AfterContentChecked|AfterViewInit|AfterViewChecked)$"
    ),
    # Angular Pipe / ControlValueAccessor / Guards / Resolvers
    re.compile(r"^(transform|writeValue|registerOnChange|registerOnTouched|setDisabledState)$"),
    re.compile(r"^(canActivate|canDeactivate|canActivateChild|canLoad|canMatch|resolve)$"),
    # React class component lifecycle
    re.compile(
        r"^(componentDidMount|componentDidUpdate|componentWillUnmount"
        r"|shouldComponentUpdate|render)$"
    ),
]


# ---------------------------------------------------------------------------
# Entry-point detection
# ---------------------------------------------------------------------------


def _has_framework_decorator(node: GraphNode) -> bool:
    """Return True if *node* has a decorator matching a framework pattern."""
    decorators = node.extra.get("decorators")
    if not decorators:
        return False
    if isinstance(decorators, str):
        decorators = [decorators]
    for dec in decorators:
        for pat in _FRAMEWORK_DECORATOR_PATTERNS:
            if pat.search(dec):
                return True
    return False


def _matches_entry_name(node: GraphNode) -> bool:
    """Return True if *node*'s name matches a conventional entry-point pattern."""
    for pat in _ENTRY_NAME_PATTERNS:
        if pat.search(node.name):
            return True
    return False


_TEST_FILE_RE = re.compile(
    r"([\\/]__tests__[\\/]|\.spec\.[jt]sx?$|\.test\.[jt]sx?$|[\\/]test_[^/\\]*\.py$)",
)


def _is_test_file(file_path: str) -> bool:
    """Return True if *file_path* looks like a test file."""
    return bool(_TEST_FILE_RE.search(file_path))


def detect_entry_points(
    store: GraphStore,
    include_tests: bool = False,
) -> list[GraphNode]:
    """Find functions that are entry points in the graph.

    An entry point is a Function/Test node that either:
    1. Has no incoming CALLS edges (true root), or
    2. Has a framework decorator (e.g. ``@app.get``), or
    3. Matches a conventional name pattern (``main``, ``test_*``, etc.).

    When *include_tests* is False (the default), Test nodes are excluded so
    that flow analysis focuses on production entry points.
    """
    # Build a set of all qualified names that are CALLS targets. Exclude
    # edges sourced at File nodes so that script-/notebook-/top-level-only
    # callees (e.g. ``run_job()`` invoked from module scope, a top-level
    # ``<App />`` render) remain detectable as entry points.
    called_qnames = store.get_all_call_targets(include_file_sources=False)

    # Scan all nodes for entry-point candidates.
    candidate_nodes = store.get_nodes_by_kind(["Function", "Test"])

    entry_points: list[GraphNode] = []
    seen_qn: set[str] = set()

    for node in candidate_nodes:
        if not include_tests and (node.is_test or _is_test_file(node.file_path)):
            continue

        is_entry = False

        # True root: no one calls this function.
        if node.qualified_name not in called_qnames:
            is_entry = True

        # Framework decorator match.
        if _has_framework_decorator(node):
            is_entry = True

        # Conventional name match.
        if _matches_entry_name(node):
            is_entry = True

        if is_entry and node.qualified_name not in seen_qn:
            entry_points.append(node)
            seen_qn.add(node.qualified_name)

    return entry_points


# ---------------------------------------------------------------------------
# Flow tracing (BFS)
# ---------------------------------------------------------------------------


def _trace_single_flow(
    adj: FlowAdjacency,
    ep: GraphNode,
    max_depth: int = DEFAULT_FLOW_MAX_DEPTH,
    max_nodes: int = DEFAULT_FLOW_MAX_NODES,
) -> Optional[dict]:
    """Trace the CALLS reachable set from *ep* via forward BFS.

    Returns a flow dict (see :func:`trace_flows` for the schema) or ``None``
    if the set is trivial (single-node, no outgoing CALLS that resolve).
    ``path`` is BFS visit order of the reachable set, not a call sequence.
    """
    path_ids: list[int] = [ep.id]
    path_qnames: list[str] = [ep.qualified_name]
    visited: set[str] = {ep.qualified_name}
    queue: deque[tuple[str, int]] = deque([(ep.qualified_name, 0)])

    actual_depth = 0
    truncated = False
    truncation_reason: str | None = None
    nodes_by_qn = adj.nodes_by_qn
    calls_out = adj.calls_out

    while queue:
        current_qn, depth = queue.popleft()
        if depth > actual_depth:
            actual_depth = depth
        if depth >= max_depth:
            if calls_out.get(current_qn):
                truncated = True
                if truncation_reason is None:
                    truncation_reason = "max_depth"
            continue

        for target_qn in calls_out.get(current_qn, ()):
            if target_qn in visited:
                continue
            target_node = nodes_by_qn.get(target_qn)
            if target_node is None:
                continue
            if len(path_ids) >= max_nodes:
                truncated = True
                truncation_reason = "max_nodes"
                queue.clear()
                break
            visited.add(target_qn)
            path_ids.append(target_node.id)
            path_qnames.append(target_qn)
            queue.append((target_qn, depth + 1))

    # Skip trivial single-node flows.
    if len(path_ids) < 2:
        return None

    files = list({n.file_path for qn in path_qnames if (n := nodes_by_qn.get(qn)) is not None})

    flow: dict = {
        "name": _sanitize_name(ep.name),
        "entry_point": ep.qualified_name,
        "entry_point_id": ep.id,
        "kind": FLOW_KIND_REACHABLE_SET,
        "path": path_ids,
        "members": list(path_ids),
        "depth": actual_depth,
        "node_count": len(path_ids),
        "file_count": len(files),
        "files": files,
        "truncated": truncated,
        "truncation_reason": truncation_reason,
        "criticality": 0.0,
    }
    flow["criticality"] = compute_criticality(flow, adj)
    return flow


def trace_flows(
    store: GraphStore,
    max_depth: int = DEFAULT_FLOW_MAX_DEPTH,
    include_tests: bool = False,
    max_nodes: int = DEFAULT_FLOW_MAX_NODES,
) -> list[dict]:
    """Trace reachable sets from every entry point via forward BFS.

    Returns a list of flow dicts, each containing:
      - name: human-readable flow name (entry point name)
      - entry_point: qualified name of the entry point
      - entry_point_id: node database id of the entry point
      - kind: ``reachable_set`` (not an ordered execution path)
      - path / members: BFS visit order of reachable node IDs
      - depth: maximum BFS depth reached
      - node_count: number of distinct nodes in the set
      - file_count: number of distinct files touched
      - files: list of distinct file paths
      - truncated / truncation_reason: ``max_depth`` or ``max_nodes`` when capped
      - criticality: computed criticality score (0.0-1.0)
    """
    entry_points = detect_entry_points(store, include_tests=include_tests)
    if not entry_points:
        return []

    adj = store.load_flow_adjacency()
    flows: list[dict] = []

    for ep in entry_points:
        flow = _trace_single_flow(adj, ep, max_depth, max_nodes)
        if flow is not None:
            flows.append(flow)

    # Sort by criticality descending.
    flows.sort(key=lambda f: f["criticality"], reverse=True)
    return flows


# ---------------------------------------------------------------------------
# Criticality scoring
# ---------------------------------------------------------------------------


def compute_criticality(flow: dict, adj: FlowAdjacency) -> float:
    """Score a flow from 0.0 to 1.0 based on multiple weighted factors.

    Weights:
      - File spread:         0.30
      - External calls:      0.20
      - Security sensitivity: 0.25
      - Test coverage gap:   0.15
      - Depth:               0.10
    """
    node_ids: list[int] = flow.get("path", [])
    if not node_ids:
        return 0.0

    nodes_by_id = adj.nodes_by_id
    nodes_by_qn = adj.nodes_by_qn
    calls_out = adj.calls_out
    has_tested_by = adj.has_tested_by

    nodes: list[GraphNode] = [n for nid in node_ids if (n := nodes_by_id.get(nid)) is not None]
    if not nodes:
        return 0.0

    # --- File spread (0.0 - 1.0) ---
    file_count = len({n.file_path for n in nodes})
    # Normalize: 1 file => 0.0, 5+ files => 1.0
    file_spread = min((file_count - 1) / 4.0, 1.0) if file_count > 1 else 0.0

    # --- External calls (0.0 - 1.0) ---
    # Calls that target nodes NOT in the graph are considered external.
    external_count = 0
    for n in nodes:
        for target_qn in calls_out.get(n.qualified_name, ()):
            if target_qn not in nodes_by_qn:
                external_count += 1
    # Normalize: 0 => 0.0, 5+ => 1.0
    external_score = min(external_count / 5.0, 1.0)

    # --- Security sensitivity (0.0 - 1.0) ---
    security_hits = 0
    for n in nodes:
        name_lower = n.name.lower()
        qn_lower = n.qualified_name.lower()
        for kw in _SECURITY_KEYWORDS:
            if kw in name_lower or kw in qn_lower:
                security_hits += 1
                break  # Count each node at most once.
    security_score = min(security_hits / max(len(nodes), 1), 1.0)

    # --- Test coverage gap (0.0 - 1.0) ---
    tested_count = sum(1 for n in nodes if n.qualified_name in has_tested_by)
    coverage = tested_count / max(len(nodes), 1)
    test_gap = 1.0 - coverage

    # --- Depth (0.0 - 1.0) ---
    depth = flow.get("depth", 0)
    # Normalize: 0 => 0.0, 10+ => 1.0
    depth_score = min(depth / 10.0, 1.0)

    # --- Weighted sum ---
    criticality = (
        file_spread * 0.30
        + external_score * 0.20
        + security_score * 0.25
        + test_gap * 0.15
        + depth_score * 0.10
    )
    return round(min(max(criticality, 0.0), 1.0), 4)


def refresh_flow_criticality(store: GraphStore) -> int:
    """Recompute stored flow criticality from current TESTED_BY / CALLS facts.

    Incremental tracing only retraces flows whose path files changed. Adding a
    test file that covers a flow member does not touch those files, so stored
    criticality would otherwise stay stale. Recomputing scores is cheap relative
    to tracing. See: #114
    """
    adj = store.load_flow_adjacency()
    conn = getattr(store, "_conn", None)
    rust_get = getattr(store, "get_flows_json", None)
    rust_update = getattr(store, "update_flow_criticalities_json", None)

    flows: list[dict[str, Any]]
    if conn is not None:
        rows = conn.execute("SELECT id, depth, path_json, criticality FROM flows").fetchall()
        flows = [
            {
                "id": int(row["id"]),
                "depth": row["depth"],
                "path": json.loads(row["path_json"]),
                "criticality": float(row["criticality"] or 0.0),
            }
            for row in rows
        ]
    elif callable(rust_get):
        flows = json.loads(rust_get("criticality", 1_000_000))
    else:
        return 0

    updates: list[tuple[int, float]] = []
    for flow in flows:
        path = flow.get("path") or []
        recomputed = compute_criticality({"path": path, "depth": flow.get("depth", 0)}, adj)
        previous = float(flow.get("criticality") or 0.0)
        if abs(recomputed - previous) > 1e-9:
            flow_id = flow.get("id")
            if flow_id is None:
                continue
            updates.append((int(flow_id), recomputed))

    if not updates:
        return 0

    if callable(rust_update):
        payload = json.dumps([[flow_id, score] for flow_id, score in updates])
        return int(rust_update(payload))

    if conn is None:
        logger.warning("Cannot refresh flow criticality: store has no SQL connection")
        return 0

    with store_write_transaction(store):
        conn.executemany(
            "UPDATE flows SET criticality = ? WHERE id = ?",
            [(score, flow_id) for flow_id, score in updates],
        )
    return len(updates)


# ---------------------------------------------------------------------------
# Persistence
# ---------------------------------------------------------------------------

_FLOW_INSERT_SQL = """INSERT INTO flows
               (name, entry_point_id, depth, node_count, file_count,
                criticality, path_json, kind, truncated, truncation_reason)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"""


def _flow_insert_params(flow: dict) -> tuple:
    """Values for :data:`_FLOW_INSERT_SQL`."""
    return (
        flow["name"],
        flow["entry_point_id"],
        flow["depth"],
        flow["node_count"],
        flow["file_count"],
        flow["criticality"],
        json.dumps(flow.get("path", [])),
        flow.get("kind") or FLOW_KIND_REACHABLE_SET,
        1 if flow.get("truncated") else 0,
        flow.get("truncation_reason"),
    )


def _flow_disclosure_fields(row: Any, path_ids: list[int]) -> dict:
    """Kind / truncation fields stored beside the membership path."""
    kind = FLOW_KIND_REACHABLE_SET
    truncated = False
    truncation_reason = None
    try:
        raw_kind = row["kind"]
        if raw_kind:
            kind = str(raw_kind)
        truncated = bool(row["truncated"])
        reason = row["truncation_reason"]
        if reason:
            truncation_reason = str(reason)
    except (KeyError, IndexError):
        pass
    return {
        "kind": kind,
        "truncated": truncated,
        "truncation_reason": truncation_reason,
        "members": list(path_ids),
    }


def store_flows(store: GraphStore, flows: list[dict]) -> int:
    """Clear existing flows and persist new ones.

    Returns the number of flows stored.
    """
    rust_store = getattr(store, "store_flows_json", None)
    if callable(rust_store):
        return int(rust_store(json.dumps(flows)))

    # NOTE: store_flows uses _conn directly because it performs
    # multi-statement batch writes (DELETE + INSERT loop) that are
    # tightly coupled to the DB transaction lifecycle.
    conn = store._conn

    # Wrap the full DELETE + INSERT sequence in an explicit transaction
    # so partial writes cannot occur if an exception interrupts the loop.
    with store_write_transaction(store):
        conn.execute("DELETE FROM flow_memberships")
        conn.execute("DELETE FROM flows")

        # Batch-insert all flows in one executemany call
        conn.executemany(
            _FLOW_INSERT_SQL,
            [_flow_insert_params(f) for f in flows],
        )
        count = len(flows)

        # Fetch newly-inserted IDs keyed by entry_point_id (unique per flow)
        ep_to_flow_id: dict[int, int] = {}
        for row in conn.execute("SELECT id, entry_point_id FROM flows").fetchall():
            ep_to_flow_id[row["entry_point_id"]] = row["id"]

        # Build all membership rows and insert in one executemany
        all_memberships: list[tuple[int, int, int]] = [
            (ep_to_flow_id[f["entry_point_id"]], node_id, position)
            for f in flows
            if f["entry_point_id"] in ep_to_flow_id
            for position, node_id in enumerate(f.get("path", []))
        ]
        if all_memberships:
            conn.executemany(
                "INSERT OR IGNORE INTO flow_memberships (flow_id, node_id, position) "
                "VALUES (?, ?, ?)",
                all_memberships,
            )

    return count


def _qualified_name_file(qualified_name: str) -> str:
    if "::" in qualified_name:
        return qualified_name.rsplit("::", 1)[0]
    return qualified_name


def _entry_reaches_changed_files(
    adj: FlowAdjacency,
    entry_qn: str,
    changed_files: set[str],
    changed_qnames: set[str],
    max_depth: int = 15,
) -> bool:
    """Return True when a forward trace from *entry_qn* can reach *changed_files*."""
    if entry_qn in changed_qnames or _qualified_name_file(entry_qn) in changed_files:
        return True

    visited: set[str] = set()
    queue: deque[tuple[str, int]] = deque([(entry_qn, 0)])
    calls_out = adj.calls_out
    while queue:
        qn, depth = queue.popleft()
        if qn in visited:
            continue
        visited.add(qn)
        if qn in changed_qnames or _qualified_name_file(qn) in changed_files:
            return True
        if depth >= max_depth:
            continue
        for target in calls_out.get(qn, ()):
            queue.append((target, depth + 1))
    return False


def _resolve_flow_entry_qn(
    store: GraphStore,
    flow_id: int,
    entry_point_id: int,
    path_ids: list[int],
) -> str | None:
    """Best-effort entry qualified name when ``entry_point_id`` may be stale."""
    nodes = store.get_nodes_by_ids([entry_point_id])
    if entry_point_id in nodes:
        return nodes[entry_point_id].qualified_name

    row = store._conn.execute(
        "SELECT n.qualified_name FROM flow_memberships fm "
        "JOIN nodes n ON n.id = fm.node_id "
        "WHERE fm.flow_id = ? ORDER BY fm.position LIMIT 1",
        (flow_id,),
    ).fetchone()
    if row is not None:
        return row["qualified_name"]

    for node_id in path_ids:
        node = store.get_nodes_by_ids([node_id]).get(node_id)
        if node is not None:
            return node.qualified_name
    return None


def _normalize_changed_file_keys(store: GraphStore, changed_files: list[str]) -> list[str]:
    keys: set[str] = set()
    for file_path in changed_files:
        keys.add(file_path)
        normalized = store._normalize_file_path_key(file_path)
        if normalized != file_path:
            keys.add(normalized)
    return list(keys)


def _get_affected_flow_ids(store: GraphStore, changed_files: list[str]) -> list[int]:
    """Locate flows that touch *changed_files* even after node ids were replaced."""
    if not changed_files:
        return []

    lookup_files = _normalize_changed_file_keys(store, changed_files)
    changed_file_set = set(lookup_files)
    conn = store._conn
    affected: set[int] = set()
    placeholders = ",".join("?" * len(lookup_files))

    for row in conn.execute(  # nosec B608
        f"SELECT DISTINCT fm.flow_id FROM flow_memberships fm "
        f"JOIN nodes n ON n.id = fm.node_id "
        f"WHERE n.file_path IN ({placeholders})",
        lookup_files,
    ):
        affected.add(int(row[0]))

    for row in conn.execute(  # nosec B608
        f"SELECT f.id FROM flows f "
        f"JOIN nodes n ON n.id = f.entry_point_id "
        f"WHERE n.file_path IN ({placeholders})",
        lookup_files,
    ):
        affected.add(int(row[0]))

    for row in conn.execute(  # nosec B608
        f"SELECT DISTINCT f.id FROM flows f, json_each(f.path_json) AS je "
        f"JOIN nodes n ON n.id = CAST(je.value AS INTEGER) "
        f"WHERE n.file_path IN ({placeholders})",
        lookup_files,
    ):
        affected.add(int(row[0]))

    for row in conn.execute(
        "SELECT f.id, f.name FROM flows f "
        "LEFT JOIN nodes n ON n.id = f.entry_point_id "
        "WHERE n.id IS NULL"
    ):
        flow_id = int(row[0])
        flow_name = row[1]
        match = conn.execute(  # nosec B608
            f"SELECT 1 FROM nodes WHERE file_path IN ({placeholders}) AND name = ? LIMIT 1",
            (*lookup_files, flow_name),
        ).fetchone()
        if match is not None:
            affected.add(flow_id)

    changed_qnames = {
        row["qualified_name"]
        for row in conn.execute(  # nosec B608
            f"SELECT qualified_name FROM nodes WHERE file_path IN ({placeholders})",
            lookup_files,
        )
    }

    stale_rows = conn.execute(
        "SELECT DISTINCT f.id, f.entry_point_id, f.path_json "
        "FROM flows f "
        "WHERE EXISTS ("
        "  SELECT 1 FROM json_each(f.path_json) AS je "
        "  LEFT JOIN nodes n ON n.id = CAST(je.value AS INTEGER) "
        "  WHERE n.id IS NULL"
        ")"
    ).fetchall()

    adj: FlowAdjacency | None = None
    for row in stale_rows:
        flow_id = int(row["id"])
        if flow_id in affected:
            continue
        path_ids = json.loads(row["path_json"])
        entry_qn = _resolve_flow_entry_qn(store, flow_id, int(row["entry_point_id"]), path_ids)
        if entry_qn is None:
            affected.add(flow_id)
            continue
        if adj is None:
            adj = store.load_flow_adjacency()
        if _entry_reaches_changed_files(adj, entry_qn, changed_file_set, changed_qnames):
            affected.add(flow_id)

    return sorted(affected)


def _delete_flows_by_ids(store: GraphStore, flow_ids: list[int]) -> set[int]:
    """Delete flows (and snapshots/memberships) and return their entry-point ids."""
    if not flow_ids:
        return set()

    conn = store._conn
    entry_point_ids: set[int] = set()
    ep_placeholders = ",".join("?" * len(flow_ids))
    for row in conn.execute(  # nosec B608
        f"SELECT entry_point_id FROM flows WHERE id IN ({ep_placeholders})",
        flow_ids,
    ):
        entry_point_ids.add(int(row[0]))

    with store_write_transaction(store):
        _batch_size = 450
        for i in range(0, len(flow_ids), _batch_size):
            chunk = flow_ids[i : i + _batch_size]
            placeholders = ",".join("?" * len(chunk))
            conn.execute(  # nosec B608
                f"DELETE FROM flow_snapshots WHERE flow_id IN ({placeholders})",
                chunk,
            )
            conn.execute(  # nosec B608
                f"DELETE FROM flow_memberships WHERE flow_id IN ({placeholders})",
                chunk,
            )
            conn.execute(  # nosec B608
                f"DELETE FROM flows WHERE id IN ({placeholders})",
                chunk,
            )
    return entry_point_ids


def _delete_flows_for_entry_qualified_names(
    store: GraphStore,
    entry_qualified_names: set[str],
) -> None:
    """Remove stale flows that share an entry-point qualified name."""
    if not entry_qualified_names:
        return

    conn = store._conn
    flow_ids: set[int] = set()
    for qn in entry_qualified_names:
        for row in conn.execute(
            "SELECT f.id FROM flows f "
            "JOIN nodes n ON n.id = f.entry_point_id "
            "WHERE n.qualified_name = ?",
            (qn,),
        ):
            flow_ids.add(int(row[0]))
    if flow_ids:
        _delete_flows_by_ids(store, sorted(flow_ids))


def incremental_trace_flows(
    store: GraphStore,
    changed_files: list[str],
    max_depth: int = 15,
) -> int:
    """Re-trace only flows that touch *changed_files*.  Much faster than full trace.

    1. Find affected flows by qualified name, live memberships, dangling
       memberships, or stale ``path_json`` node ids.
    2. Collect the entry-point node IDs of those flows before deleting them.
    3. Delete only the affected flows and their memberships.
    4. Re-detect entry points, keeping those in *changed_files* **or** whose
       node ID was an entry point of a deleted flow.
    5. BFS-trace each relevant entry point via :func:`_trace_single_flow`.
    6. INSERT the new flows (without clearing unrelated flows), first deleting
       any stale flow that shares the same entry-point qualified name.
    7. Recompute criticality for every remaining flow so TESTED_BY coverage
       and unresolved-external facts stay current even when the changed files
       are tests that are not on any flow path.

    Returns the number of re-traced flows that were stored.
    """
    if not changed_files:
        return 0

    rust_delete = getattr(store, "delete_affected_flows", None)
    rust_insert = getattr(store, "insert_flows_json", None)
    if callable(rust_delete) and callable(rust_insert):
        changed_file_set = set(changed_files)
        entry_point_ids = {int(node_id) for node_id in rust_delete(changed_files)}

        entry_points = detect_entry_points(store)
        relevant_eps = [
            ep
            for ep in entry_points
            if ep.file_path in changed_file_set or ep.id in entry_point_ids
        ]

        new_flows: list[dict] = []
        if relevant_eps:
            adj = store.load_flow_adjacency()
            for ep in relevant_eps:
                flow = _trace_single_flow(adj, ep, max_depth)
                if flow is not None:
                    new_flows.append(flow)

        count = 0
        if new_flows:
            count = int(rust_insert(json.dumps(new_flows)))
        refresh_flow_criticality(store)
        return count

    changed_file_set = set(changed_files)

    # ------------------------------------------------------------------
    # 1-3. Find and delete affected flows
    # ------------------------------------------------------------------
    affected_ids = _get_affected_flow_ids(store, changed_files)
    entry_point_ids = _delete_flows_by_ids(store, affected_ids)

    # ------------------------------------------------------------------
    # 4. Re-detect entry points and filter to relevant ones
    # ------------------------------------------------------------------
    entry_points = detect_entry_points(store)
    relevant_eps = [
        ep for ep in entry_points if ep.file_path in changed_file_set or ep.id in entry_point_ids
    ]

    # ------------------------------------------------------------------
    # 5. BFS-trace each relevant entry point
    # ------------------------------------------------------------------
    new_flows: list[dict] = []
    if relevant_eps:
        adj = store.load_flow_adjacency()
        for ep in relevant_eps:
            flow = _trace_single_flow(adj, ep, max_depth)
            if flow is not None:
                new_flows.append(flow)

    # ------------------------------------------------------------------
    # 6. INSERT new flows without clearing unrelated ones
    # ------------------------------------------------------------------
    count = len(new_flows)
    if new_flows:
        _delete_flows_for_entry_qualified_names(
            store,
            {flow["entry_point"] for flow in new_flows if flow.get("entry_point")},
        )

        conn = store._conn
        conn.executemany(
            _FLOW_INSERT_SQL,
            [_flow_insert_params(f) for f in new_flows],
        )

        # Map freshly-inserted flows back to IDs via entry_point_id (unique per flow)
        known_ep_ids = {f["entry_point_id"] for f in new_flows}
        ep_ph = ",".join("?" * len(known_ep_ids))
        ep_rows = conn.execute(  # nosec B608
            f"SELECT id, entry_point_id FROM flows WHERE entry_point_id IN ({ep_ph})",
            list(known_ep_ids),
        ).fetchall()
        ep_to_flow_id = {r["entry_point_id"]: r["id"] for r in ep_rows}

        memberships: list[tuple[int, int, int]] = [
            (ep_to_flow_id[f["entry_point_id"]], node_id, position)
            for f in new_flows
            if f["entry_point_id"] in ep_to_flow_id
            for position, node_id in enumerate(f.get("path", []))
        ]
        if memberships:
            conn.executemany(
                "INSERT OR IGNORE INTO flow_memberships (flow_id, node_id, position) "
                "VALUES (?, ?, ?)",
                memberships,
            )

    conn = store._conn
    conn.commit()
    refresh_flow_criticality(store)
    return count


# ---------------------------------------------------------------------------
# Query helpers
# ---------------------------------------------------------------------------


def get_flows(
    store: GraphStore,
    sort_by: str = "criticality",
    limit: int = 50,
) -> list[dict]:
    """Retrieve stored flows from the database.

    Args:
        store: The graph store.
        sort_by: Column to sort by (``criticality``, ``depth``, ``node_count``).
        limit: Maximum number of flows to return.
    """
    allowed_sort = {"criticality", "depth", "node_count", "file_count", "name"}
    if sort_by not in allowed_sort:
        sort_by = "criticality"

    rust_get = getattr(store, "get_flows_json", None)
    if callable(rust_get):
        return _annotate_flow_rows_liveness(store, json.loads(rust_get(sort_by, limit)))

    order = "DESC" if sort_by in ("criticality", "depth", "node_count", "file_count") else "ASC"

    # NOTE: get_flows reads from the flows table which is managed by
    # the flows module; _conn access is documented coupling.
    rows = store._conn.execute(
        f"SELECT * FROM flows ORDER BY {sort_by} {order} LIMIT ?",  # nosec B608
        (limit,),
    ).fetchall()

    results: list[dict] = []
    for row in rows:
        results.append(
            {
                "id": row["id"],
                "name": _sanitize_name(row["name"]),
                "entry_point_id": row["entry_point_id"],
                "depth": row["depth"],
                "node_count": row["node_count"],
                "file_count": row["file_count"],
                "criticality": row["criticality"],
                "path": json.loads(row["path_json"]),
                "created_at": row["created_at"],
                "updated_at": row["updated_at"],
            }
        )
        results[-1].update(_flow_disclosure_fields(row, results[-1]["path"]))
    return _annotate_flow_rows_liveness(store, results)


def _annotate_flow_rows_liveness(store: GraphStore, flows: list[dict]) -> list[dict]:
    """Add resolved/missing node counts to listed flows.

    ``node_count``/``file_count`` are the values recorded when the flow was
    traced. A flow whose nodes have since been deleted kept reporting them, so
    the list API used by the wiki, the visualization payload and flow listings
    presented stale counts as current -- ``_hydrate_flow_rows`` annotates this,
    but nothing on this path did.
    """
    all_ids = {
        node_id
        for flow in flows
        for node_id in (flow.get("path") or [])
        if isinstance(node_id, int)
    }
    if not all_ids:
        return flows
    try:
        live_ids = set(store.get_nodes_by_ids(sorted(all_ids)).keys())
    except Exception:  # noqa: BLE001 — annotation must never break a listing
        logger.debug("Could not resolve flow node liveness", exc_info=True)
        return flows
    for flow in flows:
        path_ids = [node_id for node_id in (flow.get("path") or []) if isinstance(node_id, int)]
        resolved = sum(1 for node_id in path_ids if node_id in live_ids)
        flow["resolved_node_count"] = resolved
        flow["missing_node_count"] = len(path_ids) - resolved
    return flows


def _collect_cross_artifact_edges_among(
    store: GraphStore,
    path_qns: set[str],
) -> list[Any]:
    """Fetch CROSS_ARTIFACT edges whose endpoints are both in ``path_qns``."""
    if not path_qns:
        return []
    try:
        get_among = getattr(store, "get_edges_among", None)
        if callable(get_among):
            return [
                edge
                for edge in get_among(path_qns)
                if getattr(edge, "kind", None) == "CROSS_ARTIFACT"
            ]
        outgoing, incoming = store.get_edges_by_endpoints(list(path_qns))
        bridge_edges: list[Any] = []
        seen: set[int] = set()
        for edge_list in (*outgoing.values(), *incoming.values()):
            for edge in edge_list:
                edge_id = getattr(edge, "id", None)
                if edge_id in seen:
                    continue
                if edge_id is not None:
                    seen.add(edge_id)
                if getattr(edge, "kind", None) == "CROSS_ARTIFACT":
                    src = str(getattr(edge, "source_qualified", "") or "")
                    tgt = str(getattr(edge, "target_qualified", "") or "")
                    if src in path_qns and tgt in path_qns:
                        bridge_edges.append(edge)
        return bridge_edges
    except Exception:  # pragma: no cover - backend parity drift
        return []


def _annotate_flow_step_resolution(flow: dict) -> dict:
    """Add resolved/missing step counts for stored flow paths."""
    path_ids = flow.get("path") or []
    steps = flow.get("steps") or []
    resolved_step_count = len(steps)
    if path_ids:
        missing_step_count = len(path_ids) - resolved_step_count
    else:
        stored_node_count = int(flow.get("node_count") or resolved_step_count)
        missing_step_count = max(0, stored_node_count - resolved_step_count)
    annotated = dict(flow)
    annotated["resolved_step_count"] = resolved_step_count
    annotated["missing_step_count"] = missing_step_count
    return annotated


def _annotate_flow_dict_bridges(store: GraphStore, flow: dict) -> dict:
    """Mark bridge arrivals on a flow dict (shared by Rust and Python paths)."""
    from .cross_artifact import annotate_flow_steps_with_bridges

    steps = list(flow.get("steps") or [])
    path_qns = {
        str(step.get("qualified_name"))
        for step in steps
        if isinstance(step.get("qualified_name"), str)
    }
    bridge_edges = _collect_cross_artifact_edges_among(store, path_qns)
    annotated = dict(flow)
    annotated["steps"] = annotate_flow_steps_with_bridges(steps, bridge_edges)
    annotated["bridge_step_count"] = sum(
        1 for step in annotated["steps"] if step.get("is_bridge_step")
    )
    return _annotate_flow_step_resolution(annotated)


def get_flow_by_id(store: GraphStore, flow_id: int) -> Optional[dict]:
    """Retrieve a single flow with reachable-set membership details.

    Returns a dict with the flow metadata plus a ``steps`` list containing
    each member node's name, kind, file, and line info in BFS visit order.
    That list is not a call sequence. Bridge arrivals among members are
    marked with ``step_kind="bridge"``.
    """
    rust_get = getattr(store, "get_flow_by_id_json", None)
    if callable(rust_get):
        raw = rust_get(flow_id)
        if not raw:
            return None
        return _annotate_flow_dict_bridges(store, json.loads(raw))

    # NOTE: get_flow_by_id reads from the flows table; see store_flows note.
    row = store._conn.execute("SELECT * FROM flows WHERE id = ?", (flow_id,)).fetchone()
    if row is None:
        return None
    return _hydrate_flow_rows(store, [row])[0]


def _hydrate_flow_rows(
    store: GraphStore,
    rows: list[Any],
) -> list[dict]:
    """Build full flow dicts (with ``steps``) for a list of flow rows.

    Issues two batched queries total instead of one per flow + one per
    step: a single ``WHERE id IN (...)`` over all node ids referenced by
    any flow's path, then a per-flow Python join.

    Bridge steps are marked distinctly when a reportable ``CROSS_ARTIFACT``
    edge connects two nodes in the same flow path.
    """
    from .cross_artifact import annotate_flow_steps_with_bridges

    if not rows:
        return []

    paths_by_flow: dict[int, list[int]] = {}
    all_node_ids: list[int] = []
    for row in rows:
        path_ids: list[int] = json.loads(row["path_json"])
        paths_by_flow[row["id"]] = path_ids
        all_node_ids.extend(path_ids)

    nodes_by_id = store.get_nodes_by_ids(all_node_ids)

    out: list[dict] = []
    for row in rows:
        path_ids = paths_by_flow[row["id"]]
        steps: list[dict] = []
        path_qns: list[str] = []
        missing_step_count = 0
        for nid in path_ids:
            node = nodes_by_id.get(nid)
            if node is None:
                missing_step_count += 1
                continue
            path_qns.append(node.qualified_name)
            steps.append(
                {
                    "node_id": node.id,
                    "name": _sanitize_name(node.name),
                    "kind": node.kind,
                    "file": node.file_path,
                    "line_start": node.line_start,
                    "line_end": node.line_end,
                    "qualified_name": _sanitize_name(node.qualified_name),
                }
            )
        resolved_step_count = len(steps)

        bridge_edges: list[Any] = []
        if path_qns:
            try:
                bridge_edges = [
                    edge
                    for edge in store.get_edges_among(set(path_qns))
                    if getattr(edge, "kind", None) == "CROSS_ARTIFACT"
                ]
            except Exception:  # pragma: no cover - backend parity drift
                bridge_edges = []
        steps = annotate_flow_steps_with_bridges(steps, bridge_edges)
        bridge_step_count = sum(1 for step in steps if step.get("is_bridge_step"))

        payload = {
            "id": row["id"],
            "name": _sanitize_name(row["name"]),
            "entry_point_id": row["entry_point_id"],
            "depth": row["depth"],
            "node_count": row["node_count"],
            "file_count": row["file_count"],
            "criticality": row["criticality"],
            "path": path_ids,
            "steps": steps,
            "resolved_step_count": resolved_step_count,
            "missing_step_count": missing_step_count,
            "bridge_step_count": bridge_step_count,
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }
        payload.update(_flow_disclosure_fields(row, path_ids))
        out.append(payload)
    return out


def get_affected_flows(
    store: GraphStore,
    changed_files: list[str],
) -> dict:
    """Find flows that include nodes from the given changed files.

    Returns::

        {
            "affected_flows": [<flow dicts>],
            "total": <int>,
        }
    """
    if not changed_files:
        return {"affected_flows": [], "total": 0}

    rust_get = getattr(store, "get_affected_flows_json", None)
    if callable(rust_get):
        affected = [
            _annotate_flow_dict_bridges(store, flow) for flow in json.loads(rust_get(changed_files))
        ]
        return {"affected_flows": affected, "total": len(affected)}

    # Find flow IDs that touch changed files (including stale path_json).
    flow_ids = _get_affected_flow_ids(store, changed_files)

    if not flow_ids:
        return {"affected_flows": [], "total": 0}

    # Batch-fetch all matching flow rows in one query (chunked to stay
    # within SQLite's IN(...) variable limit).
    rows: list[Any] = []
    batch_size = 450
    for i in range(0, len(flow_ids), batch_size):
        batch = flow_ids[i : i + batch_size]
        placeholders = ",".join("?" for _ in batch)
        rows.extend(
            store._conn.execute(  # nosec B608
                f"SELECT * FROM flows WHERE id IN ({placeholders})",
                batch,
            ).fetchall()
        )

    affected = _hydrate_flow_rows(store, rows)

    # Sort by criticality descending.
    affected.sort(key=lambda f: f.get("criticality", 0), reverse=True)

    return {
        "affected_flows": affected,
        "total": len(affected),
    }
