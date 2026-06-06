---
name: semantic-search
description: Configure, build, and verify dagayn embeddings and hybrid semantic search without losing FTS fallback behavior.
argument-hint: "[query]"
---

# Semantic Search

Use this when semantic search quality, embedding setup, or hybrid search status
matters.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so agents can avoid stale or wasteful search advice.
<!-- /dagayn skill embedding context -->

## Workflow

1. Start with graph freshness:
   ```bash
   dagayn tool list_graph_stats_tool
   ```
2. Run the intended search and read `search_mode` / per-result `source`:
   ```bash
   dagayn tool semantic_search_nodes_tool --arg query='"auth handler"' --arg detail_level='"minimal"'
   ```
   `search_mode="hybrid"` means embeddings and FTS were merged. `fts_only`
   is still valid, but semantic recall is lower.
3. If embeddings are missing or stale, build them through the graph tools:
   - Incremental local refresh: `build_or_update_graph_tool(local_embedding="bge-m3")`
   - Dedicated embedding pass: `embed_graph_tool`
   - Full local refresh: `build_or_update_graph_tool(full_rebuild=True, local_embedding="bge-m3")` only when explicitly doing embedding-quality or end-to-end maintenance work.
   Before any embedding-enabled full rebuild, state the reason and get explicit
   confirmation from the user; do not use it for parser, flow, documentation, or
   ordinary implementation verification.
4. For CLI fallback:
   ```bash
   dagayn build --local-embedding
   dagayn update --local-embedding
   dagayn tool embed_graph_tool
   ```
5. Re-run the same `semantic_search_nodes_tool` query and compare result count,
   `search_mode`, and whether high-value hits now have `source="embedding"` or
   `source="both"`.

## Troubleshooting

- `fts_only` is acceptable for exact symbol/name lookup; do not rebuild
  embeddings just to find a precise identifier.
- Use local BGE-M3 for reusable developer environments when embeddings are
  useful; use FTS-only when startup time or memory is tight.
- If Qwen sidecar startup fails, check the local server binary (`auto` or
  `llama-server`), port, and timeout before changing graph data.
- If provider imports are unavailable, keep going with FTS and report the
  reduced recall instead of blocking unrelated work.

## Efficiency Rules

- Use FTS-only results for exact names; use embeddings for fuzzy concepts,
  unfamiliar domain terms, and cross-language search.
- Do one before/after query to prove search quality changed. Do not rebuild the
  graph repeatedly without a changed file set or a failed verification.
- Never use an embedding-enabled full rebuild to compensate for untracked files
  not appearing in graph queries. Stage or otherwise expose the files first,
  then run the smallest non-embedding graph refresh that proves the claim.
