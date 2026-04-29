# Schema overview

<!-- constrained-by ./ARCHITECTURE.md -->

## Nodes

Core node kinds include:

- `File`
- `Class`
- `Function`
- `Type`
- `Test`

Nodes store file path, qualified name, language, line range, and an `extra` payload for format-specific metadata.

## Edges

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

The fork also stores confidence-related metadata and graph relationships used by higher-order analysis.

## Metadata

The metadata table tracks graph-level state such as build timing, VCS information, and repo root information needed for path normalization.

## Derived structures

Post-processing may populate additional tables for:

- communities
- flow memberships
- full-text search
- embeddings

The exact schema can evolve, but the stable user-facing idea is simple: the graph preserves enough structure to answer review and exploration questions without rescanning the full repository every time.
