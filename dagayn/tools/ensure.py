"""Tool: ensure_graph — safe graph bootstrap for the default MCP surface."""

from __future__ import annotations

from typing import Any

from .session_prepare import EmbeddingPolicy, default_prepare_budget_seconds, session_prepare

_ENSURE_POSTPROCESS = "minimal"


def ensure_graph(
    repo_root: str | None = None,
    force: bool = False,
    *,
    local_embedding: str | None = "none",
    local_embedding_mode: str | None = None,
    local_embedding_port: int = 18080,
    local_embedding_bin: str = "auto",
    keep_local_embedding_server: bool = False,
    local_embedding_timeout: int = 300,
    local_embedding_request_timeout: int = 60,
    local_embedding_batch_size: int = 1,
    budget_seconds: int | None = None,
    embedding_policy: EmbeddingPolicy = "auto",
) -> dict[str, Any]:
    """Ensure a usable+synced code knowledge graph exists for analysis tools.

    Defaults for the compact MCP surface:
    - uses ``postprocess="minimal"`` (signatures + FTS only)
    - full rebuild only when the graph is empty
    - refreshes when the graph describes another commit, or holds content the
      working tree no longer has, without ``force``
    - does **not** auto-refresh merely because the worktree is dirty: edit
      hooks index those, and re-preparing on every call would re-hash the
      tree. The response says so via ``reason`` and a missingness item.
    - ``local_embedding`` defaults to ``"none"`` for direct callers; the MCP
      wrapper passes the active ``dagayn serve --local-embedding`` mode
    - ``force=True`` runs an incremental refresh when the graph already exists

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        force: If True, run an incremental refresh even when already synced.
        local_embedding: Local embedding preset (``none`` / ``bge-m3`` / ``low``…).
        budget_seconds: Optional wall-clock budget; ``None`` uses the MCP default.
        embedding_policy: ``auto`` / ``defer`` / ``skip`` / ``inline``.
    """
    if budget_seconds is None:
        budget_seconds = default_prepare_budget_seconds(mcp=True)
    return session_prepare(
        repo_root=repo_root,
        force=force,
        local_embedding=local_embedding,
        local_embedding_mode=local_embedding_mode,
        local_embedding_port=local_embedding_port,
        local_embedding_bin=local_embedding_bin,
        keep_local_embedding_server=keep_local_embedding_server,
        local_embedding_timeout=local_embedding_timeout,
        local_embedding_request_timeout=local_embedding_request_timeout,
        local_embedding_batch_size=local_embedding_batch_size,
        budget_seconds=budget_seconds,
        embedding_policy=embedding_policy,
        seed_worktree=True,
        build_if_missing=True,
    )
