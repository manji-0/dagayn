# Cross-artifact edges specification (WIP)

<!-- constrained-by ./SCHEMA.md -->
<!-- constrained-by ./ARCHITECTURE.md -->

> **Naming note:** this spec was previously titled "Cross-language edges". It
> has been broadened to **cross-artifact** because the same edge model also
> covers bridges to non-code artifacts — Markdown specifications referencing
> code symbols, Terraform resources wiring runtime entrypoints to application
> code, schema files generating client packages, etc. "Cross-language" remains
> one important bridge family within this umbrella, but it is no longer the
> only one. The edge kind has been renamed `CROSS_LANGUAGE` → `CROSS_ARTIFACT`.
>
> **Status:** Phase 1 (schema/storage) + Phase 2 Layer-1 extractors + Phase 3 Layer-2 manifest/codegen bridges are implemented for **five bridge families**:
> 1. Cross-language process/FFI bridges (multi-language, syntax-local)
> 2. Markdown → code symbol references (doc-to-code, two-phase: parse + postprocess resolve)
> 3. Explicit documentation bridge directives in Markdown, Python comments, and Terraform comments
> 4. Terraform → application code bridges (high-confidence path and entrypoint attributes)
> 5. Manifest-backed native-extension and generated-client bridges (maturin/PyO3, OpenAPI Generator)
>
> **Phase 4 analysis integration is implemented** for impact radius, flows, review
> guidance, communities weighting, and architecture proximity. Remaining WIP is
> primarily broader cloud-provider attribute coverage.
>
> **What is implemented:**
>
> ### Bridge family 1 — Cross-language process/FFI
>
> | Language     | Bridge patterns |
> |--------------|-----------------|
> | `python`     | `subprocess.{run,Popen,call,check_call,check_output}`, `os.system`, `os.popen`, `os.exec*`, `os.spawn*`, `ctypes.{CDLL,WinDLL,PyDLL}`, `ctypes.cdll.LoadLibrary`, `cffi.FFI().dlopen` |
> | `javascript` | `child_process.{exec,execSync,execFile,execFileSync,spawn,spawnSync,fork}` |
> | `typescript` | (alias of `javascript`) |
> | `java`       | `Runtime.getRuntime().exec`, `Runtime.exec`, `Runtime.getRuntime().{loadLibrary,load}`, `System.{loadLibrary,load}` |
> | `r`          | `system`, `system2`, `.Call`, `.External`, `dyn.load`, `library.dynam` |
> | `bash`       | deferred — every command is a process invocation; needs a distinct model |
>
> - `CROSS_ARTIFACT` edge kind with the full `extra` metadata contract — `BridgePattern` in `dagayn/parser/_base/types.py`; extractors live in the Rust language parsers (formerly documented as `dagayn/parser/bridges.py`)
> - 26 tests covering Python, JavaScript, TypeScript, Java, and R (`TestCrossArtifactEdges` in `tests/test_parser.py`)
> - Confidence `HIGH` (0.8) for string-literal targets, `LOW` (0.2) for dynamic expressions
> - **Limitation:** only canonical dotted forms detected; aliased imports require dataflow resolution (deferred)
>
> ### Bridge family 2 — Markdown → code symbol references
>
> - `_extract_markdown_code_spans` (`dagayn/parser/languages/markdown.py` / Rust `markdown.rs`) scans inline backtick spans, filters by identifier-shape regex, emits `CROSS_ARTIFACT` edges with `relationship_role=describes_symbol`, `bridge_kind=documentation`, `evidence_kind=markdown_code_span`
> - Source = the deepest enclosing Markdown section (or File node when no section precedes the span)
> - Parser phase emits unresolved candidates (`target=<unresolved:{name}>`, `confidence_tier=LOW`, `extra.original_symbol_name=<raw symbol>`)
> - `_resolve_markdown_artifact_refs` (`dagayn/postprocessing.py`) runs on every postprocess call (full build and incremental alike). For each Markdown code-span CROSS_ARTIFACT edge carrying `original_symbol_name`, it consults the current nodes table and keeps the edge only when it resolves to a unique non-Markdown qualified name (confidence HIGH 0.8). Unmatched or ambiguous code-span candidates are deleted so general prose vocabulary does not enter graph data or analysis summaries as unresolved references.
> - Parser tests + resolver tests (`TestMarkdownArtifactResolver` in `tests/test_postprocessing.py`) + idempotence integration tests (`tests/test_cross_artifact_idempotence.py`)
> - **Limitation:** fenced code blocks not processed (too noisy for v1); low-intent code → doc inference is persisted only when it resolves uniquely, otherwise use explicit directives for durable dependencies
>
> ### Bridge family 3 — explicit documentation directives
>
> - Markdown comments such as `<!-- dagayn: implemented-by services/auth.py::refresh_token -->` emit high-confidence `CROSS_ARTIFACT` edges from the enclosing Markdown section.
> - Python and Terraform line comments such as `# dagayn: implements docs/auth-spec.md#Token Refresh` emit `CROSS_ARTIFACT` edges from the nearest enclosing or following implementation node.
> - Supported roles include `implemented_by`, `implements_contract`, `explained_by`, `has_runbook`, `problem_described_by`, `discussed_by`, `discusses_artifact`, and `raises_issue_for`.
> - `query_graph` exposes `docs_for` and `implementations_of` patterns so agents can follow inverse labels without materializing duplicate inverse edges.
>
> ### Bridge family 4 — Terraform → application code
>
> <!-- derived-from #extraction-strategy -->
>
> High-confidence Layer-1/2 extractors in `crates/dagayn-parser/src/terraform_bridges.rs` (wired from `terraform.rs`):
>
> | Pattern | Evidence | Role / bridge_kind |
> |---------|----------|--------------------|
> | `provisioner "local-exec"` `command` with a concrete script/path token | `evidence_kind=syntax`, `evidence_source=provisioner.local-exec.command` | `invokes_binary` / `subprocess` |
> | `filename` on Lambda/function-style resources (local path only; rejects `s3://` / `http(s)://` / `gs://`) | `evidence_kind=config`, `evidence_source=filename` | `maps_entrypoint` / `manifest_link` |
> | `source_dir` / `source_file` / `source_directory` (e.g. `archive_file`, GCP Cloud Functions) | `evidence_kind=config` | `maps_entrypoint` / `manifest_link` |
> | Explicit `handler` / `entry_point` attributes | `evidence_kind=config`; unresolved until postprocess | `maps_entrypoint` / `manifest_link` |
>
> - Paths normalize `${path.module}` / `${path.root}` relative to the `.tf` file; remaining interpolations are rejected (not high-confidence).
> - `_resolve_terraform_artifact_refs` resolves `handler = "module.attr"` to a unique Function/Test whose file stem matches `module`.
> - `query_graph` pattern `bridges_from` follows high-confidence infra→code `CROSS_ARTIFACT` edges (docs_for-style).
> - Fixtures: `tests/fixtures/terraform_cross_artifact/`; tests in `tests/test_terraform.py`, `tests/test_postprocessing.py`, and Rust `extracts_terraform_code_bridges`.
> - **Out of scope for this family:** full cloud-provider attribute matrices; Layer-3 naming-only heuristics.

