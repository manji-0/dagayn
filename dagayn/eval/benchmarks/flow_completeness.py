"""Flow completeness benchmark: evaluates entry point detection and flow tracing."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from dagayn.eval.scorer import IdentifierMatcher

logger = logging.getLogger(__name__)

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]


def run(repo_path: Path, store: Any, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run flow completeness benchmark."""
    from dagayn.flows import store_flows, trace_flows

    flows = trace_flows(store)
    count = store_flows(store, flows)

    # Get detected entry point names
    detected_entries = []
    for flow in flows:
        detected_entries.append(str(flow.get("entry_point") or flow.get("name", "")))

    known_entries = config.get("entry_points", [])
    matcher = IdentifierMatcher.from_config(config)
    known: list[str] = []
    aliases: dict[str, set[str]] = {}
    for item in known_entries:
        if isinstance(item, dict):
            target = str(item.get("target") or "")
            if target:
                known.append(target)
                aliases.setdefault(target, set()).update(
                    str(a) for a in item.get("aliases", []) if a
                )
        elif item:
            known.append(str(item))
    if aliases:
        matcher = IdentifierMatcher(aliases, allow_basename=matcher.allow_basename)
    found = sum(1 for ep in known if any(matcher.matches(d, ep) for d in detected_entries))
    hit_at_1 = 0
    if known and detected_entries:
        hit_at_1 = int(any(matcher.matches(detected_entries[0], ep) for ep in known))

    depths = [f.get("depth", 0) for f in flows]
    if not known:
        recall = None
        status = "skipped"
    else:
        recall = round(found / len(known), 3)
        status = "ok"

    return [
        {
            "benchmark": "flow_completeness",
            "repo": config["name"],
            "status": status,
            "known_entry_points": len(known),
            "detected_entry_points": found,
            "recall": recall,
            "hit_at_1": hit_at_1,
            "detected_flows": count,
            "avg_flow_depth": round(sum(depths) / max(len(depths), 1), 1),
            "max_flow_depth": max(depths, default=0),
        }
    ]
