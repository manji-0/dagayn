# Go support measurements

<!-- derived-from ./TEST-REACH-TARGET.md#evaluation -->

## Question

What does dagayn answer about Go code, measured against coverage, before
and after Go tests and Go packages are in the graph? These are the
measurements of work items M2, G1, and G2 of the first-class languages plan
(`docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md`, PR #217). Each item ships
only if it meets the bar the plan sets for it.

## Corpora

| Repository | Version | Source | Go | Coverage run |
|---|---|---|---|---|
| testify | v1.12.1 | copy of the Go module cache, no history | 1.24.13 | `assert`, `require`, `internal` |
| compress | v1.20.1 (`5d880f2`) | clone of `github.com/klauspost/compress` | 1.25.3 | all packages, `-short` |
| scip-go | v0.2.7 | clone of `github.com/sourcegraph/scip-go` | 1.25.3 | all packages |

Every run is `go test -coverpkg=<the measured packages> -coverprofile=...`
then `go tool cover -func`, so a function counts as run when any test
package ran any of its statements. testify's run leaves out `http`, `mock`,
and `suite` (their dependencies are not in the module cache, and `suite`
panicked under Go 1.24) and skips `TestDirExists` and `TestNoDirExists`,
which fail outside the source tree, as the plan's baseline did. scip-go's
`internal/git` `TestInferRepo` fails in a clone without the upstream remote;
the other packages' coverage stands. Go 1.25.3 came from
`GOTOOLCHAIN=go1.25.3` (the module cache's `golang.org/toolchain`). scip-go
v0.2.7 is the last tag that builds with Go 1.25; v0.2.8 needs 1.26.

Graphs were built with `dagayn build --repo COPY --force` on copies in a
scratch directory. Before each measurement the graph's metadata was
checked: `last_build_type` `full` and the `go` extractor version of the
build under test.

## M2: test reach against Go coverage

<!-- derived-from ./TEST-REACH-TARGET.md#evaluation -->

`eval/run_test_reach_eval.py` reads a `go tool cover -func` listing
(`--coverage cover.func --repo GO_REPO`). A listing line names the
package's import path, the file, and the line of the `func` keyword; the
harness maps the import path to a directory through the repository's
`go.mod` files and matches a Go `Function` node on file and first line. A
function in a file the build left out (another `GOARCH`'s `_amd64.go`, a
build tag) is in no listing and so in no population.

Bar: the harness reproduces the plan's testify baseline and runs on a second
repository with git history.

| Repository (go extractor 8) | Functions | Called untested | Ran, called untested | `no_callers` / `no_test_path` / `beyond_limit` (ran) | Untested precision | Recall | Tested precision |
|---|---|---|---|---|---|---|---|
| testify | 735 | 720 | **277** of 292 | 111 / 166 / 0 | 0.615 | 1.000 | 1.000 (15) |
| compress | 1,150 | 946 | 786 of 978 | 425 / 355 / 6 | 0.169 | 0.930 | 0.941 (204) |
| scip-go | 137 | 137 | 110 of 110 | 41 / 69 / 0 | 0.197 | 1.000 | none |

testify matches the plan exactly (735, 720, 277 = 111 + 166). compress is
the second repository: 1,150 of the 1,155 functions its listing names match
a node. Met. No floor is set: Go has no CI coverage run, and these numbers
only show that almost everything is called untested.

## G1: Go tests in the graph

<!-- derived-from #m2-test-reach-against-go-coverage -->

Go extractor 9: every node of a `_test.go` file is test code, and its
top-level `TestX`, `BenchmarkX`, `FuzzX`, and `ExampleX` functions are
`Test` nodes whose calls give `TESTED_BY` edges.

| Repository | `Test` nodes | `is_test` nodes | `TESTED_BY` | Ran, called untested (8 → 9) |
|---|---|---|---|---|
| testify | 0 → 479 | 0 → 813 | 0 → 1,343 | 277 → 277 |
| compress | 0 → 805 | 0 → 1,148 | 0 → 932 | 786 → 784 |
| scip-go | 0 → 35 | 0 → 65 | 0 → 2 | 110 → 110 |

Alone, G1 moves test reach by two functions, as the plan expected: a Go
test calls code of another file of its package by its bare name, and
nothing binds such a call yet, so the `TESTED_BY` edges that survive name
helpers of the test's own file. The review case `untested_change_go`
passes; `tests_to_run_go` and the negative `neg_untested_reached_go` need
the calls bound, and are measured with G2.
