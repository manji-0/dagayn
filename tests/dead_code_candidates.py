"""The graph's dead-code candidates before the repository check, for testing
the graph heuristics on their own."""

from __future__ import annotations

import json
from typing import Any

from dagayn.graph import GraphStore


def graph_dead_code_candidates(
    store: GraphStore,
    kind: str | None = None,
    file_pattern: str | None = None,
) -> list[dict[str, Any]]:
    """Every node the graph alone would call dead. Never report these as dead:
    ``dead_code_report`` drops the ones the repository still uses."""
    return json.loads(store.graph_dead_code_candidates_json(kind, file_pattern))