> ### Bridge family 5 — Manifest-backed / generated-code bridges (Phase 3 / Layer 2)
>
> - `_apply_manifest_bridges` (`dagayn/postprocessing.py`) + `dagayn/parser/manifest_bridges.py` scan the repo root on every postprocess call
> - **Maturin / PyO3:** `[tool.maturin]` with explicit `manifest-path` → `pyproject.toml` → `Cargo.toml` edge (`relationship_role=builds_artifact`, `bridge_kind=extension_module`, `evidence_kind=manifest`, confidence `EXACT`). Default adjacent `Cargo.toml` without `manifest-path` is `HIGH` only when `[tool.maturin]` is present.
> - **OpenAPI Generator:** `openapitools.json` `inputSpec`/`output` → schema → generated package (`generates_code`). `package.json` dependency on that generated package name → consumer → package (`binds_generated_client`). Exact CLI `-i`/`-o` paths in `openapi-generator-cli generate` scripts are also accepted.
> - Confidence tiers for this family: `EXACT` for explicit manifest fields/paths; `HIGH` for maturin default layout; Layer-3 naming-only heuristics are intentionally **not** emitted
> - Fixtures under `tests/fixtures/cross_artifact_manifest/` (Python↔Rust maturin + OpenAPI schema→package→consumer + negative controls); tests in `tests/test_manifest_bridges.py`
> - **Limitation:** does not invent bridges from package-name similarity alone; protobuf/`buf.gen.yaml` and setuptools-rust are deferred

>
> Edges surface automatically in graph stats (`edges_by_kind`) and `query_graph` without additional code.

## Purpose

dagayn already builds useful per-language graphs for polyglot repositories.

