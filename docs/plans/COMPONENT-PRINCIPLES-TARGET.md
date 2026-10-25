# Component principles without a layer convention

<!-- constrained-by ../POSITIONING.md#quality-before-breadth -->

## Question

`architecture_analysis_tool` judges dependency direction through SDP
(`unstable_dependency`), with SAP as its evidence, and ADP through
`import_cycles`. Clean Architecture asks more than that: that dependencies
point at abstractions (DIP), that clients depend only on what they use (ISP,
CRP), that what changes together lives together (CCP), and that a unit of
reuse is a unit of release (REP). Which of these can dagayn observe from the
graph, without asking the repository to declare layers, and in what form
should it report them?

Status: plan. Nothing here is built; the measurements below decide what is.

## Stance

- **Principles, not conventions.** A declared layer list (`domain`,
  `application`, `infrastructure`) is one way to implement the Dependency
  Rule, not the rule. dagayn observes what the rule protects (the direction
  of dependencies, the stability and abstractness of what is depended on,
  and the cohesion of components) in code that declares no layers.
- **The unit is the declared package.** A crate, Python package, or npm
  package is a unit of release, so it is the component REP talks about. The
  `units` of `mode="overview"` already map them; directories stand in where
  nothing is declared.
- **Position is evidence; a finding names a dependency.** REP, CCP, and CRP
  pull against each other, and where a component should sit between them
  depends on the project's stage. A score is therefore never a defect on its
  own ([architecture finding kinds](./ARCHITECTURE-TOOL-TARGET.md#finding-kinds)).
  Each unit's position is reported as evidence in the overview. A finding
  names a dependency and the place that makes it, as
  `unstable_dependency` does
  ([stability as a finding](./STABILITY-FINDING-TARGET.md#target-contract)),
  so an empty `findings` still means nothing to act on.

## What the graph can observe

<!-- derived-from #stance -->

| Principle | Signal | Edges | Today | Missing |
|---|---|---|---|---|
| ADP | a cycle between units | `IMPORTS_FROM` | `import_cycles` | — |
| SDP | a unit depending on a less stable one | `IMPORTS_FROM`, `DEPENDS_ON`, `INHERITS`, `IMPLEMENTS` | `unstable_dependency` | — |
| SAP | abstractness of a stable unit | as SDP, plus type roles | evidence on `unstable_dependency` | a correct count for Rust, Go, Swift, Kotlin ([evidence](#sap-counts-no-rust-struct)) |
| DIP | whether a cross-unit dependency targets an abstraction or a concrete symbol | `CALLS`, `REFERENCES`, `INHERITS`, `IMPLEMENTS` | — | cases: 2 cross-unit uses of an abstraction in four repositories; TypeScript workspace imports unresolved ([evidence](#type-positions-are-recorded-workspace-packages-are-not)) |
| ISP | the share of an abstraction's methods each client unit uses; implementers that stub a method | `CALLS` to members, `IMPLEMENTS` | — | as DIP |
| CRP | the share of a unit's public surface each depending unit uses | `CALLS`, `REFERENCES` | — | visibility of Rust functions and TypeScript exports ([evidence](#visibility-is-partly-recorded)) |
| CCP | files that change in the same commits across units | — | — | git history |
| OCP | the same dispatch edited each time behaviour is added | — | — | git history |
| REP | whether a unit has its own version and is consumed whole | manifests, cross-repo use | — | manifest versions in the graph |
| LSP | an override that refuses the call (`unimplemented!()`, `raise NotImplementedError`) | `INHERITS`, `IMPLEMENTS` | — | the body check; otherwise not static |

DIP, ISP, and CRP need symbol-level edges. SDP's edge set leaves `CALLS`
and `REFERENCES` out because they are noisy in dynamic languages
([SAP edge semantics](../SAP-METRICS.md#edge-semantics)); a measure that
uses them states it and is measured with them.

## Evidence (2026-10-25)

<!-- derived-from #what-the-graph-can-observe -->

Taken from the graphs of the four repositories
[OPEN-MEASUREMENTS](./OPEN-MEASUREMENTS.md) used: dagayn (current build),
cfwdon, purecrate-ts, and tstodon (graphs as last built). Units in the
second table are approximated by directory (`crates/<name>/`,
`packages/<name>/`, `apps/<name>/`, otherwise the top directory), not the
declared units.

### SAP counts no Rust struct

`sap_metrics` counts a top-level type toward `Nt` only for the roles
`class`, `abstract_class`, `interface`, `protocol`, `trait`,
`abstract_type`, and `mixin`; `struct` and `enum` count toward neither
`Nt` nor `Na`. This is documented
([which types count](../SAP-METRICS.md#which-types-count-toward-nt-and-na)),
but it makes abstractness meaningless for Rust:

- `crates/dagayn-mcp` holds 2 traits and 8 structs. Its abstractness is
  2 / 2 = 1.0; counting the structs, 2 / 10 = 0.2.
- The other seven crates hold no trait, so `Nt` is 0 and SAP does not apply
  to them at all, though they hold 1 to 113 structs and enums each.

Go struct types, Swift structs, and Kotlin's `data class` are excluded the
same way.

### Abstractions are rare targets

Edges from production code in one declared unit to production code in
another, split by whether the target is an abstraction (a type with role
`trait`, `interface`, `protocol`, `abstract_class`, or `abstract_type`, or
flagged abstract, or one of its members; abstract types under `tests/` are
fixtures and not counted). The first count approximated units by directory;
the second uses the declared units, after rebuilding all four graphs
(rust 23, javascript 11):

| Repository | Production abstractions | To an abstraction (directory / declared) | To a concrete symbol (directory / declared) |
|---|---|---|---|
| dagayn | 2 Rust traits, 5 Python protocols and ABCs | 41 / 2 | 1,614 / 868 |
| cfwdon | 1 Rust trait (`pub(crate)`), 6 TypeScript interfaces | 0 / 0 | 2,953 / 2,944 |
| purecrate-ts | 2 Rust traits, both in the `cli` crate nothing depends on | 0 / 0 | 4,456 / 4,287 |
| tstodon | 213 TypeScript interfaces, all generated Prisma types in the leaf package | 0 / 0 | 1 / 0 |

The directory count split dagayn's one Python package into subdirectories,
which is where its 36 `CALLS` and 3 `INHERITS` to protocols came from. The
2 left are `impl dagayn_mcp::Native for RustTools` and
`impl dagayn_mcp::Backend for PythonBackend` in `dagayn-py`.

### Type positions are recorded, workspace packages are not

<!-- derived-from #abstractions-are-rare-targets -->

Evaluation step 2 asked for 20 cross-unit uses of an abstraction in a type
position per language. The four repositories hold 2 (both in dagayn) and
none in TypeScript, so the code itself does not depend on abstractions
across units. Whether the graph would see such uses was checked on
cross-unit uses of any named type instead:

- **Rust: 21 of 22** cfwdon uses have a symbol-level edge (parameters,
  returns, fields, enum payloads, `&[T]`, nested generics). A two-crate
  repro also gets one for `&dyn Tr`, `Box<dyn Tr>`, `impl Tr` in argument
  and return position, `T: Tr`, and `where T: Tr`. Two gaps: a `const` or
  `static` item's type has no edge (the missed one of the 22), and
  `impl other_crate::Tr for S` in a file with no `use` of that crate leaves
  `IMPLEMENTS` on the bare name, unresolved.
- **TypeScript: 0 of 22** tstodon uses have one, but not for a
  type-position reason. Imports of a workspace package (`@tstodon/domain`)
  are not resolved: the module resolver consults only tsconfig `paths` and
  `baseUrl`, not a package's `name` and `exports`, so all 887 cross-package
  edges (573 `CALLS`, 314 `IMPORTS_FROM`) point at a `@tstodon/*` string
  with no node, and type references to those packages are dropped. In a
  repro, the same 7 type-position uses of an interface all get their edge
  (`IMPLEMENTS`, `INHERITS`, `REFERENCES`) when the package is reached
  through tsconfig `paths`, and none when it is a plain workspace
  dependency. Mapped by name, tstodon has at least 348 cross-package calls
  the graph does not connect.

### Visibility is partly recorded

Rust types carry `pub` in `modifiers` (142 in dagayn); Rust functions carry
no visibility at all. Python's public surface follows from names (a leading
underscore). TypeScript `export` is not in `modifiers`. CRP's denominator,
a unit's public surface, is therefore available for Python and for Rust
types only.

## Target contract (draft)

<!-- constrained-by #stance -->
<!-- derived-from #evidence-2026-10-25 -->

The overview gains, per unit, a `principles` object of evidence; no
threshold turns it into a finding:

> `instability` and `abstractness` (counted over every top-level type,
> structs and enums included); `abstract_dependency_share`, the share of the
> unit's outgoing cross-unit symbol edges that target an abstraction; and,
> per unit it depends on, `surface_used`, the used and total public symbols
> of that unit. Each value names the edge kinds it was counted over, and is
> `null` with a reason where the graph cannot count it.

Finding candidates, each built only if its measurement passes:

- **`concrete_dependency`** (DIP). A dependency on a concrete symbol of
  another unit, where that unit offers an abstraction the symbol
  implements. It names the edge, its place, and the abstraction that was
  bypassed. In review, only the ones the change introduced.
- **`refused_member`** (ISP and LSP). An implementation of an abstraction
  whose method only refuses the call (`unimplemented!()`, `todo!()`,
  `raise NotImplementedError`, `throw new Error("not implemented")`). It
  names the implementation, the abstraction, and the method: the client was
  made to depend on a member it does not support.

CRP has no candidate: a low `surface_used` is what a reused library looks
like from each of its clients, so it stays evidence.

## Deferred: needs git history

<!-- derived-from #what-the-graph-can-observe -->

CCP and OCP are about change, which the graph does not hold. Observing them
would take the commit history of the default branch: for CCP, the pairs of
units that change in the same commits; for OCP, the functions edited in
most commits that add behaviour. That is a new data source with its own
freshness problem, so it is decided after the graph-only measures, not
designed here. REP needs each unit's manifest version and the cross-repo
consumers the registry already knows; it waits with them.

## Evaluation

<!-- constrained-by ../POSITIONING.md#quality-before-breadth -->

Following [OPEN-MEASUREMENTS](./OPEN-MEASUREMENTS.md#verdicts): measured on
the four repositories, judged by hand from source with a file:line reason,
and each measure gets a verdict: keep, cut, or fix-then-keep.

1. **Rebuild** the four graphs with the current dagayn and record the
   extractor versions.
2. **Settle the type-position question.** For one Rust and one TypeScript
   repository, list by hand 20 cross-unit uses of a trait or interface in
   a type position and check whether the graph has an edge for each. If
   fewer than 15 have one, adding those edges comes first and the counts
   above are taken again.
3. **Compute the evidence** with a measurement script over the graph
   databases (no tool change): the `principles` values per unit and the
   candidates for each finding.
4. **Judge.** For each finding candidate, every hit up to 30 per
   repository, as actionable, false, or noise. For each evidence value, the
   five most extreme units per repository: does the value point at
   something an agent would change?
5. **Decide**, per measure:
   - A finding ships when its precision is at least 0.8 on at least 20
     judged hits from at least two repositories, and then gates in
     `eval/architecture_thresholds.yaml` like the other kinds.
   - An evidence value ships when at least half of its extreme units are
     judged actionable.
   - A measure with fewer than 20 observable cases across the four
     repositories is recorded as not observable here and is not built.
   - A measure whose hits all come from one repository waits for a second
     one.

## Order of work

<!-- derived-from #evaluation -->

1. Count every top-level type in SAP: done in
   [#211](https://github.com/manji-0/dagayn/pull/211).
2. Step 2 of the evaluation: done
   ([result](#type-positions-are-recorded-workspace-packages-are-not)).
   DIP (`concrete_dependency`) and the ISP half of `refused_member` have 2
   observable cases in the four repositories, under the 20 the
   [evaluation](#evaluation) requires, so they are not built until a
   repository that depends on abstractions across units is added.
3. Resolve TypeScript workspace package imports. It is not a principles
   measure: without it no cross-package edge exists in a TypeScript
   monorepo, for this plan or for `callers_of`, `units`, and review. Its
   own change, with tstodon's cross-package edges as its check.
4. The measurement script and the judgments for what remains observable:
   corrected SAP, CRP's `surface_used` (Python and Rust types), and the
   LSP half of `refused_member`; recorded in this document.
5. Only then the tool contract, for the measures that passed.
6. CCP, OCP, and REP: decide on git history as a data source.

## Touch points

- Metrics: `crates/dagayn-tools/src/architecture.rs` (`sap_metrics`,
  `is_dependency`), `stability.rs`, `arch_findings.rs`, `arch_tool.rs`.
- Units: `crates/dagayn-tools/src/units.rs`.
- Parsers: type roles and visibility per language (`rust_type_modifiers`
  for Rust).
- Docs: [SAP-METRICS.md](../SAP-METRICS.md),
  [ARCHITECTURE-TOOL-TARGET.md](./ARCHITECTURE-TOOL-TARGET.md),
  [STABILITY-FINDING-TARGET.md](./STABILITY-FINDING-TARGET.md).
- Eval: `eval/run_architecture_eval.py`, `eval/architecture_thresholds.yaml`.
