# Architecture

## Pipeline overview

`dagayn` turns repository contents into a local knowledge graph through five main stages:

1. file discovery and language detection
2. parser extraction into nodes and edges
3. SQLite persistence
4. optional post-processing for flows, communities, and search indexes
5. query-time analysis for reviews, search, and refactors

## Parsing model

The fork uses Tree-sitter where possible, plus targeted fallbacks for formats that need custom handling.

Important fork-specific parser work includes:

- commit-pinned Terraform grammar support fetched from the fork
- commit-pinned Markdown grammar support for directive-style comments
- notebook parsing that preserves per-cell attribution

## Storage model

Graph data is stored in SQLite. Nodes and edges carry file identity, qualified names, and extra metadata used by downstream analysis.

In dagayn-oriented workflows, registered paths are expected to be relative to the repository root. That keeps graph output stable across symlinked temp paths and portable across machines.

## Post-processing

Optional post-processing layers add:

- communities
- execution flows
- search indexes (FTS5 virtual table `nodes_fts`, always available after `build`)
- embedding store (`.dagayn/embeddings.db`, populated by `embed_graph_tool` or `--local-embedding`)

## Hybrid search

`semantic_search_nodes` runs two ranked retrieval arms in parallel and merges them with Reciprocal Rank Fusion (RRF, k=10):

1. **FTS5 BM25** — full-text search over the `nodes_fts` virtual table (porter + unicode61 tokenizer). Always available. The index stores symbol names, qualified names, paths, signatures, generated identifier tokens (for example `LocalEmbeddingProvider` → `local embedding provider`), and bounded source/document text such as docstrings and Markdown section bodies. The query is fired once as a whole, then re-fired once per identifier-shaped token (snake_case / PascalCase / camelCase) extracted from natural-language phrasing so a query like `"tests for embed_graph"` still hits the `embed_graph` symbol directly.
2. **Cosine similarity** — vector search over the embedding store. Available only when embeddings have been built.

The RRF constant is 10 (rather than the textbook 60) so the resulting `score` field spreads over ~0.05–0.2 instead of being compressed into 0.015–0.016. The constant is a calibration knob only: positive `k` preserves item order, it just affects the magnitude of the merged score.

Results are post-processed with:
- **Kind boost** — query heuristic: PascalCase → 1.5× for classes/types; snake_case → 1.5× for functions; dotted path → 2.0× for qualified names.
- **Context-file boost** — 1.5× for nodes in files passed as `context_files`.
- **Intent rerank** — long natural-language queries are classified with lightweight token heuristics. Implementation/code/test queries favor code artifacts and deboost Markdown unless the query explicitly asks for documentation; documentation queries favor Markdown sections; top-ranked FTS or embedding hits get a small confidence boost so a strong single-arm signal is not lost in RRF.
- **Test deboost** — 0.6× for nodes detected as test code (`is_test=True`). Tests cluster textually next to the functions they exercise and would otherwise crowd out the source on semantic queries. Tests remain visible (deboost, not filter) and the deboost is skipped for explicit test/coverage queries.

Fallback chain: hybrid → FTS-only (no embeddings) → embedding-only (FTS index corrupt) → LIKE keyword (FTS index absent).

The `search_mode` field in the response reports which path ran: `"hybrid"`, `"fts_only"`, `"embedding_only"`, or `"keyword_fallback"`. Per-result `source` tags each hit as `"fts"`, `"embedding"`, `"both"`, or `"keyword"`. Per-result `is_test` reports whether the node was detected as test code.

## Query surfaces

The MCP layer and CLI build on the same graph store. Review, architecture, traversal, and refactor tools all read from the same local dataset.