What is still under-modeled is the **bridge between artifacts** — relationships
that cross language boundaries, but also boundaries between code and other
artifact kinds (specifications, infrastructure-as-code, schemas, generated
output):

- Python calling Rust
- TypeScript calling a Go or Java backend through generated clients
- Terraform wiring runtime entrypoints to application code
- Markdown specifications describing code symbols
- build metadata connecting Python packages to native extensions
- generated code linking schema sources to implementation code

This specification defines how dagayn should represent those bridges so
graph construction and higher-order analysis still make sense even when the
repository's architecture crosses language and artifact boundaries.

## Goal

The goal is not just to parse more file types.

The goal is to make these analyses remain structurally useful in mixed-language repos:

- impact radius
- traversal
- review context
- communities
- flows
- architecture and hotspot analysis

## Non-goals

This WIP spec does **not** require dagayn to perfectly infer runtime behavior.

It also does not attempt to:

- replace language-specific parsers with a universal semantic engine
- prove every indirect dynamic dispatch path
- resolve every network-level or deployment-time relationship
- treat every external service boundary as a concrete code edge

The target is **useful, review-grade structural accuracy**, not full program verification.

## Problem model

Mixed-language repositories often fail graph tools in one of two ways:

1. each language is parsed correctly, but there are no edges between them
2. cross-language edges exist, but they are mixed with ordinary `CALLS` or `IMPORTS_FROM` edges and lose their meaning

dagayn should solve both problems by:

- representing cross-language relationships explicitly
- preserving the source of evidence for each inferred bridge
- letting downstream analysis decide how strongly to trust each edge

## Recommended edge model

### Core recommendation

dagayn should introduce a general cross-language edge kind:

- `CROSS_ARTIFACT`

The specific meaning should live in `edge.extra`, not in a large explosion of top-level edge kinds.

Recommended metadata:

- `relationship_role`
- `bridge_kind`
- `evidence_kind`
- `evidence_source`
- `source_language`
- `target_language`
- `confidence`
- `confidence_tier`

This keeps the schema stable while still allowing precise downstream behavior.

### Why a single top-level edge kind

A single `CROSS_ARTIFACT` edge is preferred because:

- the number of bridge patterns will grow over time
- many bridge patterns are similar structurally but differ in evidence source
- analysis tools often want to filter “all cross-language relationships” first
- schema churn stays lower than with many specialized edge kinds

Downstream tools can still branch on `relationship_role` and `bridge_kind`.

## Proposed metadata contract

### `relationship_role`

The semantic role of the bridge.

Recommended values:

- `invokes_binary`
- `loads_native_module`
- `loads_shared_library`
- `implemented_by`
- `implements_contract`
- `describes_symbol`
- `explained_by`
- `discusses_artifact`
- `discussed_by`
- `raises_issue_for`
- `problem_described_by`
- `has_runbook`
- `binds_generated_client`
- `binds_generated_server`
- `builds_artifact`
- `generates_code`
- `maps_entrypoint`
- `references_schema`
- `references_contract`
- `wraps_foreign_api`

### `bridge_kind`

The technical mechanism used by the bridge.

Recommended values:

- `subprocess`
- `ffi`
- `extension_module`
- `build_config`
- `generated_code`
- `schema_contract`
- `manifest_link`
- `convention`
- `annotation`

### `evidence_kind`

How dagayn inferred the edge.

Recommended values:

- `syntax`
- `string_literal`
- `manifest`
- `config`
- `filesystem`
- `naming_convention`
- `annotation`
- `comment_directive`
- `generated_artifact`

### `confidence_tier`

Cross-language edges should follow the same general confidence model as other edges.

Recommended tiers:

- `EXACT`
- `HIGH`
- `MEDIUM`
- `LOW`

Examples:

- an explicit `ctypes.CDLL("./target/release/libfoo.so")` match is `HIGH` or `EXACT`
- a `subprocess.run(["foo-cli"])` inferred to a local Cargo binary by name is `MEDIUM`
- an explicit `tool.maturin.manifest-path` or `openapitools.json` `inputSpec`/`output` pair is `EXACT`
- a `[tool.maturin]` block with the default adjacent `Cargo.toml` (no `manifest-path`) is `HIGH`
- a manifest-level guessed mapping by package naming convention alone is `LOW` and is **not** emitted by the Phase 3 extractor

## Documentation bridge semantics

<!-- derived-from #recommended-edge-model -->
<!-- derived-from #proposed-metadata-contract -->

