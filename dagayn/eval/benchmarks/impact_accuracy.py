"""Impact accuracy benchmark: measures precision/recall of change impact analysis."""

from __future__ import annotations

import logging
from pathlib import Path

from dagayn.eval.git_utils import ensure_parent_available, run_git
from dagayn.eval.scorer import IdentifierMatcher

logger = logging.getLogger(__name__)


def _get_changed_files(repo_path: Path, sha: str) -> list[str]:
    """Get list of changed files for a commit."""
    result = run_git(["diff", "--name-only", f"{sha}~1", sha], cwd=repo_path)
    return [f.strip() for f in result.stdout.strip().splitlines() if f.strip()]


def run(repo_path: Path, store, config: dict) -> list[dict]:
    """Run impact accuracy benchmark."""
    results = []
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
            changed = _get_changed_files(repo_path, sha)
        except Exception as exc:
            results.append({**base, "status": "error", "error": str(exc)})
            continue
        if not changed:
            results.append({**base, "status": "skipped", "error": "no changed files"})
            continue

        # Get predicted impact from our tool
        try:
            from dagayn.changes import analyze_changes

            analysis = analyze_changes(
                store,
                changed,
                repo_root=str(repo_path),
                base=tc["sha"] + "~1",
            )
            # Extract files from changed_functions and affected_flows
            predicted = set(changed)
            for f in analysis.changed_functions:
                if isinstance(f, dict) and "file_path" in f:
                    predicted.add(f["file_path"])
                elif isinstance(f, dict) and "file" in f:
                    predicted.add(f["file"])
            for flow in analysis.affected_flows:
                if isinstance(flow, dict):
                    for node in flow.get("nodes", []):
                        if isinstance(node, dict) and "file_path" in node:
                            predicted.add(node["file_path"])
        except Exception as exc:
            logger.warning("analyze_changes failed: %s", exc)
            results.append({**base, "status": "error", "error": str(exc)})
            continue

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
