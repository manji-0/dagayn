# What architecture_analysis_tool is for

## Question

`architecture_analysis_tool` is the architecture entry point the docs, the
`architecture-analysis` skill, and the MCP instructions send agents to. Does
it tell an agent something it could not get by reading the tree, and what
should it be?

Verdict: **today it mostly names the obvious and flags the intended.** Every
mode returns a full page whatever the repository looks like, the units it
measures are directories rather than anything a language declares, and none
of the top results on this repository is something to act on. This note
defines the target contract, the evidence behind it, and the order of work.

Status: shipped. The overview answers with the map and `findings` (see
[Result](#result)); the evidence below describes the output before that
change.

## Target contract

<!-- supersedes ./ANALYSIS-TOOL-STRATEGY.md#architecture-health-analysis -->

> `architecture_analysis_tool(mode="overview")` returns two things: a **map**
> of the units the repository declares (crates, packages, modules) and the
> direction and weight of the dependencies between them, and a list of
> **findings**, structural facts worth acting on, each naming the place to
> change and the edges behind it. A repository with nothing to act on gets
> an empty `findings` list.

Consequences of that sentence:

- **Units are declared, not inferred.** A unit is what a manifest or the
  language defines: a Cargo crate, a Python import package, an npm
  workspace package, a Go module, a Terraform module. Directories are a
  fallback only when no manifest declares anything. Today's "package" is
  the file's parent directory (`file_to_package`,
  `crates/dagayn-tools/src/architecture.rs`), which is why parent/child
  directories show up as cycles and stability violations.
- **Silence is a result.** A finding kind fires only where its fact holds;
  no kind fires everywhere by construction.
- **Findings, not rankings.** Each finding is one checkable claim: kind,
  location (file:line where there is one), evidence edges, and the action.
  Degree and betweenness rankings are not findings.
- **Bounded output.** `minimal` fits in about 3K tokens: the map lists each
  unit once, unit edges are aggregated, findings keep 10 per kind and count
  the rest.
- **Works on the bootstrap graph.** The map and findings use parse-time
  edges only, so the overview answers right after `ensure_graph_tool`
  (`postprocess="minimal"`). Communities and flows are not inputs.
- **Precision before recall.** A finding kind that is wrong more often than
  right on the eval set is not shipped.

## Evidence: the current output

Measured on 2026-10-06 against this repository at `30a5226a`, extracted into
a scratch directory and built with `dagayn build` (full postprocess: 858
files, 16,443 nodes, 143,583 edges, 373 flows, 569 communities). Every mode
was called with defaults (`artifact_scope="code"`,
`dependency_profile="strict_static"`, `granularity="package"`).

| Mode | Output (chars) | Summary | Top results |
|---|---|---|---|
| `overview` | 18,952 | 569 communities, 5 coupled pairs, 51 warnings | see below |
| `communities` | 2,164 | 569 communities | named `tests-store` (1,856 nodes), `src-javascript`, `src-file`, `docs-tool` |
| `hubs` | 5,054 | 10 hubs | `node_text`, `GraphStore`, `ParsedEdge`, `ParsedNode`, `qualify`, `EdgeKind` |
| `bridges` | 4,979 | 10 bridges | `GraphStore`, `parse_file_dispatch`, `parse_file_in_repo`, `PyGraphStore` |
| `knowledge_gaps` | 18,474 | 1,222 gaps | 1,011 isolated nodes, 187 untested hotspots (top: `node_text`) |
| `surprising_connections` | 7,865 | 10 | all ten are Python → Rust PyO3 bindings |
| `adp_violations` | 3,677 | 21 cycles | all top ten consist only of parent/child directories |
| `sdp_violations` | 3,711 | 8 | all eight run between a directory and its own subdirectory |
| `sap_violations` | 3,201 | 7 | `dagayn/contracts` (type definitions only) in the "zone of pain" |
| `get_suggested_questions_tool` | 4,254 | 11 questions | "Is `node_text` adequately tested?", "Is `GraphStore` a critical connector?" |

What the numbers mean:

- **Nothing in the top results is something to act on.** Each top-ten list
  was classified by hand against the source:

  | Mode | Top results | Intended or obvious | Actionable |
  |---|---|---|---|
  | `surprising_connections` | 10 | 10 (PyO3 bindings the design requires) | 0 |
  | `adp_violations` (package) | 10 | 10 (directory nesting) | 0 |
  | `sdp_violations` | 8 | 8 (directory nesting) | 0 |
  | `hubs` / `bridges` | 20 | 20 (the parser's text helper, the store, the dispatcher) | 0 |
  | `knowledge_gaps.untested_hotspots` | 10 | 10 | 0 |

- **Cycles are aggregation artifacts.** The heaviest ADP cycle
  (`dagayn` → `dagayn/cli/commands` → `dagayn/server` → `dagayn/tools`,
  severity 332) closes through one function-local import,
  `dagayn/_cli_launcher.py:68` (`from dagayn.cli.commands import serve`
  inside `_serve`), while the other direction carries over a hundred edges.
  The output reports only the summed `edge_weight`, so the one edge worth
  looking at is not in it. At module level the Python package has no import
  cycle at all (strongly connected components over its 472 internal import
  edges: none).
- **File granularity enumerates permutations.** `granularity="file"`
  reports 2,146 cycles; the top eight are orderings of the same ten files in
  `crates/dagayn-parser/src` (`core.rs`, `js_*.rs`, `util.rs`, `types.rs`).
  Modules of one Rust crate referring to each other is normal: the crate is
  the compilation unit and Cargo already forbids cycles between crates.
- **"Untested" means "no direct `TESTED_BY` edge".** `node_text` (574
  edges) is the top untested hotspot, yet tested functions call it
  directly: it has no `TESTED_BY` edge of its own, but its callers do. Requiring a test among callers up to 4 hops, the rule
  `review_tool`'s `untested_change` uses, leaves 0 of the 100 most
  referenced production nodes untested.
- **Communities do not describe boundaries.** 569 communities over 858
  files; names are two tokens (`tests-store`, `src-javascript`); the largest
  holds 1,856 nodes. The only coupling warning is between two doc
  communities (`docs-tool` ↔ `audits-tool`, 107 of 131 edges are
  `CROSS_ARTIFACT` doc links).
- **`detail_level` does nothing.** `standard` differs from `minimal` by 113
  characters on `overview`, 1 on `sap_metrics`, and 0 on `knowledge_gaps`.
- **The bootstrap graph answers with nothing.** Right after
  `ensure_graph_tool` (minimal postprocess), `overview` reports
  "0 communities" with `status: "ok"`; only `graph_health.reason_codes`
  says `missing_communities`.
- **The map the tool never shows is cheap and informative.** Aggregating
  the same graph's `CALLS` / `IMPORTS_FROM` / `REFERENCES` edges by Cargo
  crate gives about 25 unit edges that read as the system's layering (for
  example `dagayn-tools → dagayn-graph`: 160 calls, 183 references;
  `dagayn-parser → dagayn-grammars`: 51 calls; Python `dagayn` →
  `dagayn-py`: 109 calls, 29 FFI bridges).

The graph also lacks two facts the findings need: Python import edges carry
no marker for function-local or `TYPE_CHECKING` imports (both are stored
with the file as source and no context in `extra`), and Cargo, npm, and Go
manifests are indexed only when a cross-artifact bridge points at them.

## Result

<!-- derived-from #evidence-the-current-output -->

Measured on the same snapshot of this repository after the change, rebuilt
with the new extractors:

| | Before | After |
|---|---|---|
| `overview` output, `minimal` (chars) | 18,952 | 7,205 |
| `overview` output, `standard` (chars) | 19,065 | 9,553 |
| Summary | 569 communities, 5 coupled pairs, 51 warnings | 17 units, 25 dependency pairs, 6 `broken_doc_link` |
| package-level `adp_violations` | 21 | 0 |
| `sdp_violations` | 8 | 0 |
| `sap_violations` | 7 | 2 |

- **The map reads as the layering.** 17 units: 10 Cargo crates, the Python
  package, the VS Code extension's npm package, and 5 top-level directories
  (`tools`, `eval`, `scripts`, `diagrams`, `hooks`). Every edge between two
  crates matches a dependency their `Cargo.toml` declares, and
  `dagayn-parser → dagayn-grammars`, which the graph cannot see (the calls
  sit inside a macro), shows up from the manifest alone.
- **All six findings are real.** Four directives in
  `docs/plans/DAGAYN-FEATURE-INTERFACE-REMEDIATION-PLAN.md` name functions of
  the deleted `dagayn/tools/review.py`, and two (lines 205, 206) name Python
  functions of `dagayn/refactor/suggestions.py` that moved to
  `crates/dagayn-tools/src/suggestions.rs`. The two were not in the earlier
  count because the graph keeps such links at `LOW` instead of dropping them.
  No `import_cycle` or `untested_core`, as expected for a repository whose
  Python modules have no cycle and whose most used code is tested.
- **The cycles and violations were artifacts.** With units in place of
  directories, the package cycles and the SDP violations disappear. Two
  more came from matching an unresolved target by name: `import abc` in
  `dagayn/` resolved to a TypeScript namespace `abc` in a test fixture.
  Imports and standard-library or third-party targets are no longer matched
  by name.
- **The eval:** 15 cases (9 negative, 6 positive); every kind has precision
  and recall 1.00 and every negative case is silent. The same findings come
  out of a graph built with `--skip-postprocess`.

## The map

`overview` returns, for `artifact_scope="code"`:

- `units`: one entry per declared unit with `name`, `kind`
  (`cargo_crate`, `python_package`, `npm_package`, `go_module`,
  `terraform_module`, `directory`), `path`, and counts of files, symbols,
  and tests.
- `unit_edges`: `from`, `to`, and counts per edge kind (`calls`, `imports`,
  `references`, `inherits`, `ffi`), only between distinct units.
- `surface`: per unit, up to 3 symbols referenced most from other units,
  the unit's de facto public API. This replaces `hubs` and `bridges` with a
  claim that has a meaning: "other units use this unit through these".

Unit discovery reads manifests from the repository at analysis time
(`Cargo.toml` `[package]`, `pyproject.toml` / top-level `__init__.py`
import roots, `package.json` with workspaces, `go.mod`, Terraform module
directories), so it needs no new node kinds. Tests, fixtures, and examples
are not units of their own; their counts go to the unit they test.

## Finding kinds

Proposed set, each with the graph fact it rests on; the eval decides which
ship.

| Kind | Fires when | Evidence | Action |
|---|---|---|---|
| `import_cycle` | modules form a strongly connected component in a language where import order matters at load time (Python, JavaScript, TypeScript), counting only module-level runtime imports | the component's import edges; the smallest set of edges whose removal breaks it, each with file:line | break the cycle at the listed import(s) |
| `untested_core` | a production symbol is referenced from at least the repository's p95 count of other files, and no direct test reaches it through its callers up to 4 hops | inbound edges and test edges; same path classification as `untested_change` | write or point to a test |
| `broken_doc_link` | an authored doc directive (`constrained-by`, `derived-from`, `dagayn: discusses-artifact`, …) points inside the repository at a file, section, or symbol that does not exist; file existence is checked on disk, not in the graph, and targets outside the repository are skipped | the directive's source line and the missing target | fix or remove the link |

Each kind keeps 10 findings and counts the rest in `findings_omitted`. On
this repository the expected output is 0 `import_cycle`, 0
`untested_core`, and 4 `broken_doc_link`, measured on the same graph: the
Markdown edges whose target is neither a node nor an indexed file are 6;
4 are real (`docs/plans/DAGAYN-FEATURE-INTERFACE-REMEDIATION-PLAN.md`
lines 119, 120, 151, 152 name functions of the deleted
`dagayn/tools/review.py`), 1 points outside the repository
(`~/.pi/agent/AGENTS.md` from `AGENTS.md`), and 1 names a file that exists
but is not indexed (`prek.toml` from `CONTRIBUTING.md`); the last two are
why the rule checks the disk and skips outside targets.

Considered and not proposed:

- **Cycles between declared units** (crates, Go modules): the build system
  rejects them, so they cannot occur in a repository that builds. Python
  subpackage cycles with acyclic modules are harmless and are what today's
  ADP reports.
- **SDP / SAP as findings**: an instability or abstractness score crossing
  a threshold is not a defect without a stated design intent. The metrics
  stay available as explicit modes, computed over declared units instead
  of directories. Revised (2026-10-08): a dependency on a less stable unit,
  with the import that makes it, is now the finding `unstable_dependency`;
  SAP stays evidence ([STABILITY-FINDING-TARGET.md](./STABILITY-FINDING-TARGET.md#target-contract)).
- **Layer rules** ("`tools` must not depend on `cli`"): useful, but only
  with a rule file the repository authors. Revisit once the map is in use.
- **Dead code**: `refactor_tool(mode="suggest")` already reports removal
  candidates; `knowledge_gaps.isolated_nodes` duplicates it with less
  evidence.

## What happens to the current modes

| Mode | Fate |
|---|---|
| `overview` | returns `units`, `unit_edges`, `surface`, `findings`; the current `architecture_health` block moves behind `detail_level="verbose"` for one release |
| `adp_violations` | replaced by `import_cycle`; removed after 9.0.0, with `dagayn detect-adp` |
| `sdp_metrics`, `sdp_violations`, `sap_metrics`, `sap_violations` | kept as metric modes, units switched to declared units |
| `hubs`, `bridges` | replaced by `surface`; removed after 9.0.0 |
| `knowledge_gaps` | split: `untested_hotspots` → `untested_core`, `isolated_nodes` → `refactor_tool(mode="suggest")`; community gaps dropped; removed after 9.0.0 |
| `surprising_connections` | removed after 9.0.0 |
| `communities`, `community` | kept as an advanced drill-down; no longer part of `overview` |
| `get_suggested_questions_tool` | dropped after one release; its signals are the findings above |

`detail_level` gets a meaning: `minimal` is the map without `surface` plus
findings; `standard` adds `surface` and each finding's full edge list;
`verbose` adds the deprecated blocks.

## Evaluation

A harness beside `eval/run_review_eval.py`, run in CI on fixed fixtures:

- **Negative cases**, each expecting zero findings: modules of one Rust
  crate that refer to each other; a Python package whose subpackages import
  each other with acyclic modules; a cycle that closes only through a
  function-local or `TYPE_CHECKING` import; a TypeScript cycle made only of
  `import type`; a PyO3 extension called from Python; a helper reached by
  tests through two calls; a doc link to a section that exists.
- **Seeded positives**, each expecting exactly its finding: a Python
  module-level import cycle; a TypeScript runtime import cycle; a widely
  used production function no test reaches; a doc directive pointing at a
  removed section and at a renamed symbol.
- **Real repositories**: this repository plus one Python and one
  TypeScript project, with every finding classified by hand.
- **Reported per run:** precision and recall per kind, `minimal` output
  size (p50/max), and the unit list for the real repositories.

## Order of work

1. Python extractor: mark import edges with their context
   (`import_scope`: `module`, `function`, `type_checking`) and TypeScript
   `import type`; bump the extractor versions.
2. Unit discovery from manifests and the `units` / `unit_edges` /
   `surface` map in `overview`.
3. Eval harness (`eval/run_architecture_eval.py`) with the cases above in
   `tests/fixtures/architecture_eval`.
4. `import_cycle`, `untested_core`, `broken_doc_link`, each gated in CI at
   precision 0.8.
5. Move the current blocks behind `verbose`, list them in
   `deprecated_fields`, switch SDP/SAP to declared units, regenerate the MCP
   parity snapshots.
6. Docs, the `architecture-analysis` and `explore-codebase` skills, the MCP
   tool description, and the installed agent instructions describe the map
   and findings.
7. After one release, drop the deprecated modes and fields.

## Touch points

- Code: `crates/dagayn-tools/src/arch_tool.rs`, `architecture.rs`,
  `analysis.rs`, `community.rs`, `questions.rs`;
  `crates/dagayn-parser/src/python/` (import context);
  `crates/dagayn-parser/src/js_modules/` (`import type`).
- Docs: `docs/COMMANDS.md`, `docs/LLM-OPTIMIZED-REFERENCE.md`,
  `docs/USAGE.md`, `docs/plans/ANALYSIS-TOOL-STRATEGY.md`, the MCP tool
  description and server instructions (`dagayn/server/mcp_surface.json`),
  `dagayn/skills/instructions.py`, `skills/architecture-analysis/SKILL.md`,
  `skills/explore-codebase/SKILL.md`.
- Tests: `crates/dagayn-tools/tests/tools.rs` architecture cases,
  `tests/test_architecture_analysis.py`, `tests/test_analysis_tools.py`,
  and `tests/fixtures/parity/__mcp_snapshots__`.