Documentation bridges need more precision than a single `documented_by` role.
Two relationships look similar in graph traversal, but they carry different
ownership and review semantics:

1. a specification, design note, ADR, or task document defines intent that is
   implemented by code, Terraform, or another artifact
2. a document explains, operates, audits, or raises a problem about an existing
   implementation

`documented_by` should therefore be treated as a query/display alias, not as a
stored `relationship_role`.  The stored role should preserve who owns the
relationship and why a reviewer should follow it.

### Contract-to-implementation links

When a document owns the intent and implementation artifacts realize that
intent, prefer a document-authored edge:

- source: the Markdown section that defines the contract
- target: the concrete function, class, Terraform block, resource, module, or
  file that realizes it
- `relationship_role = "implemented_by"`
- `bridge_kind = "documentation"`
- `evidence_kind = "markdown_directive"` or `markdown_code_span`

Example future Markdown directive:

```markdown
## Token refresh

<!-- dagayn: implemented-by services/auth.py::refresh_token -->
<!-- dagayn: implemented-by infra/main.tf::resource.aws_lambda_function.auth -->
```

This means that a change to `docs/auth-spec.md::token-refresh` should pull the
listed implementation nodes into the review surface as possible work to update.

Sometimes the implementation is the better authoring site because the code is
the only stable place where the obligation is visible.  In that case, prefer a
code-authored inverse edge rather than materializing both directions:

```python
# dagayn: implements docs/auth-spec.md#token-refresh
def refresh_token(...):
    ...
```

```hcl
# dagayn: implements ../docs/auth-spec.md#token-refresh
resource "aws_lambda_function" "auth" {
  ...
}
```

The stored edge is then:

- source: the nearest enclosing implementation node
- target: the Markdown file or section
- `relationship_role = "implements_contract"`
- `bridge_kind = "documentation"`
- `evidence_kind = "comment_directive"`

Query surfaces may present `implements_contract` as the inverse of
`implemented_by`, but the graph should not create a duplicate inverse edge by
default.  Keeping only the authored edge avoids stale paired edges during
incremental updates.

### Implementation-to-context links

When the implementation owns the pointer to explanatory, operational, or problem
context, prefer a code- or Terraform-authored edge:

```python
# dagayn: explained-by docs/auth-runbook.md#refresh-token-failures
# dagayn: problem-described-by docs/audits/auth-refresh.md#stale-cache-window
def refresh_token(...):
    ...
```

```hcl
# dagayn: has-runbook ../docs/infra-runbook.md#graph-store-bucket
resource "aws_s3_bucket" "graph_store" {
  ...
}
```

Recommended stored roles:

- `explained_by` — background, rationale, or behavioral explanation
- `has_runbook` — operational procedure for the implementation
- `problem_described_by` — audit, incident, known issue, or problem statement
- `discussed_by` — weaker catch-all for notes that do not fit the above

These edges should pull documents into the review surface when the source
implementation changes, because the document may become stale even when no
contract is violated.

### Document-authored context links

Some explanation or problem documents are naturally authored from the document
side.  For those, use document-to-artifact roles instead of pretending the code
owns the pointer:

```markdown
## Stale cache window

<!-- dagayn: raises-issue-for services/auth.py::refresh_token -->
```

Recommended stored roles:

- `describes_symbol` — low-intent symbol mention, currently emitted from inline
  Markdown code spans
- `discusses_artifact` — explicit prose discussion of an artifact
- `raises_issue_for` — explicit problem statement about an artifact

`describes_symbol` should remain broad and low-confidence unless postprocessing
resolves it uniquely.  Higher-intent relations such as `implemented_by`,
`discusses_artifact`, and `raises_issue_for` should use explicit directives so
ordinary backticks do not create review obligations accidentally.

### Direction and inverse policy

The source node should be the artifact that owns the authored assertion:

- contract document owns intent -> document to implementation
- implementation declares conformance -> implementation to contract document
- implementation points to explanatory context -> implementation to document
- explanation or issue document owns the discussion -> document to implementation

Do not materialize inverse edges by default.  Instead, query tools should expose
inverse labels:

| Stored role | Natural inverse label |
|-------------|-----------------------|
| `implemented_by` | `implements_contract` |
| `implements_contract` | `implemented_by` |
| `explained_by` | `explains` |
| `has_runbook` | `runbook_for` |
| `problem_described_by` | `describes_problem_in` |
| `discussed_by` | `discusses` |
| `discusses_artifact` | `discussed_by` |
| `raises_issue_for` | `has_issue_note` |

