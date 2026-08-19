"""Tools 7, 8, 19, 20: embed_graph, get_docs_section, wiki tools."""

from __future__ import annotations

import logging
from pathlib import Path

from ..embeddings import (
    EmbeddingStore,
    embed_all_nodes,
    finalize_embedding_run,
    prepare_all_nodes,
)
from ..embeddings_store import EmbedWorkItem
from ..incremental import get_db_path
from ._common import (
    ToolPayload,
    _error_response,
    _get_store,
    _validate_repo_root,
    handle_tool_runtime_error,
)

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Tool 7: embed_graph
# ---------------------------------------------------------------------------


def _provider_unavailable_error(provider: str | None) -> str:
    """Explain which embedding provider is missing and how to configure it."""
    if provider == "local":
        return (
            "provider='local' sentence-transformers embeddings were removed. "
            "Use dagayn build/update/serve --local-embedding for the managed "
            "llama-server sidecar, or provider='openai' with a localhost "
            "OpenAI-compatible endpoint."
        )
    if provider in ("openai", "google", "minimax"):
        return (
            f"The '{provider}' embedding provider is not available. "
            "Check the required environment variables "
            "(see README and `get_provider()` docstring) and that "
            "the endpoint is reachable."
        )
    return (
        "No embedding provider is configured. Use "
        "dagayn build/update/serve --local-embedding for the managed "
        "llama-server sidecar, or configure provider='openai', "
        "'google', or 'minimax'."
    )


def scan_embed_work(
    repo_root: str | None = None,
    model: str | None = None,
    provider: str | None = None,
    *,
    prune_orphans: bool = True,
) -> tuple[ToolPayload, list[EmbedWorkItem]]:
    """Scan for embedding work, then close the graph store.

    The returned items can be written by :func:`write_embed_work` without any
    graph store open, so a caller that wants several bounded lock windows pays
    this scan once per pass instead of once per window. On a 42k-node graph the
    scan is ~8 s while a window embeds for 4 s, so re-scanning per window was two
    thirds of the run.
    """
    store, root = _get_store(repo_root, cached=False)
    db_path = get_db_path(root)
    emb_store = EmbeddingStore(db_path, provider=provider, model=model, source_root=root)
    try:
        if not emb_store.available:
            return {"status": "error", "error": _provider_unavailable_error(provider)}, []
        work = prepare_all_nodes(store, emb_store, prune_orphans=prune_orphans)
        return (
            {
                "status": "ok",
                "orphans_removed": emb_store.last_orphans_removed,
                "pending": len(work),
                "text_mode": emb_store.text_mode,
            },
            work,
        )
    finally:
        emb_store.close()
        store.close()


def write_embed_work(
    work: list[EmbedWorkItem],
    repo_root: str | None = None,
    model: str | None = None,
    provider: str | None = None,
    *,
    show_progress: bool = False,
    slice_seconds: float | None = None,
    finalize: bool = False,
) -> ToolPayload:
    """Write embeddings for *work*, opening no graph store.

    ``finalize=True`` closes the run out (provider pointer, retired-partition
    sweep) and should be set on the window that drains the last item.
    """
    root = _validate_repo_root(Path(repo_root)) if repo_root else None
    db_path = get_db_path(root) if root is not None else get_db_path(Path.cwd())
    emb_store = EmbeddingStore(db_path, provider=provider, model=model, source_root=root)
    try:
        if not emb_store.available:
            return {"status": "error", "error": _provider_unavailable_error(provider)}
        embedded = emb_store.embed_prepared(
            work,
            show_progress=show_progress,
            slice_seconds=slice_seconds,
        )
        remaining = emb_store.last_remaining
        if finalize and remaining <= 0:
            finalize_embedding_run(emb_store, prune_orphans=True)
        if embedded:
            emb_store.checkpoint_writes(truncate=True)
        return {
            "status": "ok",
            "newly_embedded": embedded,
            "remaining": remaining,
            "total_embeddings": emb_store.count(),
            "text_mode": emb_store.text_mode,
        }
    finally:
        emb_store.close()


