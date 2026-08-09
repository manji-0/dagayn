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

<!-- derived-from ../../docs/ARCHITECTURE.md#hybrid-search -->

1. Start with graph freshness via the default MCP surface:
   ```bash
   dagayn tool get_minimal_context_tool --arg 'task="semantic search setup"'
   dagayn tool ensure_graph_tool
   ```
   If `graph_health.status` is already healthy, skip ensure. Use
   `list_graph_stats_tool` only when the advanced surface is available and you
   need detailed counts.
2. Run the intended search and read `search_mode` / per-result `source`:
   ```bash
   dagayn tool semantic_search_nodes_tool --arg query='"auth handler"' --arg detail_level='"minimal"'
   ```
   `search_mode="hybrid"` means embeddings and FTS were merged. `fts_only`
   is still valid, but semantic recall is lower. `embedding_only` means the
   vector path ran while FTS was unavailable, and `keyword_fallback` means the
   FTS index was absent and only LIKE matching ran.
   - For exact symbol/name lookup, trust exact `name` / `qualified_name` hits
     even when `search_mode` is `fts_only`.
   - For fuzzy purpose searches, prefer high-ranked hits with `source="both"`
     or `source="embedding"`.
   - For process-pattern prose, check `rerank_intent="process_pattern"`; this
     routes to narrative embeddings when they are available.
   - Use per-result `source` to explain why a hit ranked: `fts`, `embedding`,
     `both`, or `keyword`.
3. Hand off the best hit to graph tools:
   - If exactly one result matches the intended `name` or `qualified_name`, use
     that `qualified_name` for `query_graph_tool`.
   - If several fuzzy hits look plausible, inspect the top 3 with
     `query_graph_tool(pattern="file_summary")` or a relationship query before
     drawing conclusions.
   - If the user wants callers, callees, tests, docs, imports, or children, use
     `query_graph_tool`; use `traverse_graph_tool` only for a bounded
     neighborhood after the start node is clear.
   - If the search result has a `next_action`, follow it before widening the
     query.
4. If embeddings are missing or stale, build them through maintenance tools
   (`dagayn serve --tools all` or `dagayn tool`):
   - Incremental local refresh: `build_or_update_graph_tool(local_embedding="bge-m3")`
   - Dedicated embedding pass: `embed_graph_tool`
   - Full local refresh: `build_or_update_graph_tool(full_rebuild=True, local_embedding="bge-m3")` only when explicitly doing embedding-quality or end-to-end maintenance work.
   Before any embedding-enabled full rebuild, state the reason and get explicit
   confirmation from the user; do not use it for parser, flow, documentation, or
   ordinary implementation verification. Prefer `ensure_graph_tool` /
   `dagayn session prepare` when serve already runs with `--local-embedding`;
   otherwise use `build_or_update_graph_tool` / `embed_graph_tool` for an
   explicit embedding refresh.
5. For CLI fallback:
   ```bash
   dagayn build --local-embedding
   dagayn update --local-embedding
   dagayn session prepare --local-embedding
   dagayn tool embed_graph_tool
   ```
6. Re-run the same `semantic_search_nodes_tool` query and compare result count,
   `search_mode`, `rerank_intent`, and whether high-value hits now have
   `source="embedding"` or `source="both"`.

## Troubleshooting

- `fts_only` is acceptable for exact symbol/name lookup; do not rebuild
  embeddings just to find a precise identifier.
- `keyword_fallback` means the FTS index is absent; refresh graph/FTS before
  drawing conclusions about search quality.
- `provider_mismatch` or `missing_vectors` in embedding health means the query
  did not find matching vectors for the selected provider/text mode. Refresh
  embeddings only when the task actually needs fuzzy recall.
- Use local BGE-M3 for reusable developer environments when embeddings are
  useful; use FTS-only when startup time or memory is tight.
- If Qwen sidecar startup fails, check the local server binary (`auto` or
  `llama-server`), port, and timeout before changing graph data.
- If provider imports are unavailable, keep going with FTS and report the
  reduced recall instead of blocking unrelated work.

## Efficiency Rules

- Use FTS-only results for exact names; use material embeddings for fuzzy
  purpose concepts; use narrative embeddings/process-pattern intent for queries
  about behavior such as calls, reads, writes, loops, returns, or merges.
- Treat semantic search as start-node discovery, not final proof. Confirm
  behavior, relationships, or coverage with `query_graph_tool`, `flow_tool`,
  `review_tool`, or source reads.
- Do one before/after query to prove search quality changed. Do not rebuild the
  graph repeatedly without a changed file set or a failed verification.
- Never use an embedding-enabled full rebuild to compensate for untracked files
  not appearing in graph queries. Stage or otherwise expose the files first,
  then run the smallest non-embedding graph refresh that proves the claim.
