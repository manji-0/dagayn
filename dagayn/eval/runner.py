"""Evaluation runner: orchestrates benchmark execution across repositories."""

from __future__ import annotations

import csv
import importlib
import logging
from datetime import date
from pathlib import Path
from typing import Any

from dagayn.eval.git_utils import checkout_config_ref, run_git
from dagayn.eval.semantics import decorate_rows

try:
    import yaml
except ImportError:
    yaml: Any | None = None

logger = logging.getLogger(__name__)

BENCHMARK_REGISTRY = {  # nosec B105 - benchmark names, not credentials
    "token_efficiency": "dagayn.eval.benchmarks.token_efficiency",
    "impact_accuracy": "dagayn.eval.benchmarks.impact_accuracy",
    "flow_completeness": "dagayn.eval.benchmarks.flow_completeness",
    "guidance_precision": "dagayn.eval.benchmarks.guidance_precision",
    "search_quality": "dagayn.eval.benchmarks.search_quality",
    "fts_quality": "dagayn.eval.benchmarks.fts_quality",
    "build_performance": "dagayn.eval.benchmarks.build_performance",
    "doc_fuzzy_search": "dagayn.eval.benchmarks.doc_fuzzy_search",
    "embedding_text_modes": "dagayn.eval.benchmarks.embedding_text_modes",
    "embedding_materials": "dagayn.eval.benchmarks.embedding_materials",
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
        run_git(["fetch", "--all", "--tags", "--prune"], cwd=repo_path)
    else:
        run_git(["clone", config["url"], str(repo_path)])
        run_git(["fetch", "--all", "--tags"], cwd=repo_path)

    checkout_config_ref(config, repo_path)
    return repo_path


_COMMON_CSV_KEYS = [
    "benchmark",
    "repo",
    "resolved_commit",
    "commit",
    "status",
    "error",
    "query",
    "label",
]


def _csv_fieldnames(results: list[dict]) -> list[str]:
    keys = {key for row in results for key in row}
    common = [key for key in _COMMON_CSV_KEYS if key in keys]
    rest = sorted(keys - set(common))
    return common + rest


def write_csv(results: list[dict], path: Path) -> None:
    """Write benchmark results to a CSV file."""
    if not results:
        return
    results = decorate_rows(results)
    path.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = _csv_fieldnames(results)
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

        try:
            repo_path = clone_or_update(config)
        except Exception as e:
            logger.error("Checkout failed for %s: %s", name, e)
            for bench_name in benchmark_names:
                key = f"{name}_{bench_name}"
                all_results[key] = [
                    {
                        "benchmark": bench_name,
                        "repo": name,
                        "commit": config.get("commit", "HEAD"),
                        "status": "error",
                        "error": str(e),
                    }
                ]
                write_csv(all_results[key], output_dir / f"{key}_{today}.csv")
            continue

        # Build graph
        from dagayn.graph import GraphStore
        from dagayn.incremental import full_build, get_db_path

        db_path = get_db_path(repo_path)
        store = GraphStore(db_path)
        try:
            full_build(repo_path, store)

            for bench_name in benchmark_names:
                if bench_name not in BENCHMARK_REGISTRY:
                    logger.warning("Unknown benchmark: %s", bench_name)
                    continue

                logger.info("  Running %s...", bench_name)
                key = f"{name}_{bench_name}"
                try:
                    bench_fn = _load_benchmark_run(bench_name)
                    results = bench_fn(repo_path, store, config)
                    for row in results:
                        row.setdefault("benchmark", bench_name)
                        row.setdefault("repo", name)
                        row.setdefault("commit", config.get("commit", "HEAD"))
                        row.setdefault("resolved_commit", config.get("resolved_commit", ""))
                        row.setdefault("status", "ok")
                        if config.get("moving_ref"):
                            row.setdefault("moving_ref", True)
                            if config.get("moving_ref_warning"):
                                row.setdefault("warning", config["moving_ref_warning"])
                    all_results[key] = results
                    logger.info("  %s: %d result(s)", bench_name, len(results))
                except Exception as e:
                    logger.error("  %s failed: %s", bench_name, e)
                    all_results[key] = [
                        {
                            "benchmark": bench_name,
                            "repo": name,
                            "commit": config.get("commit", "HEAD"),
                            "resolved_commit": config.get("resolved_commit", ""),
                            "status": "error",
                            "error": str(e),
                        }
                    ]
                write_csv(all_results[key], output_dir / f"{key}_{today}.csv")
        except Exception as e:
            logger.error("Build failed for %s: %s", name, e)
            for bench_name in benchmark_names:
                key = f"{name}_{bench_name}"
                all_results[key] = [
                    {
                        "benchmark": bench_name,
                        "repo": name,
                        "commit": config.get("commit", "HEAD"),
                        "resolved_commit": config.get("resolved_commit", ""),
                        "status": "error",
                        "error": str(e),
                    }
                ]
                write_csv(all_results[key], output_dir / f"{key}_{today}.csv")
        finally:
            store.close()

    return all_results
