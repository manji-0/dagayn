# SAP metrics specification (WIP)

> **Status:** Implemented. CLI commands `sap-metrics` / `detect-sap` and MCP tools `compute_sap_metrics_tool` / `detect_sap_violations_tool` are available. See `dagayn/sap.py`, `dagayn/tools/sap_tools.py`, and `tests/test_sap.py`.

## Purpose

This specification defines how dagayn should measure package-level SAP metrics:

- **A** — abstractness
- **I** — instability
- **D** — distance from the main sequence

The target use case is architectural review for mixed-language repositories, not language-pure textbook examples only.

## Scope model

The default analysis unit is a logical **package**.

`package` means:

- a language-native package or namespace when one is clearly available
- otherwise a repo-root-relative directory boundary

Alternative scope kinds may be supported:

- `file`
- `directory`
- `community`

The chosen scope key must remain **repo-root-relative** so results stay stable across machines and temporary paths.

## Type classification contract

Eligible declarations are normalized into a shared abstraction model.

Each relevant node should expose:

- `extra.type_role`
- `extra.is_abstract`
- `extra.is_contract`

Examples:

- Java/C#/PHP interfaces -> contract-like abstract types
- Swift protocols -> contract-like abstract types
- Scala traits -> contract-like abstract types
- Python ABC-based classes -> abstract classes
- Julia abstract types -> abstract types

## Edge semantics

**Default and only** dependency edges for Ce/Ca (fixed, not configurable):

- `IMPORTS_FROM` — explicit module-level import
- `DEPENDS_ON` — generic dependency (used by Terraform, Markdown, and other non-import languages)
- `INHERITS` — nominal inheritance or subtype extension
- `IMPLEMENTS` — interface/protocol/trait conformance

`CALLS` and `REFERENCES` are excluded because they produce noise in dynamic languages (e.g., calling `len()`) and do not cleanly signal cross-boundary coupling.

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

## Output contract

A SAP result row should include:

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
- `top_incoming_dependencies`
- `top_outgoing_dependencies`

## Known open questions

- how aggressively to treat Scala traits and Go embeddings as `IMPLEMENTS`
- whether package identity should prefer language-native namespaces over filesystem boundaries in every language
- whether community-level SAP should be exposed as a separate command or just a filter mode
