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

<!-- derived-from #storage-model -->

## In-memory representation

<!-- constrained-by #storage-model -->
<!-- constrained-by ./SCHEMA.md#nodes -->

SQLite and the Python `GraphStore` API still speak strings and JSON. The Rust
core keeps a stricter in-memory form so the type says what the value is:

- Closed vocabularies are enums. Parser `NodeKind` / `EdgeKind` cover the
  schema labels (`File`, `Type`, `DocBody`, `IMPLEMENTS`, …). The store
  boundary converts with `as_str()`.
- A sequence that is never grown after construction is `Box<[T]>`, not
  `Vec<T>`. Flow paths, community member lists, graph-stat language lists, and
  Brandes adjacency are frozen slices: 16 bytes instead of 24, and no spare
  capacity pretending the list is still a builder.
- A value that two indexes need is shared, not cloned. Flow tracing keeps one
  `Arc<GraphNode>` per node and two maps (`qualified_name`, `id`) that point
  at it. Parser nodes and edges from one file share one `FilePath` (`Arc<str>`);
  clone is an Arc bump. The SQLite / Python boundary still stores a plain
  string.

`Vec` stays on the write path: parsers still push into growable buffers, and
SQLite loaders still collect rows. Freeze at the point the collection becomes
a fact.

<!-- derived-from #in-memory-representation -->

## GraphStore ownership boundary

The Python `GraphStore` remains the compatibility and orchestration boundary for CLI, MCP tools, tests, and Python-only analysis helpers. It owns the stable SQLite schema, path normalization, transaction semantics, cache invalidation, and the public read/write methods that higher layers call.

The Rust graph backend owns hot-path storage and analysis implementations as they become available through PyO3 bindings. Python code may call Rust methods when the bound method exists, but it should preserve the Python `GraphStore` API as the migration contract. New callers should depend on `GraphStore` methods rather than importing Rust bindings directly.

During the migration, duplicated behavior is allowed only as an adapter layer: Python keeps the canonical user-facing API and fallback semantics, while Rust implementations are treated as accelerated implementations behind that API. When a Rust path becomes the only supported implementation, the matching Python docs and tests should be updated in the same change.

Current Rust-owned GraphStore responsibilities include batch file storage, Rust-owned parse/store paths, flow and community JSON persistence, Markdown artifact reference resolution, and persisted centrality score computation for `hub_scores` / `bridge_scores`. Python keeps fallback implementations for source checkouts or environments without `dagayn._core`.

## Post-processing

Optional post-processing layers add:

- communities
- execution flows (CALLS reachable sets from entry points; `path` / `steps` are BFS visit order, not a call sequence; truncation is disclosed)
- search indexes (FTS5 virtual table `nodes_fts`, always available after `build`)
- embedding store (the `embeddings` table inside `.dagayn/graph.db`, populated by `embed_graph_tool` or `--local-embedding`)
- persisted centrality tables (`hub_scores`, `bridge_scores`) used by architecture analysis after post-processing

## Hybrid search

`semantic_search_nodes` runs two ranked retrieval arms in parallel and merges them with Reciprocal Rank Fusion (RRF, k=10):

1. **FTS5 BM25** — full-text search over the `nodes_fts` virtual table (porter + unicode61 tokenizer, with Japanese source/document text pre-segmented before insertion). Always available. The index stores symbol names, qualified names, paths, signatures, generated identifier tokens (for example `OpenAIEmbeddingProvider` → `open ai embedding provider`), structured code-reference text, and bounded source/document text such as docstrings and Markdown section bodies. On the native store, Japanese kana / CJK / Hangul is indexed as Lindera IPADIC morphemes (plus dictionary base forms) *and* overlapping CJK bigrams; queries keep content morphemes and drop particles, auxiliaries, and light verbs such as `する`, so an inflected query like `検索する` AND-matches `検索を行う` instead of falling through to OR or missing entirely. ASCII spans stay intact so mixed queries such as `GraphStoreで自然言語検索` still hit. If dictionary load fails at runtime, covering (non-overlapping) bigrams with the same stop list still run. CJK identifier names also land in `identifier_tokens` (BM25 weight 5), not only `doc_text`. The Python store still uses optional MeCab-compatible wakati when those packages are installed, with the same ASCII-preserving bigram fallback. The query is fired once as a whole, then re-fired once per identifier-shaped token (snake_case / PascalCase / camelCase) extracted from natural-language phrasing so a query like `"tests for embed_graph"` still hits the `embed_graph` symbol directly.
2. **Cosine similarity** — vector search over the embedding store. Available only when embeddings have been built.

