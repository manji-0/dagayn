# Schema overview

<!-- constrained-by ./ARCHITECTURE.md -->

## Nodes

<!-- derived-from ./ARCHITECTURE.md#storage-model -->

Core node kinds include:

- `File`
- `Class`
- `Function`
- `Type`
- `Test`
- `DocSection` — Markdown heading (`#`, `##`, …). Distinguished from `Class` to reduce search noise when querying for code symbols.
- `DocBody` — Markdown prose/list/table/code blocks attached to the nearest
  `DocSection`, used for finer-grained documentation search and embeddings.

Nodes store file path, qualified name, language, line range, and an `extra` payload for format-specific metadata.

## Edges

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

Edge kinds include:

- `CALLS`
- `IMPORTS_FROM`
- `REFERENCES`
- `CONTAINS`
- `INHERITS`
- `IMPLEMENTS`
- `TESTED_BY`
- `DEPENDS_ON`
- `CROSS_ARTIFACT` — cross-boundary references between artifacts (cross-language process/FFI bridges, Markdown → code symbol references). Carries `bridge_kind`, `relationship_role`, `evidence_kind`, and `confidence_tier` in `extra`. Markdown-sourced edges also carry `extra.original_symbol_name` — the raw backtick span symbol — so the idempotent resolver can re-evaluate them on every postprocess run without requiring a Markdown re-parse.

`TESTED_BY` edges are directed from the covered production symbol to the test
symbol that exercises it. For example, `src/auth.py::login -> tests/test_auth.py::test_login`.

The fork also stores confidence-related metadata and graph relationships used by higher-order analysis.

## Metadata

<!-- derived-from ./ARCHITECTURE.md#storage-model -->

The metadata table tracks graph-level state such as build timing, VCS information, and repo root information needed for path normalization.

## Derived structures

<!-- derived-from ./ARCHITECTURE.md#post-processing -->

Post-processing may populate additional tables for:

- communities
- flow memberships
- full-text search
- embeddings

The exact schema can evolve, but the stable user-facing idea is simple: the graph preserves enough structure to answer review and exploration questions without rescanning the full repository every time.
