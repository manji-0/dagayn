# Refactor plan: the files every change goes through

<!-- constrained-by ./SCIP-CALL-RESOLUTION.md#freezing-the-inference-layer -->
<!-- constrained-by ./PURE-RUST-MIGRATION.md#54-remaining-python-surfaces -->

Draft, 2026-11-05. File counts, line numbers, and churn are measured at
`origin/main` 26c59a38. The `refactor_tool` and `architecture_analysis_tool`
results come from the main checkout's graph, which reports
`graph_built_by_older_extractor`, so their line numbers can be a few lines
off. All seven [steps](#steps) are done; see [status](#status).

## Status

<!-- derived-from #steps -->

Done, 2026-11-05. Each step is one PR, merged after the
[verification](#verification) gates passed locally. Every step left the
parity export and the MCP snapshots byte-identical.

| Step | PR | Where it differs from the plan text below |
|---|---|---|
| 1. Normalize `extractor_versions` | #310 | — |
| 2. `CHANGELOG.md merge=union` | #309 | — |
| 3. One post-processing sequence | #311 | `run_passes` in `dagayn-postproc/src/pipeline.rs` is the list. Bare-name call and inheritance resolution are two steps on every path, so a failed call step no longer skips inheritance. On the Python build path a failed pass's warning has Rust's wording (no exception type name) and is no longer logged. |
| 4. Split `tools.rs` | #313 | 80 tests on `main`, not 84. 14 modules: the eight tools plus `common`, `stats`, `docs`, `postprocess`, `reply_contract`, `request_boundary`. The five `request_boundary` directives now name `GraphStore.resolve_request_boundaries`. |
| 5. Split `postprocess/tests.rs` | #312 | Nine modules, one per pass source file. |
| 6. Split `query.rs` | #314 | `query_graph` is 147 lines. 160,326 replies compared before and after; the 3 that differed were `nearest_test` ties that already changed from run to run on `main` (#315). |
| 7. `returned.rs` | #316 | `returned/go.rs` holds only the Go items. The helpers every language uses (`PendingCall`, `pending_calls`, `origin_call`, `mark_external`, `update_call`) and the Rust-only `resolve_field_receivers` stay in `returned/mod.rs`. The table has 22 rows, checked against the old `match` on 14,590,464 inputs. |

Not decided by these steps:

- `dagayn watch` reports a different `postprocess` reply shape from the
  two build paths (`signatures_computed`, and `manifest_bridges_*: 0` when
  nothing was scanned). Making them the same is a reply change, not a
  refactor.
- The [open questions](#open-questions) below.

## Question

A few files take most of the recent edits, and some of those edits repeat
the same change in several places. Which of these cost the most in review
time, merge conflicts, or bugs? What is the smallest change that keeps
behaviour the same and stops the cost?

Each item below must remove code or remove a source of conflicts. An item
that only adds a layer is not included. Every step is a pure move or a pure
deletion: it bumps no `extractor_version`, and no snapshot changes. A step
that needs either one changed the output, so it is not a refactor and
belongs in another PR.

## Current state

### Churn

How many of the most recent commits touched each file. Commits are squash
merges, so one commit is one PR.

| File | last 50 (2026-10-26 to now) | last 150 (2026-10-06 to now) |
|---|---|---|
| `CHANGELOG.md` | 45 | 78 |
| `crates/dagayn-tools/tests/tools.rs` | 21 | 58 |
| `crates/dagayn-parser/src/extractor_version.rs` | 16 | 28 |
| each of the 9 `tests/fixtures/parity/__mcp_snapshots__/*/metadata.json` | 16 | 31 |
| `crates/dagayn-tools/src/query.rs` | 9 | 18 |
| `crates/dagayn-parser/src/go.rs` | 7 | 8 |
| `crates/dagayn-graph/src/postprocess/tests.rs` | 6 | 8 |
| `crates/dagayn-graph/src/postprocess/returned.rs` | 5 | 7 |
| `crates/dagayn-parser/src/rust_lang/mod.rs` | 2 | 8 |

### Size and function length

| File | Lines | Longest functions |
|---|---|---|
| `crates/dagayn-tools/tests/tools.rs` | 5,285 | 84 tests in one file, about 63 lines each |
| `crates/dagayn-graph/src/postprocess/tests.rs` | 4,039 | 49 tests |
| `crates/dagayn-tools/src/query.rs` | 2,387 | `query_graph` 611 (l.1777), `run_pattern` 267 (l.1398) |
| `crates/dagayn-graph/src/postprocess/returned.rs` | 2,143 | `builtin_type_package` 268 (l.725), `resolve_returned_receivers` 211 (l.1806), `load_graph` 174 (l.345) |
| `crates/dagayn-parser/src/go.rs` | 2,245 | `go_emit_call` 97 (l.1088) |
| `crates/dagayn-graph/src/postprocess/bare_names.rs` | 1,046 | `bind_bare_call_targets` 177 (l.429) |
| `crates/dagayn-graph/src/postprocess/external_calls.rs` | 895 | `resolve_enum_variant_calls` 73; most of the file is method tables |

`refactor_tool(mode="suggest")` reports 81 `complex_hotspot` findings. The
top findings are `dagayn/tools/build.py::_run_postprocess` (122 lines, 26
commits in 90 days), `query.rs::query_graph` (611 lines, 24 commits),
`dagayn/tools/build.py::build_or_update_graph` (262 lines, 24 commits), and
`rust_lang/mod.rs::rust_walk_children` (415 lines, 23 commits).
`architecture_analysis_tool` finds no import cycles and no
`unstable_dependency` between the 22 units.

### The same change in several places

- **Post-processing pass order is written in three places.** These are
  `dagayn-postproc/src/pipeline.rs::run_post_processing_json` (reached from
  `dagayn/postprocessing.py::run_post_processing`, which `dagayn watch`
  runs after each update), `dagayn-build/src/postprocess.rs::minimal_steps`
  (the Rust CLI), and `dagayn/tools/build.py::_run_postprocess` (the Python
  build and MCP path). `_run_postprocess` calls one wrapper per pass in
  `dagayn/postprocessing.py`, and each wrapper calls a PyO3 method in
  `dagayn-py/src/lib.rs`. The request-boundary pass (#247) was added to
  all three sequences, to a wrapper, and to a PyO3 method.
  The watch path already runs in a different order. `minimal_steps` and
  `_run_postprocess` agree: Terraform module references, then
  `demote_unresolved_endpoint_edges`, then the Markdown and Terraform
  artifact passes. `pipeline.rs` runs the demotion after the two artifact
  passes, just before the manifest bridges.
  `build/postprocess.rs`'s header says "the step order matters ... so it
  follows the Python one call for call", and the watch path does not. The
  demotion updates only `CALLS`, `INHERITS`, `IMPLEMENTS`, `CONTAINS`,
  `REFERENCES`, and `TESTED_BY` edges (`NODE_QUALIFIED_EDGE_KINDS` in
  `postprocess/endpoints.rs`). The Markdown pass updates and deletes only
  `CROSS_ARTIFACT` edges (`search_markdown.rs`). The Terraform artifact pass
  deletes and inserts only `CROSS_ARTIFACT` and `IMPORTS_FROM` edges
  (`terraform_bridges.rs`). So the order does not change the result today,
  but nothing stops the next pass from depending on it. A smaller
  difference: `pipeline.rs` runs bare-name call and inheritance resolution
  as two steps, each with its own warning, and the other two run them as
  one step, so a failure in the call step also skips inheritance.
- **An extractor version bump changes 10 files.** A bump changes
  `extractor_version.rs` and every `metadata.json` snapshot, the Go-free
  `python_only`, `markdown_only`, and `terraform_only` included, because
  `extractor_versions` is one string of all versions. #287 changed 16
  files, and 9 of them were this one line. Several bumps were for
  post-processing fixes, not extractor output. #283's CHANGELOG entry says
  "The Go extractor version is now 17, so the next `dagayn update` re-binds
  the calls of unchanged files". #287 changed only post-processing
  (`returned.rs`) and bumped Go to 18.
- **`CHANGELOG.md` `## Unreleased` changes in 45 of the last 50 PRs.** Two
  PRs open at the same time both add to the top of the same list, so one of
  them always has to rebase.
- **A tool argument is declared three times.** It appears in the
  `dagayn/server/main.py` signature and docstring, in the Python shim
  (`dagayn/tools/query.py::query_graph` passes `offset` through), and in
  Rust. Then `mcp_surface.json` and `tools_list.json` are regenerated. 18
  of the last 150 commits touched `main.py`, Rust tool sources, and
  `mcp_surface.json` together. This comes from the migration being half
  done, so this plan does not fix it (see [non-goals](#non-goals)).

### Bugs and review cost traced to these files

- `returned.rs`: in #287, the first commit added a Go branch to the
  external-origin `if`/`else if` chain in `resolve_returned_receivers`.
  The second commit of the same PR had to undo part of it ("the new Go
  branch ... took such calls for the standard library's, so `callers_of`
  missed them while saying it was complete"). The chain has one branch per
  family (Rust, Go, Python/JavaScript). Each branch pulls
  `external_package`, `stdlib`, and `external_symbol` out of untyped
  `extra` JSON in its own way. Across `crates/`, `get("stdlib")` appears
  39 times and `get("external_symbol")` 18 times.
- `bare_names.rs`: #214 fixed a cross-family name match (Rust
  `impl From` was bound to TypeScript `Field.From`). The language-family
  rule had to be added in three files (`architecture.rs`, `bare_names.rs`,
  `dead_code.rs`).
- `query.rs`: 5 of its last 6 commits (#278, #279, #280, #282, #287) were
  Go `callers_of` fixes. Go has its own functions, `go_unbound_calls`
  (l.1254, 18 lines), `go_call_lines` (l.1277, 10), and `go_callers_note`
  (l.1290, 81), and a Go sentence block inside `query_graph`. The response wording (`guidance`,
  `edge_caveat`, `no_edges_note`, `closed_note`, paging notes) is spread
  through the 611-line function, next to the code that resolves the
  target, walks the graph, and applies the budget.

## Candidates, ranked

Ranked by cost removed per unit of risk. The first two are process items:
the cheapest to make, and they affect the most PRs.

1. **Snapshot `extractor_versions` churn** (16 of the last 50 PRs, 9 files
   each). Normalize the key in `tools/mcp_snapshot.py`. A test that already
   exists (`tests/test_incremental.py::test_full_build_records_extractor_versions`)
   checks that the build records the current versions. Removes about 9 of
   the 16 files per bump.
2. **`CHANGELOG.md` conflicts** (45 of the last 50 PRs). The cheapest fix is
   `.gitattributes` `CHANGELOG.md merge=union`, which does not exist today.
   It helps local rebases only, because GitHub's own mergeability check
   ignores merge drivers. The complete fix is a fragment directory, one
   file per PR, that the release commit assembles. That adds a release step
   ([open question](#open-questions)).
3. **One post-processing sequence** (3 sequences, and the watch path's is in a different order). Deletion
   work: one Rust sequence, and the Python step loop removed.
4. **Split `query.rs` along what it does** (611-line function, 9 of the
   last 50 PRs). Move the wording out of the logic, so a Go wording fix
   touches one small file.
5. **Split the large test files into modules of one test binary**
   (`tools.rs` 21 of the last 50 PRs; `postprocess/tests.rs` 4,039 lines).
   Fewer conflicts between parallel PRs, and a test can be found by the
   tool it tests.
6. **`returned.rs` mechanical splits** (2,143 lines). Ranked lower than its
   size because the layer is frozen for Rust, TypeScript, and Go: new rules
   should not appear, so there will be fewer edits to save. Only moves and
   tables, no new abstraction.

Not candidates on the evidence:

- `go.rs`: its longest function is 97 lines, and its 7 recent edits are
  small and local.
- `rust_lang/mod.rs::rust_walk_children` (415 lines): long, but only 2 of
  the last 50 PRs touched it, and Rust receiver tracking is frozen.
- `external_calls.rs`: mostly tables of names. The data is the content, and
  the Rust and JavaScript tables are frozen.
- `bare_names.rs::bind_bare_call_targets` (177 lines): watch it. Split it
  only if it starts taking one fix per language, as `query.rs` did.

## Steps

<!-- derived-from #candidates-ranked -->

Each step is one PR. It ends in a state that the gates in
[verification](#verification) check. Steps 1–2 and 3–5 are independent, so
they can run in parallel. Step 6 waits for the `query.rs` PR in progress
(the Go `callers_of` summary wording) to merge.

### 1. Normalize `extractor_versions` in the MCP snapshots

- Put `extractor_versions` in `_VOLATILE_KEYS` in `tools/mcp_snapshot.py`
  (l.168), or give it its own placeholder `<EXTRACTOR_VERSIONS>` so the key
  still has to be present. Regenerate the 9 `metadata.json` files once.
- Check first: no snapshot test reads the value. The build-records-versions
  test in `tests/test_incremental.py:786` stays as the check that the
  build writes the versions.
- Result: an extractor bump changes `extractor_version.rs` and the code it
  versions, and no snapshot files.

### 2. Stop `CHANGELOG.md` conflicts in local rebases

- Add `.gitattributes` with `CHANGELOG.md merge=union`. Check it by
  rebasing two branches that both add to `### Fixed`.
- Leave the fragment-directory choice to the owner
  ([open questions](#open-questions)).

### 3. One post-processing sequence in Rust

<!-- constrained-by ./PURE-RUST-MIGRATION.md#54-remaining-python-surfaces -->

1. Write a graph-level parity check first. For each parity fixture, build
   it and run `tools/parity_export.py --stdout`, once before the change and
   once after. Use both entry points: the Rust CLI `dagayn build` /
   `update`, and the Python `build_or_update_graph` path. Save the
   before-output as the baseline.
2. Make `dagayn-postproc::pipeline` the only place that lists the passes,
   in the order `dagayn-build/src/postprocess.rs::minimal_steps` uses now
   (the order the Rust CLI and the Python build already ship). This moves
   the demotion on the watch path to before the two artifact passes. That
   changes no edge, because the demotion and the artifact passes touch
   different edge kinds ([above](#the-same-change-in-several-places)), and
   the step 3.1 export checks it. `dagayn-build` already depends
   on `dagayn-postproc`. `minimal_steps` calls the shared sequence and
   keeps only its own concerns: level metadata, orphan prune, step labels
   for warnings.
3. Point `dagayn/tools/build.py::_run_postprocess` at the single
   `store.run_post_processing_json` call that `dagayn/postprocessing.py::run_post_processing`
   already uses. Delete the per-pass wrappers in `postprocessing.py`
   (`_resolve_bare_name_edges`, ..., `_resolve_request_boundaries`) and the
   PyO3 per-pass methods that only they call. Do not restructure
   `_run_postprocess` further: PURE-RUST-MIGRATION removes it.
4. Result: a new pass is one line in one Rust function. The parity export
   and all snapshots are byte-identical.

The risk is in the reply fields, not the edges. `PostprocessResult`
(`Option<i64>` fields) and the `PostprocessOutcome` labels in
`dagayn-build` may report counters under different keys. Compare the
`postprocess` block of a `build` and an `update` reply before and after,
from the Python path and from the Rust CLI. If the two paths differ,
decide which shape the snapshots pin before 3.2, as a separate, intended
change.

### 4. Split `tools.rs` into modules of one test binary

- Move `crates/dagayn-tools/tests/tools.rs` to `tests/tools/main.rs`, with
  `mod common;` (`Repo`, `answer`, `declines`, `auto_preparing`) and one
  module per tool: `query_graph` (about 80 calls), `review` (34),
  `minimal_context` (32), `refactor` (16), `architecture` (15), `flow`
  (14), `search` (12), `ensure_graph` (9).
- Keep one binary. Every file directly under `tests/` is its own test
  binary that links the whole crate, so eight top-level files would
  multiply link time.
- Fix the five `// dagayn: tests crates/dagayn-graph/src/postprocess/request_boundary.rs::resolve_request_boundaries`
  comments (`tools.rs` l.4886, 4933, 4968, 5000, 5040) that move with the
  tests. The main checkout's graph reports all five as `broken_doc_link`
  (`missing: symbol`). Confirm that on a freshly built graph: the current
  graph comes from an older extractor, and the target is an
  `impl GraphStore` method.
- Check: `cargo test -p dagayn-tools` runs the same 84 tests with the same
  names except for the module prefix. Compare `--list` before and after.

### 5. Split `postprocess/tests.rs` the same way

- Move it to `postprocess/tests/mod.rs` with one module per pass
  (`returned`, `bare_names`, `external_calls`, `scip_overlay`, ...),
  following the parser's `core_tests/<language>.rs` layout.
- Check: the same 49 tests, compared with `--list` before and after.

### 6. Split `query.rs` into `query/`

Waits for the `query.rs` PR in progress. Each sub-step is a pure move, so
that PR or any later wording change rebases onto it without conflicts in
the logic.

- `query/notes.rs`: `guidance`, `edge_caveat`, `no_edges_note`,
  `closed_note`, `go_unbound_calls`, `go_call_lines`, `go_callers_note`,
  and the Go sentence block and paging sentences now inline in
  `query_graph`, as functions that take the computed facts and return
  text.
- `query/resolve.rs`: the exact/respelled/name resolution at the top of
  `query_graph`, `not_found`, `ambiguous`, `Resolution`.
- `query/walk.rs`: `run_pattern`, `expand_imports`, `expand_transitive`,
  `Found`.
- `query/rows.rs`: `node_row`, `edge_dict`, `compact`, `merge`,
  `project_minimal`, and the other row helpers that `search.rs`,
  `refactor.rs`, `findings.rs`, and the other tool modules import as
  `crate::query::...`. Re-export them from `query/mod.rs` so those imports
  stay the same.
- `query/mod.rs::query_graph`: arguments → resolve → walk → notes → page
  → `next`, under about 150 lines.
- No per-language trait. Go is the only language with its own text today.
  A `match` on language in `notes.rs` is enough until a second language
  needs a sentence of its own.

### 7. `returned.rs`: moves and one table

<!-- constrained-by ./SCIP-CALL-RESOLUTION.md#freezing-the-inference-layer -->

1. Move the Go-only section (`go_first_result` l.1155 to
   `resolve_go_declared_receivers`, about 450 lines, with `GoMember` and
   `GO_*` constants) to `postprocess/returned/go.rs`. `go_directory` is
   `pub(crate)`, so re-export it.
2. Turn the 268-line `match` in `builtin_type_package` into a static table
   of `(family, extension guard, names, package)` rows, plus the few
   predicate rows (`"c"` with an `NS`/`UI` prefix). Keep the lookup order
   the same: the first match wins. In the `"jvm"` family, each of Kotlin
   and Scala has a row of names, then a row that returns `None` for any
   other name in a `.kt`/`.kts` or `.scala`/`.sc` file. Both come before
   the Java rows, so a Kotlin or Scala file never gets a Java package.
3. In `resolve_returned_receivers`, read `external_package`, `stdlib`, and
   `external_symbol` out of `inner_extra` once, into a small local struct,
   before the family branches. Then each branch decides only whether the
   origin types the receiver. This changes code layout only: the branches,
   their order, and their conditions stay the same.

None of these adds a rule or a table entry, as the freeze requires.

## Verification

<!-- derived-from #steps -->

Every step must pass all of these. Every step is meant to change nothing,
so any difference anywhere fails the step.

- `uv run ruff check .`, `uv run ruff format --check .`,
  `uv run pyrefly check`, and
  `uv run pytest --tb=short -q -n auto --dist loadfile`.
- `cargo test --workspace`, and clippy with the `rust-toolchain.toml` pin.
- `tests/test_mcp_snapshots.py` against both server parameters
  (`python` and `front_end`). The front end needs the reinstalled `dagayn`,
  because `_core` must be rebuilt with `--reinstall-package`.
- The eval gates that plain pytest skips: `DAGAYN_REVIEW_EVAL=1`,
  `DAGAYN_ARCHITECTURE_EVAL=1`, `DAGAYN_REFACTOR_EVAL=1`,
  `DAGAYN_FLOW_EVAL=1`, and `DAGAYN_TEST_REACH_EVAL=1`. Steps 3 and 7
  change graph code. No Go test-reach thresholds file exists (#293), so
  for Go, rerun the M2 harness (`run_test_reach_eval.py` on a
  `go tool cover -func` listing) on testify before and after.
- Steps 3 and 7: `tools/parity_export.py` before and after on every parity
  fixture, and on this repository's own graph, built in a fresh clone. The
  PostToolUse hook rewrites a local graph with the installed dagayn, so
  read the graph metadata before trusting a local diff.
- No `extractor_version.rs` change in any step's diff.

## Non-goals

- **The Python tool bodies and `server/main.py`.**
  [PURE-RUST-MIGRATION](./PURE-RUST-MIGRATION.md#53-rust-mcp-server)
  deletes them. Restructuring `build_or_update_graph` (262 lines) or
  `session_prepare` (334 lines) would be work thrown away. Step 3 only
  makes `_run_postprocess` smaller.
- **A per-language trait or strategy layer in `returned.rs`,
  `bare_names.rs`, or the parsers.** The inference layer is frozen for
  Rust, TypeScript, and Go, so a new abstraction would serve rules that
  should not be written. The real simplification is to delete the frozen
  passes. SCIP-CALL-RESOLUTION keeps that as an open question (about 132
  `MEDIUM` calls per Rust file would stay unresolved between builds), and
  this plan does not decide it.
- **A typed replacement for the edge `extra` JSON.** It would touch every
  pass and the schema. Step 7 reads the fields once at the place with the
  most repetition.
- **Parser swaps.** The oxc and Ruff decisions are recorded in
  [OXC-PARSER-EVALUATION](./OXC-PARSER-EVALUATION.md) and
  [RUFF-PYTHON-PARSER](./RUFF-PYTHON-PARSER.md).
- **The per-language parser files** (`c_like.rs`, `python/mod.rs`,
  `csharp.rs`, ...). They are large because their languages are large, and
  recent edits to them are small and local.

## Open questions

These are for the owner. The steps above do not depend on the answers.

- **Bug fixes in frozen inference code.** The freeze table marks Go's
  return-type tables `freeze`, and the freeze text says "Frozen means no
  new table entries or rules; bugs that SCIP exposes ... are still fixed".
  #287 added a Go branch to `resolve_returned_receivers` in `returned.rs`
  as a bug fix. Are fixes of that kind allowed in frozen code? If they
  are, should the freeze table say so for Go? If not, should they wait for
  the SCIP overlay? Step 7 does not depend on the answer.
- **A separate version for post-processing.** #283 and #287 bumped the Go
  extractor so that unchanged files would re-bind. That re-parses every Go
  file, though only the post-processing passes needed to run again. A
  version of its own for post-processing (for example a
  `postprocess_versions` metadata key that triggers a full post-process)
  would rerun only post-processing. It changes the metadata format and the
  update path, so it is a behaviour change and needs its own plan.
- **CHANGELOG: `merge=union` or fragments.** `.gitattributes`
  `CHANGELOG.md merge=union` (step 2) helps local rebases only. One
  changelog fragment per PR, assembled by the release commit, also removes
  the conflict on GitHub. But it adds a step to the release procedure
  (release commit, then green CI, then signed tag).