def embed_graph(
    repo_root: str | None = None,
    model: str | None = None,
    provider: str | None = None,
    *,
    show_progress: bool = False,
    slice_seconds: float | None = None,
    prune_orphans: bool = True,
) -> ToolPayload:
    """Compute vector embeddings for all graph nodes to enable semantic search.

    Local embeddings use dagayn's managed llama-server sidecar
    (``--local-embedding``), which exposes an OpenAI-compatible localhost
    endpoint. Cloud providers like ``openai`` / ``google`` / ``minimax`` use
    stdlib ``urllib``.
    Override the model via ``model`` param or CRG_OPENAI_MODEL env var.
    Changing the model or provider re-embeds all nodes automatically.

    Only embeds nodes that don't already have up-to-date embeddings.

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        model: Embedding model name. For openai: model ID (e.g.
               ``text-embedding-3-small``); for google: Gemini model ID. Falls
               back to CRG_OPENAI_MODEL where appropriate.
        provider: Provider name: ``openai``, ``google``, or ``minimax``.
                  Omit when the MCP server was started with
                  ``--local-embedding``. ``openai`` requires CRG_OPENAI_BASE_URL +
                  CRG_OPENAI_API_KEY + CRG_OPENAI_MODEL env vars and accepts
                  any OpenAI-compatible endpoint (real OpenAI, Azure, new-api,
                  LiteLLM, vLLM, LocalAI, Ollama openai-mode, etc.).
        slice_seconds: Stop after the first provider batch past this many
                  seconds and report the rest in ``remaining``. Callers use
                  this to bound how long they hold the graph lock; ``None``
                  embeds everything in one pass.
        prune_orphans: Sweep orphaned embeddings and retired provider
                  partitions. Set False on follow-up slices of one run.

    Returns:
        Number of nodes embedded, total embedding count, and how many nodes
        are still unembedded (``remaining``, non-zero only with
        ``slice_seconds``).
    """
    store, root = _get_store(repo_root, cached=False)
    db_path = get_db_path(root)
    emb_store = EmbeddingStore(db_path, provider=provider, model=model, source_root=root)
    store_closed = False
    try:
        if not emb_store.available:
            return {"status": "error", "error": _provider_unavailable_error(provider)}

        newly_embedded = embed_all_nodes(
            store,
            emb_store,
            show_progress=show_progress,
            slice_seconds=slice_seconds,
            prune_orphans=prune_orphans,
        )
        orphans_removed = emb_store.last_orphans_removed
        remaining = emb_store.last_remaining
        if newly_embedded or orphans_removed:
            store.close()
            store_closed = True
            emb_store.checkpoint_writes(truncate=True)
        total = emb_store.count()

        return {
            "status": "ok",
            "summary": (
                f"Embedded {newly_embedded} new node(s). "
                f"Removed {orphans_removed} orphan embedding(s). "
                f"Total embeddings: {total}. "
                + (
                    f"{remaining} node(s) still queued for a later slice."
                    if remaining
                    else "Semantic search is now active."
                )
            ),
            "newly_embedded": newly_embedded,
            "orphans_removed": orphans_removed,
            "total_embeddings": total,
            "remaining": remaining,
            "text_mode": emb_store.text_mode,
        }
    finally:
        emb_store.close()
        if not store_closed:
            store.close()


# ---------------------------------------------------------------------------
# Tool 8: get_docs_section
# ---------------------------------------------------------------------------


