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

The default analysis unit is `scope_kind="package"`.

`package` is the **parent directory** of each node's file, repo-root-relative,
for every language (`file_to_package` in `dagayn/_scope.py`). Files at the
repository root map to the scope key `<root>`. No language-native package or
namespace is read: a Python package root, a Java `package` declaration, a C#
namespace, and a workspace marker such as `Cargo.toml` or `package.json` all
leave the scope key unchanged. A Java file at
`src/main/java/com/acme/billing/Invoice.java` is scoped as
`src/main/java/com/acme/billing`, not `com.acme.billing`.

Supported scope kinds:

- `package` (default) — parent directory, as above
- `directory` — same mapping as `package`
- `file` — the repo-root-relative file path

The scope key stays **repo-root-relative** so results stay stable across machines and temporary paths.

SAP runs with `artifact_scope="code"` by default. Markdown documentation nodes
and Markdown-authored dependency directives are excluded from code SAP counts so
documentation structure does not change code Ca/Ce/instability values. Use
`artifact_scope="docs"` to inspect documentation-only dependencies, or
`artifact_scope="all"` when intentionally comparing against the legacy mixed
graph.

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

### Which types count toward `Nt` and `Na`

`compute_sap_metrics` (`dagayn/sap.py`) counts a node only when all of these
hold:

- `kind == "Class"` (nodes of kind `Type`, such as Python, C#, and Rust type
  aliases, never count)
- it is top-level (`parent_name` is empty), so nested classes do not count
- its `type_role` is one of `class`, `abstract_class`, `interface`,
  `protocol`, `trait`, `abstract_type`, or `mixin`; a missing `type_role`
  is treated as `class`

Such a node adds 1 to `Nt`. It also adds 1 to `Na` when `extra.is_abstract`
or `extra.is_contract` is true, or its role is `abstract_class`, `interface`,
`protocol`, `trait`, or `abstract_type`. A `mixin` therefore counts toward
`Nt` but not `Na` unless it is flagged abstract.

Roles outside that list are excluded from both counts: `enum`, `struct`,
`record`, `alias`, `type_alias`, `typed_dict`, and any other role.

Per language, as the parsers currently emit roles:

| Language | Counts toward `Nt` and `Na` | Counts toward `Nt` only | Excluded |
|----------|-----------------------------|-------------------------|----------|
| Python | `Protocol` subclasses, `ABC`/`ABCMeta` subclasses | other classes | `TypedDict` subclasses, type aliases |
| Java | interfaces, `abstract` classes | other classes | enums, records |
| C# | interfaces, `abstract` classes | other classes | structs, enums, records, `using` aliases |
| Kotlin | none | every class declaration, including `interface`, `abstract class`, and `enum class` | `data class` |
| Scala | traits | other classes | case classes, enums |
| Rust | traits | none | structs, enums, type aliases |
| Go | interface types | other named types (for example `type ID int`) | struct types |
| TypeScript / JavaScript | interfaces | classes | type aliases, enums |
| Swift | protocols | classes, actors, extensions | structs, enums |
| Dart | `abstract` classes | other classes, mixins | enums |
| PHP | interfaces | other classes | none |
| Julia | `abstract type` declared outside a module | modules | `struct`; any declaration inside a module (nested) |
| Ruby, Perl, Lua, GDScript, C/C++ | none | classes | none |

Because structs and enums are excluded, a Rust or Go directory that holds
data types plus a few traits or interfaces can have a small `Nt` and a high
`A`. A directory with only data types has `Nt = 0` and is SAP-inapplicable
(see below).

## Edge semantics

The edge kinds that feed `Ce`/`Ca` come from the `dependency_profile`
parameter (`dagayn/dependency_profiles.py`). The default is `strict_static`:

- `IMPORTS_FROM` — explicit module-level import
- `DEPENDS_ON` — generic dependency (used by Terraform, Markdown, and other non-import languages)
- `INHERITS` — nominal inheritance or subtype extension
- `IMPLEMENTS` — interface/protocol/trait conformance

The other profiles add one edge kind each to that set:

| `dependency_profile` | Edge kinds |
|----------------------|------------|
| `strict_static` (default) | `IMPORTS_FROM`, `DEPENDS_ON`, `INHERITS`, `IMPLEMENTS` |
| `implementation` | `strict_static` + `CALLS` |
| `infra_dataflow` | `strict_static` + `REFERENCES` |
| `artifact_trace` | `strict_static` + `CROSS_ARTIFACT`, counting only reportable bridges (`EXACT`/`HIGH`/`EXTRACTED`, no `<unresolved:` target, not flagged low-confidence) |

Unknown profile names are rejected with an error. Every row reports the
profile it was computed with in `dependency_profile`.

`CALLS` and `REFERENCES` stay out of the default because they produce noise in dynamic languages (e.g., calling `len()`) and do not cleanly signal cross-boundary coupling. Opt into them only when the question is about call or dataflow coupling.

An edge adds a scope-to-scope dependency only when its source is an in-scope
node and its target resolves to a different scope. Edges within one
scope are ignored. `Ce` and `Ca` count distinct scopes, not edges.

Artifact scope is applied before dependency projection. In the default `code`
scope, a Markdown `DEPENDS_ON` edge to a source file is ignored because the
documentation endpoint is outside the analysis scope. In `docs` scope, only
Markdown documentation nodes participate. In `all` scope, code and documentation
are projected together for compatibility with older mixed-graph reports.

### Type-name fallback resolution

`INHERITS`/`IMPLEMENTS` targets are often bare type names (e.g., `EmbeddingProvider`) rather than qualified paths.
Target resolution runs for every dependency edge, in two stages:
1. Try `edge.target` as a qualified name (file-path-prefixed)
2. If not found, try `edge.target` as a bare name — succeeds only when every in-scope node with that name falls in **exactly one** scope
3. If the name spans several scopes, or is not found, the edge is silently dropped

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

If `Nt = 0`, report `A = 0.0` and add the note `no-eligible-types`.

If `Ca + Ce = 0`, report `I = 0.0` and add the note `isolated`.

Scopes with no eligible types or no dependency coupling are SAP-inapplicable
(`sap_applicable=false`). `applicability_reason` is `no-eligible-types` when
`Nt = 0` (checked first), otherwise `isolated` when `Ca + Ce = 0`, otherwise
`applicable`. Their raw `D` value is still available for inspection, but it is
not a main-sequence quality signal, and they never appear in `sap_violations`.
A scope with `Nt = 0` and outgoing dependencies has `A = 0`, `I = 1`, and
`D = 0`; a scope with `Nt = 0` and only incoming dependencies has `D = 1`.
Neither value is meaningful.

A scope enters the result when it holds any in-scope node (a file with only
functions still creates a scope) or when a dependency edge touches it.

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

The default `strict_static` profile uses only `IMPORTS_FROM`, `DEPENDS_ON`, `INHERITS`, and `IMPLEMENTS`. `CALLS`, `REFERENCES`, and reportable `CROSS_ARTIFACT` edges were later added back as opt-in `dependency_profile` values, not defaults. Callers choose code, documentation, or legacy mixed-graph analysis with `artifact_scope`.

### `INHERITS` vs `IMPLEMENTS` split

Early extraction folded most type relationships into `INHERITS`. The shipped model distinguishes:

- `INHERITS` — class to base class, subtype to abstract base
- `IMPLEMENTS` — class to interface, concrete type to protocol/trait/contract

The distinction is preserved in `edge.extra` via `relationship_role` and `syntax_source` fields.