The RRF constant is 10 (rather than the textbook 60) so the resulting `score` field spreads over ~0.05-0.2 instead of being compressed into 0.015-0.016. The constant is a calibration knob for how strongly top ranks dominate. A positive `k` preserves the order of a single ranked list, but multi-list fusion can reorder items when another arm contributes additional evidence.

Results are post-processed with:
- **Kind boost** — query heuristic: PascalCase → 1.5× for classes/types; snake_case → 1.5× for functions; dotted path → 2.0× for qualified names.
- **Context-file boost** — 1.5× for nodes in files passed as `context_files`.
- **Intent rerank** — queries are classified with lightweight token heuristics. Exact identifier-like queries keep FTS/name matching dominant. Purpose-style prose uses the `material` embedding text because names and adjacent comments usually carry intent. Process-pattern prose uses the `narrative` embedding text because static source and graph facts expose operations such as calls, reads, writes, returns, loops, merges, searches, and rebuilds. Documentation queries favor Markdown sections. Top-ranked FTS or embedding hits get a small confidence boost so a strong single-arm signal is not lost in RRF.
- **Test deboost** — 0.6× for nodes detected as test code (`is_test=True`). Tests cluster textually next to the functions they exercise and would otherwise crowd out the source on semantic queries. Tests remain visible (deboost, not filter) and the deboost is skipped for explicit test/coverage queries.

Fallback chain: hybrid → FTS-only (no embeddings) → embedding-only (FTS index corrupt) → LIKE keyword (FTS index absent).

The `search_mode` field in the response reports which path ran: `"hybrid"`, `"fts_only"`, `"embedding_only"`, or `"keyword_fallback"`. Per-result `source` tags each hit as `"fts"`, `"embedding"`, `"both"`, or `"keyword"`. Per-result `is_test` reports whether the node was detected as test code.

Embedding rows are partitioned by provider and text mode, so the same node can
keep both `material` and `narrative` vectors for intent-routed hybrid search.

### Japanese FTS quality gates

<!-- derived-from #hybrid-search -->
<!-- derived-from ../tests/fixtures/japanese_search/README.md -->

Public Japanese IR sets (MIRACL-ja, JaQuAD / JSQuAD, mMARCO-ja, Livedoor news)
are passage retrieval over Wikipedia or news. They do not produce dagayn
`DocSection` / `Function` / Terraform nodes, so they cannot score the FTS
path agents actually call. This repository's `README.ja-JP.md` is authentic
product Japanese, but it is one file: it has no shared-term collisions, no
inflected `検索する` vs `検索を行う` pair, no CJK identifiers, and no
Terraform comments.

The corpus of record is the vendored mixed fixture
`tests/fixtures/japanese_search/` (~65 parsed nodes): Markdown docs where
`検索` appears in NLP, UI, ops, install, and infra; Python symbols with
Japanese docstrings and a CJK function name; a Terraform OpenSearch
resource. Queries live in `queries.json`. The 7-node inline distractor was
kept only as historical baseline numbers below.

Overlapping-bigram-only indexing on that 7-node set already got exact
titles, mixed English+Japanese, and CJK identifiers to hit@1, but
`検索する` against a body that says `検索を行う` returned no hits, and longer
inflected queries only matched after the OR fallback.

Native Lindera targets, locked in
`japanese_fts_quality_gates_hit_inflected_and_identifier_queries` against
the fixture (family match: DocSection, DocBody, or the related symbol):

- Exact title, mixed `GraphStore 自然言語検索`, `検索ボタン`, `トークン検証`,
  English `verify_token`, `課金バッチ`, CJK `ユーザー取得`, hybrid-search and
  Terraform headings: **hit@1** on the owning file or name family
- Inflected `自然言語検索する` and long particle-heavy prose whose content
  words are in the document: **hit@1** with **`match_mode=and`**
- Bare `検索する`: **hit@5** among search-related names (NLP/UI/ops/OpenSearch);
  not required to rank the NLP heading first once `検索` is a shared term
- Fixture FTS rebuild under 500ms; inflected query p95 under 10ms
- Synonyms such as `認証` vs `トークン検証` stay out of scope for tokenization

## Query surfaces

The MCP layer and CLI build on the same graph store. Review, architecture, traversal, and refactor tools all read from the same local dataset.
