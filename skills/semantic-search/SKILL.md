---
name: semantic-search
description: Find code or docs by meaning when the name is unknown with dagayn's hybrid search (FTS + embeddings), read why each hit ranked, and set up or fix embeddings. Use when the user describes behavior instead of an identifier, search results look wrong or thin, `search_mode` is not hybrid, or embeddings need building or switching.
argument-hint: "[query]"
---

# Semantic Search

Search finds a starting node; the graph proves what that node does. Read how a
result was found before trusting it, then move to `source_of` and relationship
queries.

## Evidence

Rank results with the Highest / Medium / Low trust tiers in the installed
dagayn instructions (full rules: `get_docs_section_tool(section_name="trust")`).

In search terms: hybrid or embedding hits are Medium discovery, `fts_only`
hits are keyword candidates, and `keyword_fallback` or
`multiple_exact_matches` stay Low until you pick one node and read its
`source_of`.


<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so agents can avoid stale or wasteful search advice.
<!-- /dagayn skill embedding context -->

## Workflow

<!-- derived-from ../../docs/ARCHITECTURE.md#hybrid-search -->

1. **Check the graph** with `get_minimal_context_tool`; skip ensure when
   `graph_health.status` is `ok`, and follow `next` when it is
   `empty`.
2. **Search and read how it ranked**:
   `semantic_search_nodes_tool(query="auth handler")`.
   - `search_mode="hybrid"`: FTS and embeddings were merged. `fts_only` is
     still fine for names, with lower fuzzy recall. `embedding_only`: only the
     vector arm hit. `keyword_fallback`: neither FTS nor vectors returned
     anything, so LIKE matching ran — the index may exist with no match, so
     check `fts_health` (standard output) before refreshing anything. `empty`:
     nothing matched.
   - Per-result `source` (`fts`, `embedding`, `both`, `keyword`, `doc` for
     Markdown sections) explains each hit. It is only in the default
     `detail_level="standard"`; `"minimal"` keeps name, kind, location, score,
     and `evidence_type` for the top five.
   - `exactness.exact_match_count` says how many hits match the query as a
     `name` / `qualified_name` exactly; `ambiguity:
     "multiple_exact_matches"` means you must pick one.
   - `embedding_health.requested_text_mode` (with `detail_level="verbose"`)
     shows how the query was routed:
     `narrative` for process-pattern prose (calls, reads, writes, loops),
     `material` otherwise.
3. **Hand off the best hit to graph tools**:
   - One exact match → `query_graph_tool(pattern="source_of")`, then the
     relationship you need (callers, callees, tests, docs, imports, children).
   - Several plausible fuzzy hits → `source_of` or `file_summary` on the top
     few before concluding.
   - The result's `next` comes first.
   - `traverse_graph_tool` (advanced surface: `dagayn serve --tools all` or
     `dagayn tool`) is for a bounded neighborhood once the start node is clear.
   Treat semantic search as start-node discovery, not final proof.
4. **Refresh embeddings only when recall actually matters** (advanced surface or
   `dagayn tool`):
   - Incremental: `build_or_update_graph_tool(local_embedding="bge-m3")`, or a
     dedicated pass with `embed_graph_tool`.
   - Process-pattern recall needs narrative vectors: run an extra embedding
     pass with `DAGAYN_EMBEDDING_TEXT_MODE=narrative` (vectors are kept per
     provider and text mode).
   - An embedding-enabled full rebuild (`full_rebuild=True,
     local_embedding="bge-m3"`) is only for when you are
     explicitly doing embedding-quality or end-to-end maintenance work: it is slow and touches
     every vector, so state the reason and get the user's go-ahead first, and
     never use it for parser, flow, or doc verification.
   - When `dagayn serve` already runs with `--local-embedding`,
     `ensure_graph_tool` / `dagayn session prepare` keep vectors current.
5. **Prove the change**: re-run the same query and compare result count,
   `search_mode`, `requested_text_mode`, and how many top hits now come from
   `embedding` or `both`.

## Embedding modes

| Mode | `build` / `update` / `serve` flag | `dagayn install --mode` | Notes |
|---|---|---|---|
| None (FTS only) | `--local-embedding none` | `fts-only` | fastest; fine for exact names |
| BGE-M3 (default local) | `--local-embedding` (or `--mode bge-m3`) | `local-embedding` | managed llama.cpp sidecar, port 18080 |
| Qwen3 | `--local-embedding llama-qwen3` (or `low`) | `local-embedding-llama` | managed sidecar, port 18081 |
| Remote | — | `remote-embedding --provider openai\|google\|minimax` | network calls per embedding |

`dagayn session prepare --embedding auto|defer|skip|inline` controls when a
session refresh embeds.

## Troubleshooting

- Don't rebuild embeddings to find a precise identifier; `fts_only` handles it.
- `provider_mismatch` / `missing_vectors` in `embedding_health` (`verbose`): no vectors for
  this provider and text mode. Refresh only if the task needs fuzzy recall.
- A sidecar that won't start: check the server binary (`auto` /
  `llama-server`), the port, and the timeout before touching graph data.
- Provider imports unavailable: continue on FTS and report the reduced recall.
- Untracked files are indexed (git's tracked + untracked, minus ignored); if
  they're missing, refresh with `ensure_graph_tool(force=True)` instead of an
  embedding rebuild.

## CLI

```bash
dagayn tool semantic_search_nodes_tool --arg query='"auth handler"'
dagayn tool embed_graph_tool
dagayn build --local-embedding
dagayn session prepare --local-embedding
```
