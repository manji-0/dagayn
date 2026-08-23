"""Four-axis graph-construction scale benchmark.

Axes:
  size: synthetic 10k / 100k node budgets (1M is manual via config)
  operation: cold build / incremental 1-file update / query / MCP
  metric: nodes/sec, edges/sec, write/sec, peak RSS, changed-node/sec, p95
  phase: parse+write vs postprocess vs embedding (embedding is reported skipped)

This benchmark builds its own synthetic repository; the eval runner's cloned
repo is unused except as a config/name source.
"""

from __future__ import annotations

import logging
import os
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

from dagayn.eval.synthetic_repo import write_synthetic_python_repo

logger = logging.getLogger(__name__)

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]

_DEFAULT_10K = 10_000
_DEFAULT_100K = 100_000


def _init_git_repo(root: Path) -> None:
    import subprocess

    subprocess.run(["git", "init"], cwd=root, check=True, capture_output=True)
    subprocess.run(
        ["git", "config", "user.email", "scale@example.com"],
        cwd=root,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "scale"],
        cwd=root,
        check=True,
        capture_output=True,
    )
    subprocess.run(["git", "add", "-A"], cwd=root, check=True, capture_output=True)
    subprocess.run(
        ["git", "-c", "commit.gpgsign=false", "commit", "-m", "synth"],
        cwd=root,
        check=True,
        capture_output=True,
    )


