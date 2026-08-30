# SAP metrics specification

<!-- constrained-by ./SCHEMA.md -->
<!-- constrained-by ./ARCHITECTURE.md -->

> **Status:** Implemented. SAP is exposed through
> `architecture_analysis_tool(mode="sap_metrics")` and
> `architecture_analysis_tool(mode="sap_violations")`. See `dagayn/sap.py`,
> `dagayn/tools/sap_tools.py`, and `tests/test_sap.py`.

## Purpose

This specification defines how dagayn measures package-level SAP (Stable Abstractions Principle) metrics:

- **A** — abstractness
- **I** — instability
- **D** — distance from the main sequence

The target use case is architectural review for mixed-language repositories, not language-pure textbook examples only.

## Scope model

The default analysis unit is a logical **package**.

`package` means:

- a language-native package or namespace when one is clearly available
- otherwise a repo-root-relative directory boundary

Alternative scope kinds:

- `file`
- `directory`
- `community`

The chosen scope key must remain **repo-root-relative** so results stay stable across machines and temporary paths.

SAP runs with `artifact_scope="code"` by default. Markdown documentation nodes
and Markdown-authored dependency directives are excluded from code SAP counts so
documentation structure does not change code Ca/Ce/instability values. Use
`artifact_scope="docs"` to inspect documentation-only dependencies, or
`artifact_scope="all"` when intentionally comparing against the legacy mixed
graph.

Language-specific package resolution:

- **Python** — nearest importable package root, else parent directory
- **Java / Kotlin / Scala / C#** — declared package or namespace when available, else directory
- **TypeScript / JavaScript / Go / Rust / Swift** — repo-relative directory, optionally cut at workspace markers
- **mixed-language fallback** — normalized repo-relative directory

## Type classification contract

Eligible declarations are normalized into a shared abstraction model.

Each relevant node exposes:

- `extra.type_role`
- `extra.is_abstract`
- `extra.is_contract`
- language-specific metadata such as Rust `extra.derive_traits` and visibility in `modifiers`

Normalized `type_role` values:

- `class`
- `abstract_class`
- `interface`
- `protocol`
- `trait`
- `abstract_type`
- `mixin`
- `enum`
- `struct`
- `record`
- `alias`

Language mapping:

- Java/C#/PHP interfaces → `interface`
- C# `record` and `record struct` types → `record`
- C# `using Alias = Type` and Python type aliases → `alias`
- Swift `protocol_declaration` → `protocol`
- Rust `trait` items → `trait`; `impl Trait for Type` emits `IMPLEMENTS`
- Scala traits → `trait`
- Julia `abstract_definition` → `abstract_type`
- Dart abstract classes → `abstract_class`
- Python `Protocol` bases → `protocol`; `ABC`/`ABCMeta` → `abstract_class`
- Go interface declarations → `interface`

## Edge semantics

**Default and only** dependency edges for Ce/Ca (fixed, not configurable):

- `IMPORTS_FROM` — explicit module-level import
- `DEPENDS_ON` — generic dependency (used by Terraform, Markdown, and other non-import languages)
- `INHERITS` — nominal inheritance or subtype extension
- `IMPLEMENTS` — interface/protocol/trait conformance

`CALLS` and `REFERENCES` are excluded because they produce noise in dynamic languages (e.g., calling `len()`) and do not cleanly signal cross-boundary coupling.

Artifact scope is applied before dependency projection. In the default `code`
scope, a Markdown `DEPENDS_ON` edge to a source file is ignored because the
documentation endpoint is outside the analysis scope. In `docs` scope, only
Markdown documentation nodes participate. In `all` scope, code and documentation
are projected together for compatibility with older mixed-graph reports.

### Type-name fallback resolution

