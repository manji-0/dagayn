# TypeScript, Python, and Go at Rust's level

<!-- constrained-by ../POSITIONING.md#quality-before-breadth -->
<!-- constrained-by ../POSITIONING.md#language-breadth-stays -->
<!-- derived-from ../POSITIONING.md#evidence -->

## Question

Rust is the language where dagayn measurably helps an agent. What would it
take for TypeScript (with JavaScript), Python, and Go to get the same level
of support, and in what order should it be built?

Status: plan only. No capability below is built. Every work item ships only
if it moves a number on a real repository, and is recorded as not shipped
otherwise, the way [dispatch-aware reach](./TEST-REACH-TARGET.md#measured-2026-10-25-not-shipped)
was.

## What first-class means

<!-- derived-from ./TEST-REACH-TARGET.md#rust -->
<!-- derived-from ./SCIP-CALL-RESOLUTION.md#measured-comparison -->
<!-- derived-from ./AGENT-WORKFLOW-TARGET.md#order-of-work -->

A language is first-class when all four of these hold:

1. **The graph is right, against ground truth.** Call targets agree with a
   compiler-grade indexer, and test reach is scored against a coverage run
   on real repositories.
2. **The ground truth is gated.** CI fails when a floor drops.
3. **Every tool eval has cases in the language.** The review, architecture,
   flow, and refactor evals each have cases for it.
4. **An agent gains.** A pre-registered agent experiment shows fewer tokens
   or less time at equal accuracy.

Rust meets 1, 2, and 4, and partly meets 3. The capabilities behind that,
read from source at `44c098c6`:

| # | Capability | Rust evidence |
|---|---|---|
| C1 | Modules and packages resolved from the root | `rust_lang/modules.rs:247` builds the crate's module tree from its root, `:460` finds the crate roots of a file |
| C2 | Declared visibility in `modifiers` | `rust_lang/mod.rs:1135` (#216); `dead_code.rs:285` does not treat `pub(crate)` as public API |
| C3 | Test code classified, including test-only files | `modules.rs:206` and `:241` gate the files under `#[cfg(test)] mod` (#215); only test attributes make a `Test` node (#216) |
| C4 | Calls through types and traits | `postprocess/impl_members.rs`; `findings.rs:593` and `:615` reach a trait impl's methods through its type |
| C5 | Units with declared dependencies | `units.rs:181-207` reads Cargo `[dependencies]` |
| C6 | SCIP call targets are authoritative | `build --scip`; 788 of 789 node edges agree with rust-analyzer |
| C7 | Test reach scored against coverage, gated | `test_reach_rust_thresholds.yaml` (llvm-cov); no hop limit (`findings.rs:30`) |
| C8 | Cases in each tool eval | review 16, architecture 3, flow 2, refactor 1 |
| C9 | Language rules in dead-code verification | `dead_code_verify.rs:583` and `:601` (trait context, impl header); `#[pymethods]` exports |
| C10 | An agent experiment that gained | E13: 0.74x tokens, 0.61x time at equal accuracy |
| C11 | FFI bridges | `rust_lang/ffi.rs` (PyO3, napi, wasm-bindgen, UniFFI, `no_mangle`) |

Paths are under `crates/dagayn-parser/src/`, `crates/dagayn-graph/src/`, or
`crates/dagayn-tools/src/`.

## How this was measured

On 2026-10-26, with the installed `dagayn` 10.0.0. The graphs built here
report the same extractor versions as `main` at `44c098c6` (rust 25,
python 16, javascript 12, go 8). Every graph was built on a copy in a
scratch directory with `dagayn build --repo COPY --force`. No repository's
own graph was rebuilt.

| Corpus | Language | Source | Size |
|---|---|---|---|
| dagayn | Rust, Python | the main checkout's graph, full build at `44c098c6` | 3,260 Rust and 1,489 Python production functions |
| rdra-ish-dsl | Rust | copy of `~/src/rdra-ish-dsl` | 950 production functions |
| tstodon | TypeScript | copy of `~/src/tstodon`, without `node_modules` | 1,061 production functions |
| kong v1.15.0 | Go | copy from the Go module cache | 347 |
| testify v1.12.1 | Go | copy from the Go module cache | 813 |
| scip-go v0.2.7 | Go | copy from the Go module cache | 137 |
| compress v1.20.1 | Go | copy from the Go module cache | 1,267 |

No Go repository is checked out under `~/src`. The four Go modules are
release archives from `~/go/pkg/mod`, so they have no git history. The
graph numbers are SQL counts over `nodes` and `edges`. Test code is a
`Test` node, an `is_test` node, or a file on a test path. Generated,
vendored, fixture, and example paths are excluded. The scripts were kept
outside the repository, as for the
[SCIP proof of concept](./SCIP-CALL-RESOLUTION.md#measured-comparison).

## Capability matrix

<!-- derived-from #what-first-class-means -->
<!-- derived-from #how-this-was-measured -->

### Graph numbers

"Plain call to a same-directory function" counts unresolved `CALLS` edges
with no receiver whose name a function in the caller's own directory
defines. In Go that directory is the package, and the compiler resolves
these calls with no import at all.

| Measure | Rust (dagayn / rdra) | Python (dagayn) | TS (tstodon) | Go (kong / testify / scip-go / compress) |
|---|---|---|---|---|
| `CALLS` unresolved | 0.046 / 0.077 | 0.025 | 0.292 | 0.494 / 0.426 / 0.297 / 0.370 |
| ... of them on an unknown receiver | 0.78 / 0.74 | 0.78 | 0.93 | 0.22 / 0.17 / 0.28 / 0.21 |
| Plain call to a same-directory function, unresolved | 29 / 50 | 17 (any directory) | 34 | 502 / 1,424 / 56 / 3,098 |
| `IMPORTS_FROM` to a repository file | 0.64 / 0.81 | 0.43 (0.56 external) | 0.74 | 0 / 0 / 0 / 0 (12 / 24 / 48 / 127 in-module imports) |
| Production functions with no caller or referrer | 0.054 / 0.086 | 0.106 | 0.025 | 0.516 / 0.284 / 0.416 / 0.478 |
| `Test` nodes | 803 / 399 | 2,490 | 662 | 0 / 0 / 0 / 0 |
| `TESTED_BY` edges | 4,072 / 1,116 | 5,932 | 3,194 | 0 / 0 / 0 / 0 |
| Production functions with `modifiers` | 0.32 / 0.30 | 0 | 0.56 | 0 / 0 / 0 / 0 |
| `IMPLEMENTS` / `INHERITS` edges | 53 / 43 | 98 | 2 | 0 everywhere |

### Tool answers

- **Go test reach.** testify's tests were run offline with coverage
  (`go test -coverpkg` on `assert`, `require`, and `internal`;
  `go tool cover -func`). `http` and `mock` need a module that is not in
  the cache, `suite` panicked under Go 1.24, and `TestDirExists` and
  `TestNoDirExists` fail outside the source tree, so those were left out.
  `tests_for`'s `test_reach` was then asked of each of the 735 production
  functions, as `eval/run_test_reach_eval.py` does. dagayn called 720 of
  them untested. 277 of the 292 functions that ran were among them (111 with
  no caller at all, 166 with no test path). The harness's three numbers
  (untested precision 0.615, recall 1.000, tested precision 1.000 on 15)
  only restate that almost everything was called untested: 0.615 is close
  to the share that did not run (0.60).
- **Go `tests_for kong.go::New`** returns 0 tests and `nearest_test: null`.
  kong's tests call it as `kong.New(...)` from `package kong_test`
  (`kong_test.go:26`).
- **Go `importers_of internal/symbols`** on scip-go returns `not_found`,
  though 8 files import `github.com/scip-code/scip-go/internal/symbols`.
- **Go architecture overview** on scip-go shows 1 unit, `tests: 0`, and no
  unit edges.
- **Go `callers_of mapper.go::NewRegistry`** on kong finds all 3 callers.
  Bare edges are matched by name at query time, so callers mostly survive
  without resolution, but no test or import walk passes through them.
- **`dead_code`.** kong: 3 reported, 0 right. Two are benchmarks, which
  `go test -bench` runs. The third is `providerCLI.ProvideFoo`, which kong
  calls by reflection on the `Provide` prefix (`context.go:829`). tstodon:
  5 reported, 4 right (unused exports in `packages/worker/src`); the fifth
  is in a generated Prisma file. dagayn: 0 reported, 422 suppressed.

### TypeScript and JavaScript

| # | Status | Evidence |
|---|---|---|
| C1 | present | tsconfig `paths` and workspace packages (`js_resolve.rs:76-143`, `js_workspace.rs`, #212); 0.74 of imports reach a file. Package imports (`zod`, `hono`, `@prisma/client`) carry no `external` mark (`extra: {}`), unlike Python's: 0.26 of imports on tstodon |
| C2 | present | `js_declarations.rs:592-626` (`public` / `private` / `protected`, `readonly`, `abstract`, ...); `exported` in `extra` (`js_like.rs:870-885`) |
| C3 | present | test runner calls (`js_tests.rs`); 662 `Test` nodes on tstodon |
| C4 | partial | `member_shape` and heritage edges; on tstodon, 1,235 unresolved method calls name a repository method. On Hono, method-call precision 0.95 and recall 0.70 ([E14 and E15](./AGENT-WORKFLOW-TARGET.md#order-of-work)). Lazy `await import()` routes and anonymous route arrows are not resolved ([open measurements](./OPEN-MEASUREMENTS.md#flow-entry-points)) |
| C5 | present | npm `dependencies` / `peerDependencies` (`units.rs:209-221`) |
| C6 | present | authoritative; 73 of 89 node edges agree, 13 where SCIP shows dagayn wrong |
| C7 | **missing** | no coverage harness and no gate |
| C8 | partial | review 4, architecture 2, flow 1, refactor 0 |
| C9 | present | export markers (`dead_code.rs:285-306`); tstodon 4 of 5 right |
| C10 | partial | E15: time 0.65x at equal accuracy, tokens not different |
| C11 | n/a | |

### Python

| # | Status | Evidence |
|---|---|---|
| C1 | present | module-to-file resolution and the src layout (`python/mod.rs:1377-1390`, `:1908`); 0.43 of imports reach a file, 0.56 are marked external |
| C2 | n/a | Python declares no visibility; the `_name` convention is read by name, and lazy-export tables are read (`python/mod.rs:2003`) |
| C3 | present | test paths and `conftest.py` (`python/mod.rs:113-114`, `:1052`) |
| C4 | partial | dunder, property, and validator methods reached through their class (`findings.rs:537-579`); a call through a base class does not reach the override ([left open](./TEST-REACH-TARGET.md#code-that-runs-without-a-caller)) |
| C5 | partial | the outermost `__init__.py` directory is a unit (`units.rs:230-237`); no manifest dependencies, and a namespace package falls back to its directory |
| C6 | partial | fill only: pyright leaves unannotated parameters `Unknown`, so SCIP cannot type 2,049 of dagayn's node edges |
| C7 | present | the gate is 0.82 / 0.64 / 0.93; measured 0.839 / 0.656 / 0.948, better than Rust's 0.464 / 0.267 / 0.944, but on dagayn's own code only |
| C8 | present | review 31, architecture 12, flow 11, refactor 6 |
| C9 | present | decorator, registration, and protocol-method rules (`dead_code_verify.rs`) |
| C10 | **failed** | 1.16x to 1.40x tokens ([below](#why-python-cost-agents-tokens)) |
| C11 | present | PyO3 bridges to Rust (`python/bridges.rs`) |

### Go

| # | Status | Evidence |
|---|---|---|
| C1 | **missing** | an import is recorded as its path string (`go.rs:133-162`) and never bound to the package's directory, though `go_module_path` (`go.rs:1393`) knows the module path. The package a file declares is not recorded as its namespace: C#, Java, Kotlin, PHP, and Scala call `set_declared_namespaces` (`util.rs:182`), `go.rs` does not. So bare-name binding (`bare_names.rs:399-410`, same file, an import, or a namespace) never binds a call to another file of the same package |
| C2 | **missing** | `modifiers: None` for every type and function (`go.rs:194`, `:262`). Exported means capitalized, and `external_api_candidate` (`dead_code.rs:277-310`) has no Go rule |
| C3 | **missing in the graph** | `is_test: false` everywhere (`go.rs:195`, `:263`): `TestX`, `BenchmarkX`, `FuzzX`, and `ExampleX` are `Function` nodes, and no `TESTED_BY` edge exists. Only the tools' path rule (`_test.` in `findings.rs:229-239`) knows the files are tests |
| C4 | partial | method calls on a receiver of a declared type (`go.rs:588`); interfaces are satisfied implicitly and no edge records it (0 `IMPLEMENTS`) |
| C5 | partial | a `go.mod` is one unit (`units.rs:223-229`), with no `require` dependencies; packages, Go's unit of import, are not units |
| C6 | partial | wired to fill only, run once on a small module ([open questions](./SCIP-CALL-RESOLUTION.md#open-questions)); `scip-go` is not installed here |
| C7 | **missing** | no harness; testify measured [above](#tool-answers) |
| C8 | **missing** | no case in any eval |
| C9 | partial | `init` and protocol methods (`dead_code_verify.rs:319-330`); no rule for `Benchmark` / `Fuzz` / `Example` functions or for exported names |
| C10 | **missing** | no experiment |
| C11 | present | cgo and wasm bridges (`go.rs:280`, `:1316`) |

## Why Python cost agents tokens

<!-- derived-from ./AGENT-WORKFLOW-TARGET.md#order-of-work -->

The experiments were E1, E10, and E11 (Claude Haiku 5.5, ten runs per
question, on dagayn's own Python). The questions asked for transitive
importers, directory import cycles, Markdown dependencies, and direct
callers:

- **The cost was turns, not wrong answers.** When the questions were worded
  to the graph's rules (E10), both arms got 49 of 50 exact. With dagayn the
  agent took 1.31x the tokens.
- **Agents re-checked the graph's rows one by one** (E11: a median of 5 to 8
  checks per import question), and each turn carried the long reply.
  Without dagayn the agent wrote one `ast` script and answered: Python's
  syntax names every import and every plain call it needs.
- **Trusting the answer was cheaper and less accurate.** Telling the agent
  it could use the answer as it was cut tokens to 0.78x (E8), but exact
  matches fell from 48 to 35 of 50. The misses were rows where the graph
  counts by a rule other than the question's: a lazy-export import, or a
  directive in the middle of a section.

The Python extractor is not the weak part: Python test reach is the best
measured answer dagayn gives (untested precision 0.839, the Rust figure is
0.464). What would change the result is questions a short script cannot
answer:

- which tests reach a function
- callers of a method on a receiver whose type no import names
- callers through a table or a decorator registry
- what a change left unedited

Where the graph's rule and the question's differ, the answer would have to
say so in a way the agent trusts without re-checking. Rust gained for the
same reason: no short script answers its questions (E12, E13).

## Gaps ranked by the four tools

<!-- derived-from #capability-matrix -->

Agents call `review_tool(mode="changes")` most (21 calls), then search (17),
`callers_of` (13), and `dead_code` (9) ([evidence](../POSITIONING.md#evidence)).
Each gap is ranked by the measured answers it breaks:

| Rank | Gap | Tools it breaks | Measured size |
|---|---|---|---|
| 1 | Go test functions are not tests in the graph (C3) | review `untested_change`, `tests_to_run`; `tests_for` | 0 `TESTED_BY`; 277 of 292 functions that ran called untested on testify |
| 2 | Go same-package calls unresolved (C1) | `callers_of` (name match only), review `unchanged_caller`, test reach, `dead_code` | 3,098 calls on compress; 0.28 to 0.52 of functions with no caller |
| 3 | Go imports not bound to packages (C1) | `importers_of`, `import_cycles`, architecture units, `kong.New` from `package kong_test` | 0 of 211 in-module imports reach a file over four modules (kong 12, testify 24, scip-go 48, compress 127) |
| 4 | TypeScript test reach unmeasured (C7) | review `untested_change`, `tests_for` | unknown: no coverage run yet |
| 5 | Go dead-code rules (C2, C9) | `dead_code` | kong 0 of 3 right |
| 6 | Python agent gain not shown (C10) | all | 1.16x to 1.40x tokens |
| 7 | TypeScript method receivers (C4) | `callers_of`, review | 1,235 unresolved calls naming a repository method on tstodon; the answer is SCIP, not new inference rules |
| 8 | TypeScript package imports unmarked (C1) | `importers_of` noise, unit edges | 0.26 of imports on tstodon |
| 9 | Go and TypeScript eval cases (C8) | every gate | Go 0 cases; TypeScript 0 refactor cases |

Search is not on the list. It ranks nodes by name and text, and the search
eval measured no language-specific gap.

## Work list

<!-- derived-from #gaps-ranked-by-the-four-tools -->
<!-- constrained-by ../POSITIONING.md#call-resolution-comes-from-scip -->

Measurements come first, then fixes, in this order. Each item names the
number it must move and its acceptance bar. An item that misses its bar is
recorded here as not shipped, with what it measured. Floors are set from the
first measured run, less the margin the existing threshold files use, as
`test_reach_rust_thresholds.yaml` was.

No item adds receiver-type inference rules for any language. That layer is
frozen ([SCIP](./SCIP-CALL-RESOLUTION.md#freezing-the-inference-layer)),
and in Go the plain-call gap is three to four times larger than the
method-call gap.

### Measure first

1. **M1, TypeScript test reach against coverage.** A reader for vitest's
   coverage JSON in `eval/run_test_reach_eval.py`, run on tstodon and
   cfwdon's `web-ui`, report only.
   Bar: the three numbers and the cause table recorded here, and a floor
   file if tested precision is at least 0.90. Below that, the causes
   decide the next TypeScript item, not the floor.
2. **M2, Go test reach against coverage.** A reader for
   `go tool cover -func` output, same harness. The baseline is above: 720
   of 735 called untested on testify.
   Bar: the harness reproduces 277 of 292 on testify, and runs on a second
   Go repository with git history (see [data](#data-and-repositories-needed)).
3. **M3, Python agent experiment on questions a script cannot answer.**
   Pre-registered, Claude Haiku 5.5, ten runs per question, with and without
   dagayn, on dagayn and on one third-party Python repository. Use the four
   question kinds [above](#why-python-cost-agents-tokens).
   Bar: tokens at most 0.85x (Holm p < 0.05) at equal exact matches, the bar
   E12 and E13 cleared for Rust. If it fails, Python stays measured and
   maintained, with no further investment from this plan.

### Go work

4. **G1, Go tests in the graph.** `TestX(t *testing.T)`,
   `BenchmarkX(b *testing.B)`, `FuzzX(f *testing.F)`, and `ExampleX()` in
   `_test.go` files become `Test` nodes, with `TESTED_BY` derived from
   their calls, as in the other extractors.
   Bar: `tests_to_run_go` and `untested_change_go` review cases pass, and M2
   on testify gets fewer functions that ran called untested than 277.
   Alone it moves little while calls stay unresolved, so G1 and G2 are
   measured both apart and together.
5. **G2, the package as a namespace.** Each Go file declares
   `module path + directory` (the package's import path) as its namespace,
   so bare-name binding sees every file of the package.
   Bar: on compress, same-directory plain calls 3,098 → under 300, and
   functions with no caller 0.478 → under 0.15. A hand check of 30
   newly bound edges finds no call bound into another package. M2 on
   testify: functions that ran called untested at most a third of 277, at
   tested precision ≥ 0.92. `unchanged_caller_go` passes.
   The no-caller bar is revised under
   [Decisions](#decisions-2026-10-26): no extractor can reach 0.15.
6. **G3, imports bound to packages.** An import of `module/sub/pkg` binds to
   the files of `sub/pkg`, for internal and external test packages
   (`package kong_test`). A package call (`kong.New`) then resolves through
   the import.
   Bar: the 211 in-module imports of the four modules reach files (scip-go
   48, compress 127).
   `importers_of` answers for `internal/symbols` (8 importers). `kong.go::New`
   gets a nearest test. A `dangling_rename_go` case and an
   `import_cycles` negative case pass. Go forbids import cycles, so there
   is no positive case.
7. **G4, Go units by package.** A Go package (a directory of `.go` files
   under a `go.mod`) is a unit, and `require` lines become declared
   dependencies between modules of one repository.
   Bar: scip-go's overview shows its `internal/*` packages and the edges
   between them, and `neg_layered_units` gets a Go counterpart that passes.
8. **G5, Go dead-code rules.** Capitalized names in a module that other
   modules import count as public API. `Benchmark`, `Fuzz`, and `Example`
   functions are test entry points.
   Bar: kong reports neither benchmark, and `pos_unused_go_function` plus a
   `neg_exported_go_function` refactor case pass. `ProvideFoo` (reflection)
   is not covered; it stays a known false report.
9. **G6, the SCIP comparison for Go.** `scip-go` on compress and scip-go,
   compared edge by edge as the proof of concept did. It decides whether
   Go SCIP becomes authoritative.
   Bar: the agreement table filled for Go. Authoritative only if dagayn's
   own node edges agree on at least 0.95, as Rust's did.
10. **G7, Go agent experiment.** After G1 to G3, the same design as E13,
    with an answer key from scip-go.
    Bar: tokens or time at most 0.85x at equal accuracy. Go is first-class
    when this and the M2 gate both hold.

### TypeScript work

11. **T1, package imports marked external.** An import that resolves to no
    repository file and names a package in `package.json` gets
    `external: true` and `external_package`.
    Bar: tstodon's unmarked imports 0.26 → under 0.01, with no change to
    `calls_to_node`, the review eval, or the architecture eval.
12. **T2, TypeScript under `--scip`.** M1 rerun on a `build --scip` graph,
    as the Rust steps table did.
    Bar: the numbers recorded. Whether to gate `--scip` follows the Rust
    decision.
13. **T3, the causes M1 finds.** Lazy `await import()` routes and anonymous
    route arrows, only if M1's cause table attributes at least 10 wrong
    answers to them.
    Bar: those answers fixed, and tested precision no lower.
14. **T4, eval coverage.** A refactor `pos_unused_typescript_export` case,
    and flow and architecture cases from the causes M1 finds.

### Python work

15. **P1, a third-party test reach run.** The existing harness on one
    third-party Python repository with a pytest suite, report only.
    Bar: the numbers recorded. If untested precision is under 0.70, its
    cause table replaces dagayn's own as the Python baseline.
16. **P2, overrides through a base class.** Only if P1 or M3 shows calls
    through a base class among the wrong answers. This is the case
    [left open](./TEST-REACH-TARGET.md#code-that-runs-without-a-caller).
    Bar: the eval case named there passes, and P1's untested precision
    rises.

## Data and repositories needed

<!-- derived-from #work-list -->

- **A Go corpus with history.** Nothing is cloned yet. Proposed: clone
  `github.com/sourcegraph/scip-go` (in-module `internal/*` packages, small)
  and `github.com/klauspost/compress` (1,267 production functions, 734
  methods). Neither builds with the local Go 1.24.13: both need Go 1.25.
  testify runs offline from the module cache, apart from `http`, `mock`,
  and `suite`. kong does not (`alecthomas/assert/v2` is missing).
- **Go tooling.** Go 1.25 and `scip-go` for G6 and the answer key of G7.
  The `scip-go` v0.2.7 source in the module cache needs Go 1.25 too.
- **A third-party Python repository** with a pytest suite, for M3 and P1.
  None is on disk. `~/src/kamae-py` has 9 Python files and is mostly
  Markdown.
- **TypeScript coverage.** tstodon and cfwdon with `node_modules`
  installed in a scratch copy, for M1.
- **SCIP indexers.** `scip-typescript` and `scip-python` are not installed.
  rust-analyzer is.
- **Agent experiments.** The `claude` CLI and API access for M3 and G7
  (`eval/run_agent_tokens_eval.py`; Haiku is about 1 USD per 100 runs).

## Risks

- **One author.** tstodon, cfwdon, and dagayn were written by one person.
  The Go corpus would be the first third-party code measured, which helps,
  but its numbers may not match the others'.
- **Name binding across a package.** G2 widens what a bare name can bind
  to. In a large package, two methods of different types with one name are
  already common (`Write`, `Close`). G2 binds plain calls only, and its hand
  check guards that.
- **Go coverage is per function.** `go tool cover -func` reports a function
  as run when any statement ran, so a branch the test does not take is
  invisible, as in Rust.
- **Hop limit.** Go keeps the 4-hop test walk (`findings.rs:30`) until M2
  shows how deep its test paths run.
- **Priority.** The positioning puts Rust untested precision before new
  languages. The Go items are new investment in a language with no agent
  measurement, so they wait for the owner's decision below.

## Out of scope

- New receiver-type inference rules for any language
  ([frozen](./SCIP-CALL-RESOLUTION.md#freezing-the-inference-layer)).
- Reflection-driven calls (kong's `Provide*`, `reflect.Value.Call`), Go
  generics instantiation, and build tags beyond `_test.go`.
- Implicit interface satisfaction as `IMPLEMENTS` edges in Go, unless M2's
  cause table shows calls through an interface among the wrong answers.
- JavaScript without TypeScript as a separate target: it shares the
  TypeScript extractor and follows it.
- Any other language.
- Dispatch-aware reach, which stays
  [not shipped](./TEST-REACH-TARGET.md#measured-2026-10-25-not-shipped).

## Decisions (2026-10-26)

<!-- derived-from #work-list -->

- **Go starts now.** G1 to G3 go ahead without waiting for Rust untested
  precision: Go is the largest measured gap, and they are extractor
  changes, not inference rules. M2 lands with G1, because G1's bar is
  read from it.
- **Python's bar.** If M3 fails, Python is maintained, not invested in.
- **TypeScript's SCIP comes from tsrs.** `scip-typescript` is replaced by
  tsrs crates linked into dagayn, in its conservative mode, which keeps a
  call with several candidates ambiguous. `scip-typescript` names external
  packages `.` on pnpm workspaces (27,963 symbols on tstodon, 2,278 hono and
  zod calls aimed at no package, #219); tsrs has none of these, agrees on all
  4,731 cfwdon calls, and ran in 1.95s against 17.9s on tstodon. dagayn
  moves to Rust 1.99.x and follows tsrs, which is pinned to a git rev.
- **Python's SCIP comes from ty, linked as crates.** `scip-python` binds
  names through star re-exports wrongly (602 HIGH edges on httpx). ty
  linked into dagayn and pyrefly were compared on httpx, dagayn and 104
  pydantic v2 cases. They resolve calls about equally well, but pyrefly
  binds lazy exports to the module's `__getattr__`, which cost 278 HIGH
  edges on dagayn. Its API is `#[doc(hidden)]` and it needs 384 crates
  against ty's 236. ty indexed dagayn in 1.58s against 2.96s for pyrefly,
  and compiled unchanged across 427 upstream commits. The LSP route
  (`ty server`) is not taken. Either way dagayn passes the project's
  interpreter explicitly, which `scip_overlay.py` does not do today.
- **Go is the one language whose SCIP needs its own toolchain.** Rust's
  indexer comes from rustup, and TypeScript's and Python's are linked into
  dagayn. `scip-go` needs a Go toolchain of the version it was built for
  (1.25). So Go's
  first-class bar does not depend on SCIP: G1 to G5 must pass on the
  extractor alone, and G6 uses `scip-go` as an answer key for measurement,
  not as a build requirement.
- **Cloning and tooling.** scip-go and compress may be cloned, and Go 1.25
  and `scip-go` installed, for measurement.
- **G2's no-caller bar is measured over directly referenced functions.**
  On compress, 0.169 of all functions have no direct reference in any
  scip-go index (interface dispatch, method values), so no resolver of
  direct calls reaches 0.15. The bar is now: of the functions scip-go
  references directly, under 0.12 have no caller (go 10: 0.185, go 11
  after G2b: 0.114, with the scip-go overlay: 0.048). The share over all
  functions is still reported. Reaching 0.15 there needs edges from an
  interface method to its implementations, which no item builds.
- **Third-party Python.** Chosen when M3 starts.