This keeps storage idempotent while still letting agents ask both questions:
"what implements this spec?" and "which specs or docs should I read before
changing this implementation?"

## Node targets

Cross-language edges may target existing graph nodes or new synthetic nodes.

### Prefer existing nodes when possible

If dagayn can map the bridge to a concrete file, function, module, class, or artifact already represented in the graph, it should do so.

Examples:

- Python wrapper function -> Rust crate binary entry file
- Python import site -> Rust extension module source crate
- OpenAPI spec -> generated client package

### Use synthetic bridge nodes when necessary

Some bridges point to artifacts rather than ordinary source symbols.

dagayn should allow synthetic nodes such as:

- `Artifact`
- `Binary`
- `Library`
- `Schema`
- `GeneratedPackage`

These may be introduced either as new node kinds later or initially as regular nodes with a discriminating `extra.node_role`.

Recommended minimal metadata:

- `extra.node_role`
- `extra.origin_file`
- `extra.generated`
- `extra.language_runtime`

## Evidence sources

Cross-language edges should be extracted from multiple evidence layers.

### 1. Source-code syntax

Examples:

- Python `subprocess.run(...)`
- Python `ctypes.CDLL(...)`
- Python `cffi.FFI().dlopen(...)`
- Rust `include_str!` / `include_bytes!` references to external assets
- TypeScript imports of generated SDK packages
- shell scripts invoking binaries built elsewhere in the repo

This is usually the highest-signal input.

### 2. Build and package manifests

Examples:

- `Cargo.toml`
- `pyproject.toml`
- `package.json`
- `go.mod`
- `buf.yaml`
- OpenAPI / protobuf generator config
- `maturin` / `setuptools-rust` / `PyO3` configuration

These are especially important when the language bridge is build-time rather than syntax-local.

### 3. Generated-code layout

Examples:

- `generated/` clients
- protobuf output directories
- bindings folders
- checked-in generated SDKs

These often let dagayn connect schema or generator definitions to consumer code.

### 4. Explicit annotations or directives

dagayn should support structured hints when inference is hard.

Examples:

- source comments
- Markdown directives
- sidecar config files
- future dagayn-specific cross-language directives

These should be treated as first-class evidence, not hacks.

### 5. Filesystem and naming conventions

Examples:

- `python/pkg/_native.*.so` matching a Rust crate name
- `target/release/foo` matching a wrapper that invokes `foo`
- shared basename or declared package name matches

This evidence is weaker and should not be treated as exact without supporting signals.

## Extraction strategy

dagayn should extract cross-language edges in layers, from highest confidence to lowest.

### Layer 1: exact syntax-local bridges

Examples:

- `subprocess.run(["./target/release/dagayn-core", "build"])`
- `ctypes.CDLL(str(repo_root / "target/release/libcore.so"))`
- `import dagayn_core` where build metadata explicitly binds that module to a Rust crate

### Layer 2: manifest-backed bridges

Examples:

- `pyproject.toml` declaring a native extension backed by a Rust crate
- a Cargo binary target referenced by Python wrapper config
- protobuf/OpenAPI generator config producing a package consumed elsewhere

### Layer 3: convention-backed bridges

Examples:

- a Python wrapper calling `my-tool` and a local Rust crate exporting `[[bin]] name = "my-tool"`
- generated client directory naming aligned with a schema package

These edges should still be stored, but with lower confidence.

## Recommended initial bridge families

### A. Process boundary bridges

For repositories where one language launches another as a binary or script.

Examples:

- Python -> Rust CLI
- shell -> Python
- Node -> Go service binary

Recommended role:

- `relationship_role = "invokes_binary"`

### B. Native library and extension bridges

For repositories where one language loads another through native bindings.

Examples:

- Python -> Rust via PyO3 or maturin
- Python -> C/C++/Rust via `ctypes` / `cffi`
- Ruby / Node / PHP native extension modules

Recommended role:

- `relationship_role = "loads_native_module"` or `loads_shared_library`

### C. Build-time bridges

For repositories where manifests connect source in one language to runtime artifacts used by another.

Examples:

- `pyproject.toml` referencing a Rust extension build
- `Cargo.toml` exposing `cdylib` or binary targets consumed elsewhere
- code generation pipelines

Recommended role:

- `relationship_role = "builds_artifact"`

### D. Schema and generated-code bridges

For repositories where language boundaries are connected through contracts.

Examples:

