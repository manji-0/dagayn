"""Impact accuracy benchmark: measures precision/recall of change impact analysis."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from dagayn.eval.git_utils import ensure_parent_available, get_commit_changed_files
from dagayn.eval.scorer import IdentifierMatcher

logger = logging.getLogger(__name__)

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]


def run(repo_path: Path, store: Any, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run impact accuracy benchmark."""
    results: list[BenchmarkPayload] = []
    matcher = IdentifierMatcher.from_config(config)
    for tc in config.get("test_commits", []):
        sha = str(tc["sha"])
        base = {
            "benchmark": "impact_accuracy",
            "repo": config["name"],
            "commit": sha,
            "resolved_commit": config.get("resolved_commit", ""),
        }
        try:
            ensure_parent_available(repo_path, sha)
            changed = get_commit_changed_files(repo_path, sha)
        except Exception as exc:
            results.append({**base, "status": "error", "error": str(exc)})
            continue
        if not changed:
            results.append({**base, "status": "skipped", "error": "no changed files"})
            continue

        # Get predicted impact from review_tool(mode="changes").
        try:
            from dagayn.tools.review_dispatcher import review_func

            analysis = review_func(
                mode="changes",
                repo_root=str(repo_path),
                base=sha + "~1",
                changed_files=changed,
                detail_level="standard",
            )
        except Exception as exc:
            logger.warning("review_tool changes failed: %s", exc)
            results.append({**base, "status": "error", "error": str(exc)})
            continue
        if analysis.get("status") != "ok":
            error = str(analysis.get("error") or analysis.get("summary") or "review failed")
            logger.warning("review_tool changes failed: %s", error)
            results.append({**base, "status": "error", "error": error})
            continue
        # Files of the changed functions and of the changed steps of the
        # affected flows.
        predicted = set(changed)
        for f in analysis.get("changed_functions", []):
            if isinstance(f, dict) and f.get("file_path"):
                predicted.add(str(f["file_path"]))
        for flow in analysis.get("affected_flows", []):
            if isinstance(flow, dict):
                for step in flow.get("changed_steps", []):
                    if isinstance(step, dict) and step.get("file"):
                        predicted.add(str(step["file"]))

        expected_files = {str(item) for item in tc.get("expected_impacted_files", [])}
        expected_symbols = {str(item) for item in tc.get("expected_impacted_symbols", [])}
        explicit_expected = expected_files | expected_symbols
        metric_prefix = ""
        status = "ok"
        if explicit_expected:
            actual = explicit_expected
        else:
            status = "proxy"
            metric_prefix = "graph_proxy_"
            actual = set(changed)
            for f in changed:
                nodes = store.get_nodes_by_file(f)
                for node in nodes:
                    for edge in store.get_edges_by_target(node.qualified_name):
                        if edge.kind in ("CALLS", "IMPORTS_FROM"):
                            src_qual = edge.source_qualified
                            src_file = src_qual.split("::")[0] if "::" in src_qual else ""
                            if src_file:
                                actual.add(src_file)

        tp = sum(
            1
            for actual_item in actual
            if any(matcher.matches(predicted_item, actual_item) for predicted_item in predicted)
        )
        precision = tp / len(predicted) if predicted else 0.0
        recall = tp / len(actual) if actual else 0.0
        f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
        score = {
            "precision": round(precision, 4),
            "recall": round(recall, 4),
            "f1": round(f1, 4),
        }

        results.append(
            {
                **base,
                "status": status,
                "predicted_files": len(predicted),
                "actual_files": len(actual),
                "true_positives": tp,
                f"{metric_prefix}precision": score["precision"],
                f"{metric_prefix}recall": score["recall"],
                f"{metric_prefix}f1": score["f1"],
            }
        )
    return results