def get_docs_section(
    section_name: str,
    repo_root: str | None = None,
    max_chars: int = 4000,
) -> ToolPayload:
    """Return a specific section from the LLM-optimized reference.

    Used by skills and Claude Code to load only the exact documentation
    section needed, keeping token usage minimal (90%+ savings).

    Args:
        section_name: Exact section name. One of: usage, review-delta,
                      review-pr, commands, legal, watch, embeddings,
                      languages, troubleshooting.
        repo_root: Repository root path. Auto-detected from current
                   directory if omitted.
        max_chars: Maximum characters to return. Default: 4000.
            When truncated, content ends with "... (truncated)".

    Returns:
        The section content, or an error if not found.
    """
    import re as _re

    search_roots: list[Path] = []

    if repo_root:
        # Validate before reading: an unvalidated caller-supplied root turns
        # this into a read from any directory on the filesystem.
        try:
            search_roots.append(_validate_repo_root(Path(repo_root)))
        except ValueError:
            return _error_response(
                f"repo_root does not look like a project root: {repo_root}",
                section=section_name,
            )

    store = None
    try:
        store, root = _get_store(repo_root)
        if root not in search_roots:
            search_roots.append(root)
    except (RuntimeError, ValueError):
        pass
    finally:
        if store is not None:
            store.close()

    # Fallback: package directory (for uvx/pip installs)
    pkg_docs = Path(__file__).parent.parent.parent / "docs" / "LLM-OPTIMIZED-REFERENCE.md"
    if pkg_docs.exists():
        pkg_root = pkg_docs.parent.parent
        if pkg_root not in search_roots:
            search_roots.append(pkg_root)

    for search_root in search_roots:
        candidate = search_root / "docs" / "LLM-OPTIMIZED-REFERENCE.md"
        if candidate.exists():
            content = candidate.read_text(encoding="utf-8", errors="replace")
            match = _re.search(
                rf'<section name="{_re.escape(section_name)}">'
                r"(.*?)</section>",
                content,
                _re.DOTALL | _re.IGNORECASE,
            )
            if match:
                content = match.group(1).strip()
                truncated = len(content) > max_chars
                if truncated:
                    content = content[:max_chars] + "\n... (truncated)"
                return {
                    "status": "ok",
                    "section": section_name,
                    "content": content,
                    "truncated": truncated,
                }

    available = [
        "usage",
        "review-delta",
        "review-pr",
        "commands",
        "legal",
        "watch",
        "embeddings",
        "languages",
        "troubleshooting",
    ]
    return {
        "status": "not_found",
        "error": (f"Section '{section_name}' not found. Available: {', '.join(available)}"),
    }


# ---------------------------------------------------------------------------
# Tool 19: generate_wiki  [DOCS]
# ---------------------------------------------------------------------------


def generate_wiki_func(
    repo_root: str | None = None,
    force: bool = False,
) -> ToolPayload:
    """Generate a markdown wiki from the community structure.

    [DOCS] Creates a wiki page for each detected community and an index
    page. Pages are written to ``.dagayn/wiki/`` inside the
    repository. Only regenerates pages whose content has changed unless
    force=True.

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        force: If True, regenerate all pages even if content is unchanged.

    Returns:
        Status with pages_generated, pages_updated, pages_unchanged counts.
    """
    from ..incremental import get_data_dir
    from ..wiki import generate_wiki

    store = None
    try:
        store, root = _get_store(repo_root)
        wiki_dir = get_data_dir(root) / "wiki"
        result = generate_wiki(store, wiki_dir, force=force)
        total = result["pages_generated"] + result["pages_updated"] + result["pages_unchanged"]
        return {
            "status": "ok",
            "summary": (
                f"Wiki generated: {result['pages_generated']} new, "
                f"{result['pages_updated']} updated, "
                f"{result['pages_unchanged']} unchanged "
                f"({total} total pages)"
            ),
            "wiki_dir": str(wiki_dir),
            **result,
        }
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="generate_wiki")
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 20: get_wiki_page  [DOCS]
# ---------------------------------------------------------------------------


def get_wiki_page_func(
    community_name: str,
    repo_root: str | None = None,
) -> ToolPayload:
    """Retrieve a specific wiki page by community name.

    [DOCS] Returns the markdown content of the wiki page for the given
    community. The wiki must have been generated first via generate_wiki.

    Args:
        community_name: Community name to look up (slugified for filename).
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Page content or not_found status.
    """
    from ..incremental import get_data_dir
    from ..wiki import get_wiki_page

    _, root = _get_store(repo_root)
    wiki_dir = get_data_dir(root) / "wiki"
    content = get_wiki_page(wiki_dir, community_name)
    if content is None:
        return {
            "status": "not_found",
            "summary": (
                f"No wiki page found for '{community_name}'. "
                "Run generate_wiki_tool first to build the wiki."
            ),
            "next_tool_suggestions": ["generate_wiki_tool -- build wiki pages from communities"],
        }
    return {
        "status": "ok",
        "summary": (f"Wiki page for '{community_name}' ({len(content)} chars)"),
        "content": content,
    }