`INHERITS`/`IMPLEMENTS` targets are bare type names (e.g., `EmbeddingProvider`) rather than qualified paths.
Resolution proceeds in two stages:
1. Try `edge.target` as a qualified name (file-path-prefixed)
2. If not found, try `edge.target` as a bare name — succeeds only when **exactly one** node in the repo has that name
3. If ambiguous (multiple nodes share the name) or not found, the edge is silently dropped

Stdlib types (`ABC`, `list`, etc.) are dropped in stage 2 because they have no matching repo node.

## Metric formulas

For each scope:

- `Na` = number of abstract or contract-like types
- `Nt` = number of eligible top-level types
- `Ce` = number of distinct outgoing dependent scopes
- `Ca` = number of distinct incoming dependent scopes

Derived metrics:

- `A = Na / Nt`
- `I = Ce / (Ca + Ce)`
- `D = |A + I - 1|`

If `Nt = 0`, report `A = 0.0` and mark the scope as having no eligible types.

If `Ca + Ce = 0`, report `I = 0.0` and mark the scope as isolated.

Scopes with no eligible types or no dependency coupling are SAP-inapplicable:
their raw `D` value is still available for inspection, but it is not a
main-sequence quality signal.

## Output contract

A SAP result row includes:

- `scope_kind`
- `scope_key`
- `display_name`
- `na`
- `nt`
- `ca`
- `ce`
- `abstractness`
- `instability`
- `distance`
- `sap_applicable`
- `applicability_reason`
- `top_incoming_dependencies`
- `top_outgoing_dependencies`
- optional `notes` such as `no-eligible-types`, `isolated`, `test-scope`, and `fixture-scope`

`architecture_analysis_tool(mode="sap_metrics")` separates SAP-inapplicable
scopes into `inapplicable_metrics` by default so raw `no-eligible-types` or
`isolated` rows do not sort above actionable architecture signals. Pass
`detail_level="verbose"` to include those rows in the main `metrics` list.
Rows include `dependency_profile`; the default `strict_static` profile preserves
the historical SAP edge set. Use `implementation`, `infra_dataflow`, or
`artifact_trace` only when the question is explicitly about call dependencies,
Terraform/dataflow references, or high-confidence code/docs/infra traceability.

`architecture_analysis_tool(mode="sap_violations")` and
`detect_sap_violations_func()` suppress `test-scope` and `fixture-scope`
entries, and SAP-inapplicable rows, from the violation list. Those scopes still
appear in `compute_sap_metrics` so callers can inspect raw measurements without
turning harness structure into product-architecture alerts.

## Known open questions

- how aggressively to treat Scala traits and Go embeddings as `IMPLEMENTS`
- whether package identity should prefer language-native namespaces over filesystem boundaries in every language
- whether community-level SAP should be exposed as a separate command or just a filter mode

## Design history

This section preserves decisions made during initial design that differ from alternatives considered.

### Why `CALLS` and `REFERENCES` are excluded

An early design (`docs/plans/SAP-METRICS.md`) included `CALLS` and `REFERENCES` as default dependency edges alongside `IMPORTS_FROM` and `INHERITS`. This was revised before implementation because:

- `CALLS` in dynamic languages (Python `len()`, JavaScript prototype calls) produces cross-package noise that inflates `Ce` without representing real coupling.
- `REFERENCES` in Terraform and Markdown is a structural artifact of how those formats express dependency, not a coupling signal between logical packages.

The shipped implementation uses only `IMPORTS_FROM`, `DEPENDS_ON`, `INHERITS`, and `IMPLEMENTS`. Callers choose code, documentation, or legacy mixed-graph analysis with `artifact_scope`.

### `INHERITS` vs `IMPLEMENTS` split

Early extraction folded most type relationships into `INHERITS`. The shipped model distinguishes:

- `INHERITS` — class to base class, subtype to abstract base
- `IMPLEMENTS` — class to interface, concrete type to protocol/trait/contract

The distinction is preserved in `edge.extra` via `relationship_role` and `syntax_source` fields.