- OpenAPI -> generated TypeScript/Python client
- protobuf -> generated Go/Rust/Python code
- SQL schema -> generated ORM layer

Recommended role:

- `relationship_role = "generates_code"` or `references_schema`

## Analysis behavior

Cross-language edges should affect downstream analysis, but not always in the same way as ordinary edges.

### Impact radius

Impact radius should be able to traverse `CROSS_ARTIFACT` edges by default, but include the bridge type and confidence in the explanation.

This prevents mixed-language repos from looking falsely isolated.

### Communities

Community detection should either:

- include cross-language edges with configurable lower weight, or
- offer an option to include/exclude them

Otherwise the repo may incorrectly split into separate language silos.

### Flows

Flows should treat bridge edges as boundary transitions.

The output should preserve that a step crossed:

- process boundary
- native module boundary
- generated contract boundary

This is more informative than flattening the bridge into an ordinary `CALLS` edge.

### Architecture analysis

Hotspots, bridge nodes, and coupling reports should be able to surface:

- the strongest cross-language bridge hubs
- risky single points of interoperability
- unstable build-time bindings

## Suggested weighting

Default weighting should likely be lower than direct in-language calls.

An example starting point:

- direct `CALLS`: `1.0`
- `IMPORTS_FROM`: `0.5`
- `CROSS_ARTIFACT` with `EXACT`: `0.8`
- `CROSS_ARTIFACT` with `HIGH`: `0.6`
- `CROSS_ARTIFACT` with `MEDIUM`: `0.4`
- `CROSS_ARTIFACT` with `LOW`: `0.2`

These are not canonical values, but they illustrate the intended shape:
cross-language edges should matter, but their certainty and semantic looseness should be visible.

## Rollout plan

### Phase 1: schema and storage

- allow `CROSS_ARTIFACT` edges in the graph model
- standardize the `edge.extra` metadata contract
- expose filters in query surfaces

### Phase 2: exact bridge extraction

Implement the highest-signal bridge extractors first:

- subprocess-launched local binaries
- native module loading
- build manifest links for common Python/Rust patterns

### Phase 3: generated and manifest-driven bridges

- code generation pipelines — **shipped** for OpenAPI Generator (`openapitools.json` + exact `package.json` script paths)
- schema-to-generated-package relationships — **shipped** (`generates_code` + consumer `binds_generated_client`)
- manifest-backed package linking — **shipped** for maturin/PyO3 (`builds_artifact`); broader Cargo/npm/protobuf manifests remain incremental

### Phase 4: analysis integration

- impact radius
- traversal
- communities
- flows
- architecture and hotspot tools

**Implemented:** reportable `CROSS_ARTIFACT` hops expand impact with explainable
`bridge_transitions`; low-confidence bridges surface as missingness/caveats;
flow hydration marks bridge arrivals with `step_kind="bridge"`; review and
architecture guidance recommend `docs_for` / `implementations_of` / bridge
follow-ups; communities weight `CROSS_ARTIFACT` at `0.6`.

### Phase 5: explicit annotation support

Add a documented way for repositories to declare bridges when automatic inference is not enough.

## Acceptance criteria

The cross-language design is useful only if it improves analysis on real mixed-language repositories.

Minimum success criteria:

1. mixed-language fixtures no longer split into disconnected subgraphs when a real bridge exists
2. impact radius can cross supported language boundaries with an explainable path
3. flow output preserves bridge transitions instead of flattening them into ordinary calls
4. false positives stay low enough that cross-language edges remain trustworthy in review workflows
5. edge confidence and evidence are visible for debugging and refinement

## Open questions

- whether `CROSS_ARTIFACT` should stay the only top-level edge kind or whether a few specialized kinds are worth it
- whether bridge nodes should become first-class schema-level node kinds
- how much build metadata should be parsed directly versus handled through pluggable extractors
- how aggressively low-confidence convention-based edges should be emitted by default
- how to express explicit dagayn bridge directives in code or docs without creating a maintenance burden

## Current recommendation

dagayn should adopt a **general cross-language edge model** rather than solving only Python-to-Rust or any other single bridge family.

The best near-term shape is:

1. one top-level edge kind: `CROSS_ARTIFACT`
2. rich `edge.extra` metadata for semantics and evidence
3. extractor rollout starting from exact and manifest-backed bridges
4. downstream analysis that treats bridge edges as first-class but explainable relationships

That gives dagayn a path to meaningful polyglot graph analysis without pretending all cross-language links are ordinary in-language calls.
