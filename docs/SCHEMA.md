# Schema overview

<!-- constrained-by ./ARCHITECTURE.md -->

## Nodes

<!-- derived-from ./ARCHITECTURE.md#storage-model -->
<!-- constrained-by ./ARCHITECTURE.md#in-memory-representation -->

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

The Rust parser represents these labels as `NodeKind` and converts to the
string form only when writing `NodeInput` / SQLite. `Type` and `DocBody` are
part of that closed set, not ad-hoc strings. In memory, `file_path` is a
shared `FilePath` (`Arc<str>`); every node and edge from one file clones the
same handle. Persistence still stores a string.

## Edges

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->
<!-- constrained-by ./ARCHITECTURE.md#in-memory-representation -->

Edge kinds include:

- `CALLS`
- `IMPORTS_FROM`
- `REFERENCES`
- `CONTAINS`
- `INHERITS`
- `IMPLEMENTS`
- `TESTED_BY`
- `DEPENDS_ON`
- `CROSS_ARTIFACT` — cross-boundary references between artifacts (cross-language process/FFI bridges, Markdown → code symbol references, Terraform → application-code path/entrypoint bridges, manifest-backed native-extension / generated-client links). Carries `bridge_kind`, `relationship_role`, `evidence_kind`, and `confidence_tier` in `extra`. Markdown code-span candidates carry `extra.original_symbol_name` while they are being resolved; post-processing keeps them only when they resolve uniquely to a non-Markdown symbol. Explicit `dagayn:` documentation directives may remain unresolved because they are author-declared dependencies. Terraform `handler` / `entry_point` bridges similarly carry `original_symbol_name` and resolve when a unique Function/Test match exists. Manifest bridges set `extra.extractor=manifest_bridges` and roles such as `builds_artifact`, `generates_code`, and `binds_generated_client`.

The Rust parser represents these labels as `EdgeKind`, including `IMPLEMENTS`.
The persisted / Python form remains the uppercase string.

`TESTED_BY` edges are directed from the covered production symbol to the test
symbol that exercises it. For example, `src/auth.py::login -> tests/test_auth.py::test_login`.
Parsers derive them from the test's `CALLS` edges (same file and line), so a
bare call target first gives a bare `TESTED_BY` source. When post-processing's
bare-name resolution binds the call, the `TESTED_BY` edge takes the same
qualified target and confidence; a bare edge whose call is still unresolved
stays bare.

Bare-name resolution binds a `CALLS`, `INHERITS`, or `IMPLEMENTS` target only
when exactly one candidate is visible to the source file, and grades the edge
by that visibility:

- `HIGH` (`0.9`): a top-level function or class in the same file or in a file
  the source imports directly.
- `MEDIUM` (`0.6`): a method, whose receiver type is unknown, or a symbol the
  source reaches only through a shared or imported namespace or through its
  class declaration (a C++ header).

A call on a named type (Ruby `Fast.fast_sum(...)`, C# `Native.Total(...)`)
carries `receiver_type` on its `CALLS` edge. Only that type's methods in the
caller's language are candidates; the visible one wins as above, and when
none is visible (a Ruby `require` or a namespace-less C# file does not make a
file visible) a single such method is bound at `MEDIUM`. A type with no such
method in the repository (`Math.Max`) leaves the call unbound rather than
binding it to an unrelated function of the same name.

`query_graph_tool` reports `confidence: "high"` when every returned edge is
`EXACT`, `EXTRACTED`, or `HIGH`, and `"medium"` otherwise.

Python `from pkg import name` records an `IMPORTS_FROM` edge to `pkg/name.py`
(or `pkg/name/__init__.py`) when `name` is a submodule, the same as
`import pkg.name`. It points at `pkg/__init__.py` only for names that are not
submodules and for `*`.

The fork also stores confidence-related metadata and graph relationships used by higher-order analysis.

## Language extraction models

Each extractor maps its language onto the shared node and edge kinds above.
TypeScript, TSX, JavaScript, and JSX (including Vue and Svelte script blocks)
are specified in [TYPESCRIPT-EXTRACTION.md](./TYPESCRIPT-EXTRACTION.md): which
declarations become `Class` / `Type` / `Function` nodes, how owner paths,
anonymous `default` exports, object-literal containers, and external
`pkg::symbol` targets appear in qualified names, and what `CALLS`,
`REFERENCES`, `INHERITS`, and `IMPLEMENTS` mean for TypeScript code.

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

Triggers on `embeddings` bump a single-row `embeddings_generation` counter
(with a random `epoch` per database) on every insert, update, or delete. Native
semantic search keys its in-memory vector matrix on that counter, so graph
writes that leave the vectors alone do not force the matrix to be reloaded.

The exact schema can evolve, but the stable user-facing idea is simple: the graph preserves enough structure to answer review and exploration questions without rescanning the full repository every time.
