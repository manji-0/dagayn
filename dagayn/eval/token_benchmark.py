"""Measures total tokens consumed by agent workflows against benchmark repos."""

from __future__ import annotations

import inspect
import json
import logging
from typing import Any, Callable, TypedDict

logger = logging.getLogger(__name__)


class WorkflowCallRecord(TypedDict):
    tool: str
    tokens: int


class WorkflowBenchmarkRecord(TypedDict, total=False):
    workflow: str
    total_tokens: int
    tool_calls: int
    calls: list[WorkflowCallRecord]
    error: str


def estimate_tokens(obj: Any) -> int:
    """Estimate token count from JSON-serializable object.

    Uses character count / 4 as a rough approximation for English + code.
    """
    return len(json.dumps(obj, default=str)) // 4


def benchmark_review_workflow(repo_root: str, base: str = "HEAD~1") -> WorkflowBenchmarkRecord:
    """Simulate a review workflow and measure total tokens consumed."""
    from ..tools.context import get_minimal_context
    from ..tools.review import detect_changes_func

    total_tokens = 0
    calls: list[WorkflowCallRecord] = []

    # Step 1: get_minimal_context
    result = get_minimal_context(task="review changes", repo_root=repo_root, base=base)
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "get_minimal_context", "tokens": tokens})

    # Step 2: detect_changes (minimal)
    result = detect_changes_func(base=base, repo_root=repo_root, detail_level="minimal")
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "detect_changes_minimal", "tokens": tokens})

    return {
        "workflow": "review",
        "total_tokens": total_tokens,
        "tool_calls": len(calls),
        "calls": calls,
    }


def benchmark_architecture_workflow(repo_root: str) -> WorkflowBenchmarkRecord:
    """Simulate an architecture exploration workflow."""
    from ..tools.community_tools import list_communities_func
    from ..tools.context import get_minimal_context
    from ..tools.flows_tools import list_flows

    total_tokens = 0
    calls: list[WorkflowCallRecord] = []

    result = get_minimal_context(task="map architecture", repo_root=repo_root)
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "get_minimal_context", "tokens": tokens})

    result = list_communities_func(repo_root=repo_root, detail_level="minimal")
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "list_communities_minimal", "tokens": tokens})

    result = list_flows(repo_root=repo_root, detail_level="minimal")
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "list_flows_minimal", "tokens": tokens})

    return {
        "workflow": "architecture",
        "total_tokens": total_tokens,
        "tool_calls": len(calls),
        "calls": calls,
    }


def benchmark_debug_workflow(repo_root: str) -> WorkflowBenchmarkRecord:
    """Simulate a debug workflow."""
    from ..tools.context import get_minimal_context
    from ..tools.query import semantic_search_nodes

    total_tokens = 0
    calls: list[WorkflowCallRecord] = []

    result = get_minimal_context(task="debug login bug", repo_root=repo_root)
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "get_minimal_context", "tokens": tokens})

    result = semantic_search_nodes(
        query="login",
        repo_root=repo_root,
        detail_level="minimal",
    )
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "semantic_search_minimal", "tokens": tokens})

    return {
        "workflow": "debug",
        "total_tokens": total_tokens,
        "tool_calls": len(calls),
        "calls": calls,
    }


def benchmark_onboard_workflow(repo_root: str) -> WorkflowBenchmarkRecord:
    """Simulate an onboarding workflow."""
    from ..tools.context import get_minimal_context
    from ..tools.query import list_graph_stats

    total_tokens = 0
    calls: list[WorkflowCallRecord] = []

    result = get_minimal_context(task="onboard developer", repo_root=repo_root)
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "get_minimal_context", "tokens": tokens})

    result = list_graph_stats(repo_root=repo_root)
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "list_graph_stats", "tokens": tokens})

    return {
        "workflow": "onboard",
        "total_tokens": total_tokens,
        "tool_calls": len(calls),
        "calls": calls,
    }


def benchmark_pre_merge_workflow(repo_root: str, base: str = "HEAD~1") -> WorkflowBenchmarkRecord:
    """Simulate a pre-merge check workflow."""
    from ..tools.context import get_minimal_context
    from ..tools.review import detect_changes_func

    total_tokens = 0
    calls: list[WorkflowCallRecord] = []

    result = get_minimal_context(task="pre-merge check", repo_root=repo_root, base=base)
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "get_minimal_context", "tokens": tokens})

    result = detect_changes_func(base=base, repo_root=repo_root, detail_level="minimal")
    tokens = estimate_tokens(result)
    total_tokens += tokens
    calls.append({"tool": "detect_changes_minimal", "tokens": tokens})

    return {
        "workflow": "pre_merge",
        "total_tokens": total_tokens,
        "tool_calls": len(calls),
        "calls": calls,
    }


ALL_WORKFLOWS: dict[str, Callable[..., WorkflowBenchmarkRecord]] = {
    "review": benchmark_review_workflow,
    "architecture": benchmark_architecture_workflow,
    "debug": benchmark_debug_workflow,
    "onboard": benchmark_onboard_workflow,
    "pre_merge": benchmark_pre_merge_workflow,
}


def run_all_benchmarks(repo_root: str, base: str = "HEAD~1") -> list[WorkflowBenchmarkRecord]:
    """Run all workflow benchmarks and return results."""
    results: list[WorkflowBenchmarkRecord] = []
    for name, fn in ALL_WORKFLOWS.items():
        try:
            if "base" in inspect.signature(fn).parameters:
                result = fn(repo_root=repo_root, base=base)
            else:
                result = fn(repo_root=repo_root)
            results.append(result)
        except Exception as e:
            logger.warning("Benchmark %s failed: %s", name, e)
            results.append({"workflow": name, "error": str(e)})
    return results
