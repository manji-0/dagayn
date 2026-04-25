# SAP metrics specification (WIP)

> **Status:** Work in progress. This document describes the intended fork behavior and analysis contract for Stable Abstractions Principle metrics. It is not fully implemented yet.

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

Type-dependency edges used by SAP are:

- `INHERITS`
- `IMPLEMENTS`

Interpretation:

- `INHERITS` means nominal inheritance or subtype extension
- `IMPLEMENTS` means interface/protocol/trait conformance

Additional dependency edges that may contribute to instability:

- `IMPORTS_FROM`
- `CALLS`
- `REFERENCES`

The exact edge set should be configurable per query.

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