def _peak_rss_mb() -> float:
    import resource

    rss = float(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
    if sys.platform == "darwin":
        return rss / (1024.0 * 1024.0)
    return rss / 1024.0


def _node_targets(config: BenchmarkPayload) -> list[int]:
    raw = config.get("scale_node_targets")
    if isinstance(raw, list) and raw:
        return [max(2, int(value)) for value in raw]
    env = os.environ.get("DAGAYN_SCALE_NODES", "").strip()
    if env:
        return [max(2, int(part)) for part in env.split(",") if part.strip()]
    targets = [_DEFAULT_10K]
    large = str(config.get("scale_include_100k") or os.environ.get("DAGAYN_SCALE_LARGE", ""))
    if large.strip().lower() in {"1", "true", "yes"}:
        targets.append(_DEFAULT_100K)
    million = str(config.get("scale_include_1m") or os.environ.get("DAGAYN_SCALE_1M", ""))
    if million.strip().lower() in {"1", "true", "yes"}:
        targets.append(1_000_000)
    return targets


def _throughput(count: int, elapsed_ms: float) -> float:
    seconds = max(elapsed_ms / 1000.0, 0.001)
    return round(count / seconds, 3)


def _measure_size(
    target_nodes: int, repo_name: str, config: BenchmarkPayload
) -> list[BenchmarkPayload]:
    from dagayn.eval.benchmarks.mcp_latency import _time_call
    from dagayn.eval.benchmarks.query_performance import run as run_query
    from dagayn.incremental import full_build, get_db_path, incremental_update
    from dagayn.tools._common import _selected_graph_store
    from dagayn.tools.build import _run_postprocess

    rows: list[BenchmarkPayload] = []
    with tempfile.TemporaryDirectory(prefix="dagayn-scale-") as tmp:
        root = Path(tmp) / "repo"
        layout = write_synthetic_python_repo(root, target_nodes=target_nodes)
        _init_git_repo(root)
        db_path = get_db_path(root)
        db_path.parent.mkdir(parents=True, exist_ok=True)
        store = _selected_graph_store()(db_path)
        try:
            rss_before = _peak_rss_mb()
            started = time.perf_counter()
            build_result = full_build(root, store)
            parse_write_ms = (time.perf_counter() - started) * 1000.0
            stats = store.get_stats()

            post_started = time.perf_counter()
            _run_postprocess(store, build_result, "full", full_rebuild=True)
            postprocess_ms = (time.perf_counter() - post_started) * 1000.0

            nodes = int(stats.total_nodes or build_result.total_nodes or 0)
            edges = int(stats.total_edges or build_result.total_edges or 0)
            rss_after = _peak_rss_mb()
            store_backend = type(store).__module__

            rows.append(
                {
                    "benchmark": "scale_performance",
                    "repo": repo_name,
                    "scenario": "cold_build",
                    "status": "ok",
                    "target_nodes": target_nodes,
                    "planned_nodes": layout["planned_nodes"],
                    "nodes": nodes,
                    "edges": edges,
                    "files_parsed": build_result.files_parsed,
                    "parse_write_ms": round(parse_write_ms, 3),
                    "postprocess_ms": round(postprocess_ms, 3),
                    "embedding_ms": 0.0,
                    "nodes_per_second": _throughput(nodes, parse_write_ms),
                    "edges_per_second": _throughput(edges, parse_write_ms),
                    "sqlite_write_per_second": _throughput(nodes + edges, parse_write_ms),
                    "peak_rss_mb": round(max(rss_before, rss_after), 3),
                    "store_backend": store_backend,
                }
            )
            rows.append(
                {
                    "benchmark": "scale_performance",
                    "repo": repo_name,
                    "scenario": "embedding",
                    "status": "skipped",
                    "target_nodes": target_nodes,
                    "note": (
                        "embedding measured by embedding_materials; "
                        "excluded from graph construction"
                    ),
                }
            )

            edit_path = root / "synth" / "m0000.py"
            original = edit_path.read_text(encoding="utf-8")
            edit_path.write_text(
                original + "\ndef extra_leaf(value: int) -> int:\n    return value\n",
                encoding="utf-8",
            )
            inc_started = time.perf_counter()
            inc_result = incremental_update(
                root,
                store,
                changed_files=["synth/m0000.py"],
            )
            _run_postprocess(
                store,
                inc_result,
                "full",
                full_rebuild=False,
                changed_files=["synth/m0000.py"],
            )
            incremental_ms = (time.perf_counter() - inc_started) * 1000.0
            changed_nodes = max(1, int(inc_result.total_nodes or 1))
            rows.append(
                {
                    "benchmark": "scale_performance",
                    "repo": repo_name,
                    "scenario": "incremental_1_file",
                    "status": "ok",
                    "target_nodes": target_nodes,
                    "incremental_ms": round(incremental_ms, 3),
                    "changed_nodes": changed_nodes,
                    "changed_node_per_second": _throughput(changed_nodes, incremental_ms),
                    "peak_rss_mb": round(_peak_rss_mb(), 3),
                }
            )

            query_rows = run_query(root, store, config)
            for row in query_rows:
                row["benchmark"] = "scale_performance"
                row["axis"] = "query"
                row["target_nodes"] = target_nodes
                row["repo"] = repo_name
            rows.extend(query_rows)

            from dagayn.eval.benchmarks.mcp_latency import _scenarios

            mcp_config = dict(config)
            mcp_config.setdefault("latency_repeat", 1)
            for name, fn in _scenarios(root, store, mcp_config).items():
                try:
                    _best, _median, p95_ms = _time_call(fn, int(mcp_config["latency_repeat"]))
                    rows.append(
                        {
                            "benchmark": "scale_performance",
                            "repo": repo_name,
                            "scenario": f"mcp_{name}",
                            "axis": "mcp",
                            "status": "ok",
                            "target_nodes": target_nodes,
                            "p95_ms": round(p95_ms, 3),
                        }
                    )
                except Exception as exc:  # noqa: BLE001
                    logger.warning("scale MCP scenario %s failed: %s", name, exc)
                    rows.append(
                        {
                            "benchmark": "scale_performance",
                            "repo": repo_name,
                            "scenario": f"mcp_{name}",
                            "axis": "mcp",
                            "status": "error",
                            "target_nodes": target_nodes,
                            "error": str(exc),
                        }
                    )
        finally:
            store.close()
    return rows


def run(repo_path: Path, store: Any, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run the synthetic scale matrix.

    Config keys:
      ``scale_node_targets``: explicit node budgets (default ``[10000]``)
      ``scale_include_100k``: also measure 100k when true
      ``scale_include_1m``: also measure 1M (manual / nightly only)
    """
    del store
    repo_name = str(config.get("name") or repo_path.name)
    rows: list[BenchmarkPayload] = []
    for target in _node_targets(config):
        try:
            rows.extend(_measure_size(target, repo_name, config))
        except Exception as exc:  # noqa: BLE001
            logger.warning("scale_performance target %s failed: %s", target, exc)
            rows.append(
                {
                    "benchmark": "scale_performance",
                    "repo": repo_name,
                    "scenario": "cold_build",
                    "status": "error",
                    "target_nodes": target,
                    "error": str(exc),
                }
            )
    return rows
