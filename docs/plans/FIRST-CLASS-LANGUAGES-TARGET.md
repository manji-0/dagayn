# TypeScript, Python, and Go at Rust's level

<!-- constrained-by ../POSITIONING.md#quality-before-breadth -->
<!-- constrained-by ../POSITIONING.md#language-breadth-stays -->
<!-- derived-from ../POSITIONING.md#evidence -->

## Question

Rust is the language where dagayn measurably helps an agent. What would it
take for TypeScript (with JavaScript), Python, and Go to get the same level
of support, and in what order should it be built?

Status: in progress. M1, M3, G5 to G10, and T1 to T4 record their results
in the [work list](#work-list); M2 and G1 to G4 record theirs in
[Go support measurements](./GO-SUPPORT-MEASUREMENTS.md). P1 and P2 have not
started. Every work item ships only if it moves a number on a real
repository, and is recorded as not shipped otherwise, the way
[dispatch-aware reach](./TEST-REACH-TARGET.md#measured-2026-10-25-not-shipped)
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
| C7 | Test reach scored against coverage, gated | `test_reach_rust_thresholds.yaml` (llvm-cov); no hop limit (`findings.rs:35`) |
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
| C1 | present | tsconfig `paths` and workspace packages (`js_resolve.rs:76-143`, `js_workspace.rs`, #212); 0.74 of imports reach a file. Package imports (`zod`, `hono`, `@prisma/client`) carried no `external` mark (`extra: {}`), unlike Python's: 0.26 of imports on tstodon, 0 since T1 |
| C2 | present | `js_declarations.rs:592-626` (`public` / `private` / `protected`, `readonly`, `abstract`, ...); `exported` in `extra` (`js_like.rs:870-885`) |
| C3 | present | test runner calls (`js_tests.rs`); 662 `Test` nodes on tstodon |
| C4 | partial | `member_shape` and heritage edges; on tstodon, 1,235 unresolved method calls name a repository method. On Hono, method-call precision 0.95 and recall 0.70 ([E14 and E15](./AGENT-WORKFLOW-TARGET.md#order-of-work)). Lazy `await import()` routes and anonymous route arrows are not resolved ([open measurements](./OPEN-MEASUREMENTS.md#flow-entry-points), #294) |
| C5 | present | npm `dependencies` / `peerDependencies` (`units.rs:209-221`) |
| C6 | present | authoritative; 73 of 89 node edges agree, 13 where SCIP shows dagayn wrong |
| C7 | partial | the harness reads vitest's istanbul JSON (M1): 0.091 / 0.964 / 0.995 on tstodon, 0.842 / 0.468 / 0.765 on web-ui (#295); no gate (#293) |
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
| C10 | **failed** | 1.16x to 1.40x tokens ([below](#why-python-cost-agents-tokens)); M3, on questions a script cannot answer: 1.05x, not significant |
| C11 | present | PyO3 bridges to Rust (`python/bridges.rs`) |

### Go

| # | Status | Evidence |
|---|---|---|
| C1 | present | each file declares its package's import path as its namespace ([G2](./GO-SUPPORT-MEASUREMENTS.md#g2-the-package-as-a-namespace), go 10): on compress, unresolved plain calls to a function of the caller's directory 3,058 → 166. An in-module import targets the package's files and a call through it binds ([G3](./GO-SUPPORT-MEASUREMENTS.md#g3-imports-bound-to-packages), go 12): 12 / 24 / 48 / 127 in-module imports reach a file on kong / testify / scip-go / compress, from 0 |
| C2 | partial | `modifiers` is still `None` for every type and function (`go.rs:668`, `:859`), and `external_api_candidate` (`dead_code.rs:283`) has no Go rule. Exported means capitalized, and `dead_code` reads the name instead: `go_exported_api` (`dead_code_verify.rs:445`, [G5](./GO-SUPPORT-MEASUREMENTS.md#g5-go-dead-code-rules)) |
| C3 | present | every node of a `_test.go` file is test code, and `TestX`, `BenchmarkX`, `FuzzX`, and `ExampleX` are `Test` nodes with `TESTED_BY` edges ([G1](./GO-SUPPORT-MEASUREMENTS.md#g1-go-tests-in-the-graph), go 9): on testify, 0 → 479 `Test` nodes and 0 → 1,343 `TESTED_BY`. With G2, functions that ran but are called untested fall from 277 to 42, and to 21 at go 12 |
| C4 | partial | method calls on a receiver of a declared type (`go.rs:588`); interfaces are satisfied implicitly and no edge records it (0 `IMPLEMENTS`) |
| C5 | present | a Go package is a `go_package` unit ([G4](./GO-SUPPORT-MEASUREMENTS.md#g4-units-by-package)): scip-go's overview goes from 1 unit to 15, with 33 edge pairs. An import of another module's package is a declared dependency when the importing `go.mod` requires that module; a `require` alone draws no edge |
| C6 | present | authoritative under `build --scip`; [G6](./GO-SUPPORT-MEASUREMENTS.md#g6-the-scip-go-comparison) measured it: node edges agree with scip-go on 7,003 of 7,017 (compress) and 287 of 287 (scip-go), and the 234 edges authoritative mode changes on compress are all right ([authoritative Go](./GO-SUPPORT-MEASUREMENTS.md#authoritative-go)) |
| C7 | partial | the harness reads `go tool cover -func` ([M2](./GO-SUPPORT-MEASUREMENTS.md#m2-test-reach-against-go-coverage)): 0.954 / 0.991 / 0.985 on testify and 0.381 / 0.849 / 0.966 on compress at go 12 ([G3](./GO-SUPPORT-MEASUREMENTS.md#test-reach-and-the-no-caller-share)); gated in CI on testify since 2026-11-08, measured 0.965 / 0.993 / 0.991 at go 19 ([the CI floor](./GO-SUPPORT-MEASUREMENTS.md#the-ci-floor)); compress has no floor |
| C8 | partial | review 7, architecture 3, flow 0, refactor 5; no flow case yet (#291) |
| C9 | present | `init` and protocol methods (`dead_code_verify.rs:319-330`); exported names of importable packages (`go_exported_api`, G5); `Benchmark` / `Fuzz` / `Example` functions are `Test` nodes (G1) |
| C10 | **failed** | G7, on questions a script cannot answer: tokens 1.00x, time 0.91x, neither significant ([results](../../eval/g7_go/RESULTS.md)). G8, on eight new questions on 11.0.0: tokens 1.20x (more, Holm p = 0.0003), time 0.97x, 73 against 79 of 80 exact ([results](../../eval/g8_go/RESULTS.md)). G9, under the revised bar, on go extractor 17: LONG time 0.79x (Holm p = 0.0003) at tokens 0.95x and 77 against 75 exact, but SHORT tokens 1.17x, above the 1.10 guard ([results](../../eval/g9_go/RESULTS.md)). G10, through the CLI on twelve new questions (six on the untuned chi), on go extractor 18: LONG tokens 1.03x and time 0.98x (Holm p = 1.0), 75 against 76 exact; SHORT tokens 0.92x, within the 1.25 guard, but 39 against 40 exact ([results](../../eval/g10_go/RESULTS.md)) |
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
| 4 | TypeScript test reach (C7) | review `untested_change`, `tests_for` | measured by M1: 538 of 978 functions that ran called untested on tstodon; tested precision 0.765 on web-ui |
| 5 | Go dead-code rules (C2, C9) | `dead_code` | kong 0 of 3 right |
| 6 | Python agent gain not shown (C10) | all | 1.16x to 1.40x tokens; M3 1.05x |
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
   **Measured** (2026-11-03,
   [TypeScript measurements](./TYPESCRIPT-SUPPORT-MEASUREMENTS.md#m1-test-reach-against-coverage)):
   untested precision, recall, and tested precision are 0.091 / 0.964 /
   0.995 on tstodon (1,034 functions, 538 of the 978 that ran called
   untested) and 0.842 / 0.468 / 0.765 on cfwdon's web-ui (483, 15 of
   312). 470 of tstodon's 538 sit under a Hono route handler that tests
   reach only through a `SELF.fetch` request; 54 of web-ui's 91 functions
   called tested that did not run are called only from event handlers the
   tests never fire. No floor: M1 is report only, and neither repository
   has a coverage run in CI.
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
   **Measured, not met** (2026-10-27,
   [pre-registration](../../eval/m3_python/PREREG.md),
   [results](../../eval/m3_python/RESULTS.md)): four questions on dagayn
   and four on httpx 0.28.1, 160 runs. Tokens 1.05x (Holm p = 0.56), time
   0.93x (Holm p = 0.30), 80 of 80 exact in both arms. dagayn won where its
   answer was complete and the question needed the diff (X4: 0.68x tokens)
   and lost where it was empty or partial (test reach through a callback,
   calls through a table: 1.26x to 1.38x); agents read the deciding files
   instead of scripting. Python is maintained, not invested in.

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
   **Met** (2026-11-03,
   [Go measurements](./GO-SUPPORT-MEASUREMENTS.md#g5-go-dead-code-rules)):
   a capitalized name of a package other than `main`, outside `internal`,
   is left out as `exported_api`. Over kong, testify, compress, and
   scip-go, reports go from 3 to 1. The two removed are `RoundTrip`
   methods behind `http.RoundTripper`, and the one kept, scip-go's
   `internal/symbols` `FormatCode`, is unused. Since G1, kong reports
   nothing: its benchmarks are `Test` nodes and `ProvideFoo` is in
   `kong_test.go`, so neither is a candidate. Both refactor cases pass,
   and so does `pos_unused_go_internal_export`.
9. **G6, the SCIP comparison for Go.** `scip-go` on compress and scip-go,
   compared edge by edge as the proof of concept did. It decides whether
   Go SCIP becomes authoritative.
   Bar: the agreement table filled for Go. Authoritative only if dagayn's
   own node edges agree on at least 0.95, as Rust's did.
   **Met** (2026-11-03,
   [Go measurements](./GO-SUPPORT-MEASUREMENTS.md#g6-the-scip-go-comparison)):
   node edges agree on 7,003 of 7,017 on compress (0.998) and 287 of 287
   on scip-go. testify, added because its forwarders exercise the case,
   was at 4,503 of 4,836 through a bare-call bug that
   [#250](https://github.com/manji-0/dagayn/pull/250) fixes (4,813 after).
   The owner made Go authoritative once the overlay's chain window
   stopped on the edge's own line
   ([#259](https://github.com/manji-0/dagayn/pull/259)): on compress the
   234 edges it changes against fill mode are all right
   ([authoritative Go](./GO-SUPPORT-MEASUREMENTS.md#authoritative-go)).
10. **G7, Go agent experiment.** After G1 to G3, the same design as E13,
    with an answer key from scip-go.
    Bar: tokens or time at most 0.85x at equal accuracy. Go is first-class
    when this and the M2 gate both hold.
    **Measured, not met** (2026-11-03,
    [pre-registration](../../eval/g7_go/PREREG.md),
    [results](../../eval/g7_go/RESULTS.md)): four questions on testify
    v1.12.1 and four on compress v1.20.1, 160 runs, keys from scip-go and
    per-test coverage. Tokens 1.00x (Holm p = 1.0), time 0.91x (Holm p =
    0.21), 78 against 74 of 80 exact. dagayn helped where its answer was
    complete and the question was about receiver types and a diff (C4:
    0.79x tokens, 10 against 6 exact) and cost where it was empty (a call
    through a field holding a method expression, C3: 1.36x, Holm p = 0.022)
    or listed 65 tests for 4 that run the function (C1). Go is not
    first-class. The defects the runs show are listed in the results.
    **G8, the rerun on new questions: measured, not met** (2026-11-04,
    [pre-registration](../../eval/g8_go/PREREG.md),
    [results](../../eval/g8_go/RESULTS.md),
    [decisions](#decisions-2026-11-04)). Eight new questions drawn by a
    seeded rule, 160 runs on 11.0.0 (go extractor 15). Tokens were 1.20x,
    more than without dagayn (Holm p = 0.0003). Time was 0.97x (Holm p =
    0.38), and 73 of 80 answers were exact against 79 of 80, so both
    parts of the bar fail. The one gain was C5's test reach (0.72x time,
    Holm p = 0.039). The largest cost was a function value called through
    a field, where dagayn's answer had 1 of 3 callers (C7: 1.39x, Holm
    p = 0.0096). On questions that the baseline answered in about 5
    turns, even complete answers cost more. In 34 of 160 runs, the first
    call named the method as `(*T).M`, or as a `T.M` that names one node,
    and got `not_found` or a false `ambiguous`
    ([#275](https://github.com/manji-0/dagayn/issues/275)). On G7's
    questions (report only), this hid the C3 fix (1.76x), and the pooled
    tokens moved from 1.00x to 1.07x. Go is not first-class. The defects
    the runs show are listed in the results.
    **G9, under the owner's revised bar: measured, not met** (2026-11-04,
    [pre-registration](../../eval/g9_go/PREREG.md),
    [results](../../eval/g9_go/RESULTS.md),
    [decisions](#decisions-2026-11-04-after-g8)). Twelve new questions on
    testify, compress, and kong v1.15.0: 8 LONG, where the bar is 0.85x
    tokens or time at Holm p < 0.05, and 4 SHORT, where the token ratio
    must stay at most 1.10 with no fewer exact answers. 240 runs on go
    extractor 17. LONG is met through time: 0.79x (Holm p = 0.0003), with
    tokens at 0.95x (Holm p = 0.44) and 77 against 75 of 80 exact. SHORT
    fails: tokens were 1.17x, though exact answers were 40 against 37. The
    SHORT cost came from kong (K3 1.66x, K4 1.37x), the one repository the
    fixes since G8 were not tuned on; kong as a whole was 1.31x tokens
    (Holm p = 0.0003) and testify 0.80x. On SHORT questions the dagayn arm
    made one model request more than the baseline (a median of 6 against
    5): the schema load and the call came first, and 36 of 40 runs still
    searched for the name after a complete answer. The one accuracy loss
    (C11, 3 runs) followed a summary that pointed at a call on the
    standard library's `gzip.Writer`. Go is not first-class. The defects
    the runs show are listed in the results (#288, #289, #290). The next
    registered run is G10 (#292).
    **G10, new questions and an untuned repository, through the CLI:
    measured, not met** (2026-11-05,
    [pre-registration](../../eval/g10_go/PREREG.md),
    [results](../../eval/g10_go/RESULTS.md),
    [decisions](#decisions-2026-11-05-after-g9)). Twelve new questions, 8
    LONG and 4 SHORT, six of them on chi v5.5.0, a repository no fix since
    G7 was tuned on. The candidate reached dagayn through `dagayn tool` in
    the shell, with no MCP server. 240 runs on go extractor 18. LONG fails:
    tokens 1.03x and time 0.98x (Holm p = 1.0 for both), with 75 against
    76 of 80 exact. SHORT fails on accuracy alone: tokens were 0.92x,
    within the 1.25 guard, but exact answers were 39 against 40. The one
    miss kept a row that dagayn listed from chi's `_examples/`, a
    directory the Go tools and the question leave out. chi's LONG
    questions cost more (tokens 1.14x, time 1.12x, Holm p = 0.077,
    secondary). The largest loss was test reach through a function passed
    as a value (H2, 1.54x, Holm p = 0.0012): `tests_for` named a nearest
    test and suggested a depth call that returned no test. compress, which
    was tuned on, gained in both (tokens 0.84x, time 0.81x, secondary). No
    run called `ToolSearch`, and no CLI call failed on its arguments. On
    every question but one, at least half of the dagayn runs searched for
    the name again after a complete reply. Go is not first-class. The
    defects the runs show are listed in the results.

### TypeScript work

11. **T1, package imports marked external.** An import that resolves to no
    repository file and names a package in `package.json` gets
    `external: true` and `external_package`.
    Bar: tstodon's unmarked imports 0.26 → under 0.01, with no change to
    `calls_to_node`, the review eval, or the architecture eval.
    **Met** (2026-11-03): on a fresh build, 1,026 of tstodon's 3,425
    TypeScript imports (0.300) reached no file and had no mark: 931 name a
    package its `package.json` files declare (`zod`, `hono`,
    `@prisma/client/...`), 94 are `cloudflare:test` and
    `cloudflare:workers`, which no `package.json` names, and 1 is a
    missing relative module (`./query_compiler_fast_bg.wasm?module`). The
    rule is the one calls through these imports already used (a package
    name that resolves to no file and is not a tsconfig alias), not a
    `package.json` lookup, which would have left the 94 (0.027). A missing
    relative module stays unmarked: it is a missing file or a build output,
    not a package. Unmarked imports are now that 1 (0.0003). The import
    targets keep the specifier as written. Every other edge is unchanged
    (`calls_to_node` 9,149 of 26,642 before and after), and so are the
    architecture overview and import cycles of tstodon, and the review,
    architecture, flow, and refactor evals.
12. **T2, TypeScript under `--scip`.** M1 rerun on a `build --scip` graph,
    as the Rust steps table did.
    Bar: the numbers recorded. Whether to gate `--scip` follows the Rust
    decision.
    **Measured** with M1: tstodon 0.092 / 0.964 / 0.996 (535 ran and
    called untested, 3 fewer: Durable Object stub calls), web-ui
    unchanged.
13. **T3, request-boundary edges.** Two framework wirings recorded as
    edges, as the pydantic work recorded validators, not as a new
    type-inference rule: a test's `SELF.fetch` (from `cloudflare:test`) to
    the worker's `default.fetch`, and a Hono route registration
    (`app.get(path, handler)`, `.post`, `.put`, `.delete`, `.use`) to its
    handler, matched by path where the test's URL and the route's path
    allow it.
    Bar: most of tstodon's 470 request-boundary answers fixed, and tested
    precision at least 0.95, both measured against vitest coverage as M1
    is. The owner revised the precision bar from "no lower" (0.996) after
    the measurement below: no route-level rule exceeds 0.966 on tstodon.
    This replaces the first scope (lazy `await import()` routes and
    anonymous route arrows, if M1 gave them 10 or more wrong answers),
    because M1 gave that scope 0: tstodon has no lazy `await import()`,
    and its 457 answers under anonymous route arrows stay wrong with an
    arrow node, because no test reaches a route file by any edge. web-ui's
    20 lazy pages are wrong the other way (called tested, not run). The
    break is the request
    ([table](./TYPESCRIPT-SUPPORT-MEASUREMENTS.md#tstodon-why-the-functions-that-ran-are-called-untested)).
    M1's other causes with 10 or more answers are decided below
    ([decisions](#decisions-2026-11-03)): the hop limit (55 on tstodon)
    and call-valued module constants (#239, 11 on web-ui).
    **Measured**
    ([T3](./TYPESCRIPT-SUPPORT-MEASUREMENTS.md#t3-request-boundary-edges)):
    met. All 470 are fixed, and tstodon's ran and called untested falls
    from 482 to 10. Tested precision is 0.964, from 0.996: 34 functions
    that did not run are now called tested, and 32 of them are on an
    untaken path in a handler that ran. Matching by path does not recover
    them. With each handler linked only if coverage says it ran, precision
    was 0.966. So a request reaches every route of the app. tstodon is
    close to the best case for that rule (245 of its 252 handlers ran).
    web-ui does not change.
14. **T4, eval coverage.** A refactor `pos_unused_typescript_export` case,
    and flow and architecture cases from the causes M1 finds.
    **Met** (2026-11-03): six cases, each failing on the code before its
    rule or with its rule disabled.

    | Case | Cause | Fails when |
    |---|---|---|
    | refactor `pos_unused_typescript_export` | an application's exports are not API | TypeScript exports are left out as `exported_api`, as Go's are |
    | flow `pos_typescript_worker_route_handler` | request boundary (T3) | the request-boundary pass is off: the entry is `src/app.ts` (`module_level`) |
    | flow `pos_typescript_shorthand_member` | #239 shorthand member | shorthand aliases are off: the target is its own `uncalled` entry |
    | flow `pos_typescript_call_valued_constant` | #239 call-valued constant | called constants get no node: no target |
    | architecture `neg_typescript_helper_tested_through_request` | request boundary (T3) | the request-boundary pass is off: `untested_core` on the helper |
    | architecture `neg_typescript_helper_tested_six_hops_away` | hop limit | on `main`: `untested_core` walked 4 hops in every language |

    The last case found a gap: `untested_core` kept its own 4-hop limit
    when `untested_change` moved to 8 for TypeScript and JavaScript. It
    now uses the same per-language limit (no limit for Rust). On tstodon
    and dagayn's own graph its findings do not change (tstodon keeps
    `unitKind`, called at module load). A `format` imported from
    `date-fns` hid an unused local `format` from `dead_code`, because the
    source scan counted any occurrence of the name, so the refactor case
    uses a name no other file mentions. 11.1.0 fixes that
    ([#257](https://github.com/manji-0/dagayn/issues/257)), and the case
    `pos_unused_function_shadowed_by_import` covers it.

### Python work

15. **P1, a third-party test reach run.** The existing harness on one
    third-party Python repository with a pytest suite, report only.
    Bar: the numbers recorded. If untested precision is under 0.70, its
    cause table replaces dagayn's own as the Python baseline. Not started
    (#298).
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
- **Hop limit.** Go keeps the 4-hop test walk (`findings.rs:35`) until M2
  shows how deep its test paths run (#293).
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
- **Third-party Python.** httpx 0.28.1 (b5addb6), chosen when M3 started.

## Decisions (2026-11-03)

<!-- derived-from ./TYPESCRIPT-SUPPORT-MEASUREMENTS.md#m1-test-reach-against-coverage -->

Taken by the owner after M1.

- **TypeScript and JavaScript walk 8 caller hops.** M1 found 55 tstodon
  functions with a test 5 to 8 hops away, and all 55 ran. At 8 hops
  (`caller_test_depth`), tstodon's ran-and-called-untested falls from 538
  to 483 and tested precision rises from 0.995 to 0.996; web-ui does not
  change. Other languages keep their limits, because their CI gates were
  tuned at them: 4 for Python and Go, none for Rust.
- **T3 is request-boundary edges** ([T3](#typescript-work)), not the lazy
  import and route arrow scope, which M1 gave no wrong answers.
- **#239 is raised in priority and is being done now.** Call-valued
  module constants and shorthand members gave web-ui 11 wrong answers
  (10 and 1) and tstodon 1.
- **No TypeScript test-reach floor or CI gate yet.** Neither tstodon nor
  web-ui runs coverage in dagayn's CI, so a floor would gate nothing.
  Revisit when a TypeScript corpus runs coverage in CI (#293).

## Decisions (2026-11-04)

<!-- derived-from ../../eval/g7_go/RESULTS.md#defects-in-dagayns-go-answers -->
<!-- constrained-by ./SCIP-CALL-RESOLUTION.md#freezing-the-inference-layer -->

Taken by the owner after G7.

- **11.1.0's goal is Go first-class.** It reruns the Go agent experiment
  as G8 ([pre-registration](../../eval/g8_go/PREREG.md)) and fixes #244,
  #257, #262, and #265.
- **G7's defects 1 to 4 are fixed in 11.0.0.** A method expression is a
  reference that `callers_of` lists
  ([#260](https://github.com/manji-0/dagayn/pull/260)). `tests_for` skips
  test doubles that share the method's name
  ([#258](https://github.com/manji-0/dagayn/pull/258)). A call without a
  selector never binds to a method
  ([#250](https://github.com/manji-0/dagayn/pull/250)). Only `_test.go`
  files hold Go tests, and `tests_to_run` names runnable tests
  ([#270](https://github.com/manji-0/dagayn/pull/270)). None of them was
  measured with agents.
- **Defect 5 is not fixed.** A receiver that comes from a call's result or
  a function parameter, and a call through an interface, need
  receiver-type inference, which this plan freezes
  ([work list](#work-list)).
- **G8 decides.** Its primary test is eight new questions, drawn from the
  keys before any dagayn answer to them existed; G7's eight questions rerun
  as a report-only comparison. The bar is G7's. If G8 misses it, the
  defects its runs show become the rest of the Go work.

## Decisions (2026-11-04, after G8)

<!-- derived-from ../../eval/g8_go/RESULTS.md#where-dagayn-won-and-lost -->
<!-- derived-from ../../eval/g8_go/RESULTS.md#defects-in-dagayns-go-answers -->
<!-- derived-from #decisions-2026-11-04 -->

Taken by the owner after G8.

- **G8 missed the bar.** Agents with dagayn used 1.20x the tokens (Holm
  p = 0.0003) and 0.97x the time, with 73 exact answers against 79. The
  token loss came from the questions that the baseline answered in 3 to 5
  tool calls (C6, C8, T6, T7, T8: medians of 3.5 to 4.5 tool calls). dagayn
  adds a fixed cost of 2 calls, the `ToolSearch` that loads its schemas and
  the dagayn call, and the agents still repeated the baseline's grep.
- **Fixes since G8.** [#278](https://github.com/manji-0/dagayn/pull/278)
  finds a Go method written `(*T).M` (#275).
  [#279](https://github.com/manji-0/dagayn/pull/279) pages a cut
  `callers_of` list and points `tests_for` at the callers walk.
  [#277](https://github.com/manji-0/dagayn/pull/277) records Go functions
  used as values and lists them in `callers_of` (#235).
  [#280](https://github.com/manji-0/dagayn/pull/280) makes a complete Go
  `callers_of` say what it checked (G8 defect 5).
  [#282](https://github.com/manji-0/dagayn/pull/282) gives the lines of a
  few unbound calls. [#283](https://github.com/manji-0/dagayn/pull/283)
  binds calls within a package at `HIGH`. The go extractor is now 17.
- **Two exploratory pilots, not registered.** Both ran G8's questions C6,
  C8, T6, and T7, 5 runs per arm, on two candidate builds: #280 alone, and
  #280 with #282 and #283. The median token ratios (dagayn / none) were:

  | Q | G8 (11.0.0, 10 runs) | pilot 1 (#280) | pilot 2 (#283) |
  |---|---|---|---|
  | C6 | 1.37 | 0.96 | 1.27 |
  | C8 | 1.39 | 1.07 | 0.76 |
  | T6 | 1.42 | 1.21 | 0.95 |
  | T7 | 1.27 | 1.54 | 1.54 |
  | geometric mean | 1.36 | 1.18 | 1.09 |

  These numbers are exploratory. They come from the four questions that
  the fixes were tuned against, with 5 runs per arm, and the baseline's
  own medians moved between pilots (C8: 106k, 128k, 137k tokens). They
  decide nothing. Raw runs are outside the repository (the session's
  scratchpad `d5/pilot*/`).
- **The bar changes. This is a redefinition of the goal by the owner, made
  after G8 missed it.** G7 and G8 tested 0.85x pooled over all their
  questions. From G9 on, 0.85x is tested only on LONG questions, those of
  the kinds where the baseline needs many steps. SHORT questions get a
  guard instead of a gain requirement: pooled over them, the token ratio is
  at most 1.10 and the exact answers are not fewer than the baseline's.
  G8 stays "not met" under the bar it registered. Under G9's rule, G8's own
  questions would split into 3 LONG (1.10x tokens) and 5 SHORT (1.26x), so
  G8 would not meet the new bar either (report only, from G9's
  `analyze.py` on G8's runs).
- **G9 decides.** New questions only, drawn by a seeded rule before any
  dagayn answer to them exists, on testify, compress, and kong v1.15.0 (a
  third Go repository with history). The LONG and SHORT kinds are fixed
  from G7's and G8's baseline turn counts before the draw
  ([pre-registration](../../eval/g9_go/PREREG.md)).

## Decisions (2026-11-05, after G9)

<!-- derived-from ../../eval/g9_go/RESULTS.md#short-one-more-request-than-the-baseline -->
<!-- derived-from #decisions-2026-11-04-after-g8 -->

Taken by the owner after G9, before G10's questions are drawn (#292).

- **The SHORT guard changes from 1.10 to 1.25. This is a change of the bar
  by the owner, made after G9 missed it.** From G10 on, pooled over the
  SHORT questions, the token ratio (dagayn / none) is at most 1.25 and the
  exact answers are not fewer than the baseline's. The LONG bar (0.85x
  tokens or time at Holm p < 0.05) does not change. G9 stays "not met"
  under the bar it registered.
- **Why.** Every dagayn run pays two requests before it learns anything:
  the `ToolSearch` that loads the schemas and the dagayn call, a median of
  45k tokens in G9 (#308). The medians of G9's four SHORT questions sum
  to 632k tokens with dagayn and 555k without (1.14x; the registered
  pooled ratio was 1.17x). With 45k taken off each question's dagayn
  median the sum is 452k, 0.81x: the fixed cost alone adds about 0.32x. A 1.10
  guard therefore fails a SHORT question even when dagayn's answer saves
  work. 1.25 passes G9's measured state with a margin of 0.08 and still
  fails if the fixed cost grows or the confirming grep after a complete
  answer gets worse.
- **Changes since G9 that G10 will measure.** #304 (a complete Go
  `callers_of` no longer says callers may be missing), #315/#318 (a
  stable `nearest_test`), and #307/#319 (a name that matches up to five
  nodes is answered per candidate). An exploratory pilot of #319 on K3,
  T10, and T11 (5 runs per arm, not registered) answered 5 of the 6
  first calls that would have been a retry; K3 stayed at 1.30x because
  every run grepped the name after the answer.
