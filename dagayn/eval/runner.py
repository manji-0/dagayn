"""Evaluation runner: orchestrates benchmark execution across repositories."""

from __future__ import annotations

import csv
import importlib
import logging
import subprocess
from datetime import date
from pathlib import Path
from typing import Any

try:
    import yaml  # type: ignore[import-untyped]
except ImportError:
    yaml: Any | None = None

logger = logging.getLogger(__name__)

BENCHMARK_REGISTRY = {
    "token_efficiency": "dagayn.eval.benchmarks.token_efficiency",
    "impact_accuracy": "dagayn.eval.benchmarks.impact_accuracy",
    "flow_completeness": "dagayn.eval.benchmarks.flow_completeness",
    "guidance_precision": "dagayn.eval.benchmarks.guidance_precision",
    "search_quality": "dagayn.eval.benchmarks.search_quality",
    "build_performance": "dagayn.eval.benchmarks.build_performance",
    "doc_fuzzy_search": "dagayn.eval.benchmarks.doc_fuzzy_search",
    "embedding_text_modes": "dagayn.eval.benchmarks.embedding_text_modes",
    "nplusone_count": "dagayn.eval.benchmarks.nplusone_count",
    "mcp_latency": "dagayn.eval.benchmarks.mcp_latency",
    "recent_changes_effects": "dagayn.eval.benchmarks.recent_changes_effects",
}

CONFIGS_DIR = Path(__file__).parent / "configs"
DEFAULT_OUTPUT = Path("evaluate/results")
DEFAULT_REPOS = Path("evaluate/test_repos")


def _require_yaml() -> Any:
    if yaml is None:
        raise ImportError(
            'pyyaml is required: pip install "dagayn[eval] @ git+https://github.com/manji-0/dagayn.git"'
        )
    return yaml


def load_config(name: str) -> dict:
    """Load a single benchmark config by name."""
    yaml_mod = _require_yaml()
    path = CONFIGS_DIR / f"{name}.yaml"
    with open(path) as f:
        return yaml_mod.safe_load(f)


def load_all_configs() -> list[dict]:
    """Load all benchmark configs from the configs directory."""
    yaml_mod = _require_yaml()
    configs = []
    for p in sorted(CONFIGS_DIR.glob("*.yaml")):
        with open(p) as f:
            configs.append(yaml_mod.safe_load(f))
    return configs


def clone_or_update(config: dict, repos_dir: Path | None = None) -> Path:
    """Clone or update a repository for benchmarking."""
    repos_dir = repos_dir or DEFAULT_REPOS
    repos_dir.mkdir(parents=True, exist_ok=True)
    repo_path = repos_dir / config["name"]

    if repo_path.exists():
        subprocess.run(
            ["git", "fetch", "--all"],
            cwd=str(repo_path),
            capture_output=True,
        )
    else:
        subprocess.run(
            ["git", "clone", "--depth", "50", config["url"], str(repo_path)],
            capture_output=True,
        )

    commit = config.get("commit", "HEAD")
    if commit != "HEAD":
        subprocess.run(
            ["git", "checkout", commit],
            cwd=str(repo_path),
            capture_output=True,
        )

    return repo_path


def write_csv(results: list[dict], path: Path) -> None:
    """Write benchmark results to a CSV file."""
    if not results:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = list(results[0].keys())
    with open(path, "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(results)


def _load_benchmark_run(bench_name: str):
    module = importlib.import_module(BENCHMARK_REGISTRY[bench_name])
    return module.run


def run_eval(
    repos: list[str] | None = None,
    benchmarks: list[str] | None = None,
    output_dir: str | Path | None = None,
) -> dict[str, list[dict]]:
    """Run evaluation benchmarks across repositories.

    Args:
        repos: List of repo config names to evaluate (None = all).
        benchmarks: List of benchmark names to run (None = all).
        output_dir: Directory for CSV output files.

    Returns:
        Dict mapping ``{repo}_{benchmark}`` to list of result dicts.
    """
    output_dir = Path(output_dir) if output_dir else DEFAULT_OUTPUT
    output_dir.mkdir(parents=True, exist_ok=True)

    if repos:
        configs = [load_config(r) for r in repos]
    else:
        configs = load_all_configs()

    benchmark_names = benchmarks or list(BENCHMARK_REGISTRY.keys())
    all_results: dict[str, list[dict]] = {}
    today = date.today().isoformat()

    for config in configs:
        name = config["name"]
        logger.info("Evaluating %s...", name)

        repo_path = clone_or_update(config)

        # Build graph
        from dagayn.graph import GraphStore
        from dagayn.incremental import full_build, get_db_path

        db_path = get_db_path(repo_path)
        store = GraphStore(db_path)

        full_build(repo_path, store)

        for bench_name in benchmark_names:
            if bench_name not in BENCHMARK_REGISTRY:
                logger.warning("Unknown benchmark: %s", bench_name)
                continue

            logger.info("  Running %s...", bench_name)
            try:
                bench_fn = _load_benchmark_run(bench_name)
                results = bench_fn(repo_path, store, config)

                key = f"{name}_{bench_name}"
                all_results[key] = results
                write_csv(results, output_dir / f"{key}_{today}.csv")
                logger.info("  %s: %d result(s)", bench_name, len(results))
            except Exception as e:
                logger.error("  %s failed: %s", bench_name, e)
                all_results[f"{name}_{bench_name}"] = []

        store.close()

    return all_results
