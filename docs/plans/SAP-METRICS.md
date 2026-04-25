# SAP metric design for dagayn

> **Status:** Implemented. See `dagayn/sap.py` and `dagayn/tools/sap_tools.py`.

## Goal

Add support for evaluating the Stable Abstractions Principle (SAP) in dagayn by defining:

1. the aggregation unit
2. abstract type normalization
3. `INHERITS` vs `IMPLEMENTS` semantics
4. `A`, `I`, and `D` aggregation

## Design summary

### 1. Aggregation unit

Use a logical `package` scope as the default SAP unit.

Supported scope kinds:

- `file`
- `package`
- `directory`
- `community`

Recommended package resolution:

- **Python:** nearest importable package root, else parent directory
- **Java/Kotlin/Scala/C#:** declared package or namespace when available, else directory
- **TypeScript/JavaScript/Go/Rust/Swift:** repo-relative directory, optionally cut at workspace markers
- **mixed-language fallback:** normalized repo-relative directory

Every unit should expose:

- `scope_kind`
- `scope_key`
- `display_name`
- `member_files`
- `member_nodes`
- `language_mix`

### 2. Abstract type normalization

Keep the schema stable initially and normalize abstraction data into `nodes.extra`.

Recommended fields:

- `extra.type_role`
- `extra.is_abstract`
- `extra.is_contract`
- `extra.abstraction_source`

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

Language mapping highlights:

- Java/C#/PHP interfaces -> `interface`
- Swift `protocol_declaration` -> `protocol`
- Scala traits -> `trait`
- Julia `abstract_definition` -> `abstract_type`
- Dart abstract classes -> `abstract_class`
- Python abstract bases -> inferred from `ABC`, `ABCMeta`, and abstract decorators
- Go interface declarations -> `interface`

For `A`:

- `Na` = abstract or contract-like type count
- `Nt` = eligible top-level type declarations

### 3. Split `INHERITS` and `IMPLEMENTS`

Current extraction folds most type relationships into `INHERITS`.

Recommended semantics:

- `INHERITS`
  - class to base class
  - subtype to abstract base
  - nominal type-to-type inheritance
- `IMPLEMENTS`
  - class to interface
  - concrete type to protocol/trait/contract
  - conformance-style relationships

Store exact syntax in `edge.extra`:

- `relationship_role`
- `syntax_source`

Migration strategy:

1. parser emits `IMPLEMENTS` where the source language distinguishes it cleanly
2. downstream consumers treating type dependency broadly accept both edge kinds
3. visualization and tooling expose the distinction explicitly

### 4. Aggregate `A`, `I`, and `D`

Start with an in-memory analysis over `get_all_nodes()` and `get_all_edges()`.

Recommended API:

- `compute_sap_metrics(store, scope_kind="package", include_edge_kinds=None, unit_filter=None)`
- tool wrapper in `dagayn.tools.analysis_tools`

Dependency folding:

- map every node to a scope
- collapse node-level dependencies to unique scope-to-scope dependencies
- count unique couplings instead of raw edge totals

Recommended default dependency edges:

- `IMPORTS_FROM`
- `CALLS`
- `REFERENCES`
- `INHERITS`
- `IMPLEMENTS`

Metrics:

- `Ce` = outgoing dependent scope count
- `Ca` = incoming dependent scope count
- `I = Ce / (Ca + Ce)`
- `A = Na / Nt`
- `D = abs(A + I - 1.0)`

Recommended output fields:

- `scope_key`
- `scope_kind`
- `na`, `nt`, `ca`, `ce`
- `abstractness`
- `instability`
- `distance`
- `member_count`
- `top_incoming_dependencies`
- `top_outgoing_dependencies`

## Implementation notes

- no DB migration is required for the first version
- repo-root-relative scope keys should match dagayn path semantics
- community-level SAP is useful as an alternate lens, not the primary default
