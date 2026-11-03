# Results: G7, Go questions a short script cannot answer

<!-- derived-from ./PREREG.md -->

The questions, keys, scoring, arms, and analysis were registered in commit
30d7d6d74358077d3aed2e2d3655c7289058201a, pushed to
`g7-go-agent-experiment` before any run. `PREREG.sha256` matches every
registered file; nothing changed after registration.

## Verdict

**The bar is not met.** Pooled over the eight questions, agents with
dagayn used 1.00x the tokens of agents without it (Holm p = 1.0) and 0.91x
the time (Holm p = 0.21). Accuracy was equal: 78 of 80 answers were exact
with dagayn and 74 of 80 without (F1 +0.003, Holm p = 1.0). The bar asked
for 0.85x tokens or time at Holm p < 0.05. Go is not first-class by the
plan's definition. G1 to G5 and M2 stand.

## What ran

- 2026-11-03 (JST). 160 runs: 8 questions, 2 arms, 10 runs each, Claude
  Haiku 5.5, `--effort medium`, as registered. No run failed, timed out,
  or lost its MCP server, so the sensitivity analysis (`--include-failed`)
  equals the primary one.
- The CLI's estimate of cost: 1.94 USD (0.95 with dagayn, 0.99 without),
  under the 25 USD cap.
- Every candidate graph was built at go extractor 12, and no candidate
  run's graph metadata changed during the run.
- No run executed the tests or the repository's code. Five runs wrote a
  `go/ast` or `go/types` program under `/tmp` and ran it on the checkout,
  which the prompt allows: C4 027 and 036 and T2 053 and 071 without
  dagayn, T3 031 with dagayn (`go run main.go <repo>` in `/tmp/tcalls`).
- Raw transcripts and the key inputs (scip-go dumps, function lists,
  per-test coverage):
  `/Users/manji0/src/dagayn/dagayn-video/experiment_g7_haiku/` (not
  committed). `results/` holds each batch's `results.csv` and
  `summary.json`, and `analysis.json`.

## Pooled (registered test)

| Measure | dagayn / none | p | Holm p |
|---|---|---|---|
| Total tokens | 1.00x | 0.96 | 1.0 |
| Time | 0.91x | 0.070 | 0.21 |
| F1 | +0.003 | 0.57 | 1.0 |

| Bar check | Result |
|---|---|
| tokens at most 0.85x, Holm p < 0.05 | no (1.00x, 1.0) |
| time at most 0.85x, Holm p < 0.05 | no (0.91x, 0.21) |
| exact not 3 or more fewer | yes (78 / 80 against 74 / 80) |

Per repository: testify tokens 0.97x, time 0.94x (Holm p = 1.0 for both);
compress tokens 1.02x (Holm p = 0.69), time 0.88x (Holm p = 0.15).

## Per question

Medians of 10 runs. Holm over the eight questions, per measure. "Checks"
are Bash, Read, Grep, and Glob calls after the first dagayn call.

| Q | Exact (dagayn / none) | Tokens (dagayn / none) | Ratio, Holm p | Time ratio, Holm p | dagayn calls | Checks | dagayn's first answer |
|---|---|---|---|---|---|---|---|
| T1 test reach | 9 / 9 | 184k / 169k | 1.13x, 1.0 | 1.04x, 1.0 | 1 | 6 | 0 / 7 |
| T2 receiver | 9 / 9 | 359k / 427k | 0.93x, 1.0 | 0.76x, 0.43 | 1.5 | 7.5 | 9 / 9 and itself |
| T3 interface | 10 / 10 | 165k / 189k | 0.97x, 1.0 | 1.11x, 1.0 | 1 | 3 | 13 / 14 |
| T4 unedited | 10 / 10 | 134k / 154k | 0.87x, 1.0 | 0.90x, 1.0 | 1 | 0 | 12 rows = 10 + 2 edited |
| C1 test reach | 10 / 10 | 713k / 645k | 1.11x, 1.0 | 1.00x, 1.0 | 2 | 17 | 4 / 4 among 65 tests |
| C2 receiver | 10 / 10 | 187k / 196k | 0.92x, 1.0 | 0.69x, 0.066 | 1 | 4.5 | 10 / 10 |
| C3 method value | 10 / 10 | 152k / 102k | **1.36x, 0.022** | 1.07x, 1.0 | 2 | 4 | 0 / 3 |
| C4 unedited | 10 / 6 | 261k / 342k | 0.79x, 0.61 | 0.83x, 0.43 | 1 | 4.5 | 11 rows = 9 + 2 edited |

## The wrong answers

Every inexact answer misses entries of the key, and none adds one. Each
missed entry was read again against the source, and the keys stand.

- **C4 without dagayn (4 of 10): `huff0/compress_test.go::BenchmarkDeflate`
  missed.** Run 006 lists the other nine with the declaration of each
  receiver and does not mention huff0. The writer there is named `dec`
  (`dec, err := flate.NewWriter(...)`). dagayn's `callers_of` lists it, and
  all 10 runs with dagayn have it.
- **T2, one run per arm: the six `assert := New(t)` tests missed.** Both
  read `assert.Equal(...)` in package `assert` as the package function. The
  dagayn run (005) had those six rows from `callers_of` and dropped them as
  "false positives" after reading the call sites.
- **T1, one run per arm: `TestEventuallyWithTFailNow` missed**, the test
  whose outer `t` is a `*CollectT`.

## Where dagayn won and lost

The pattern is M3's. dagayn saved tokens where its answer was complete and
the question was about types. It cost tokens where its answer was empty,
or where the right rows were buried in many wrong ones.

- **C3 (1.36x, the one significant result): an empty answer for a call
  through a field.** `callers_of storeFast` returned 0 rows marked
  complete. Run 001 grepped `storeFast`, then called dagayn, grepped
  `.step`, and read `deflate.go`. The baseline answered in a median of 5.5
  turns, so the ToolSearch and the dagayn calls were a third more tokens.
- **C1 (1.11x) and T1 (1.13x): test reach that is not execution.** On C1,
  `callers_of(depth=6)` listed 65 tests, with the 4 that execute
  `statelessEnc` among them. The agents then read gzip, gzhttp, and flate
  tests to find the level -3 paths (17 checks, the most of any question).
  On T1, `tests_for` returned the other types' `Errorf` methods, and the
  agents read the `EventuallyWithT` tests as the baseline did.
- **C4 (0.79x tokens, 10 / 10 exact against 6 / 10): the clearest gain,
  and not significant.** One `git show` and one `callers_of` gave 11 rows.
  The agents dropped the two edited callers and found the missing
  `testResetOutput` in their checks (run 004 grepped the `.Reset(` calls in
  the files that import flate and read each receiver). The baseline built
  the list from that grep alone and four times missed the writer named
  `dec`.
- **C2 (0.69x time, Holm p = 0.066) and T2 (0.76x time): right answers,
  checked anyway.** dagayn's 10 / 10 on C2 still drew a median of 4.5
  checks (run 003's first was a grep of every `.Reset(` call). T2's answer
  had one wrong row (the method itself), and a median of 7.5 checks
  followed.
- **T4 (0.87x, 0 checks) and T3 (0.97x, 3 checks).** On T4 the 12 callers
  less the 2 the diff edits was the answer, and the median run made no
  check. On T3 dagayn's 13 rows lacked only the interface call in `Run`,
  and all 10 runs added it.

## Defects in dagayn's Go answers

These were seen at registration (PREREG) and are confirmed by the runs.
They are recorded for the plan, not fixed here.

1. **A call through a field that holds a method expression is not an
   edge** (C3: 0 rows, `results_complete: true`). This was M3's first gap.
   The reply could name where the graph saw the method used as a value
   (`d.step = (*compressor).storeFast` in `init`), which would replace the
   grep with one read.
2. **`tests_for` on a Go method returns other types' methods of the same
   name** (T1: 10 `Errorf` methods of test doubles, no test).
3. **A method's call to the package function of its own name binds to the
   method**: `Assertions.Equal` is its own caller (T2), and
   `Assertions.Fail` is missing from `Fail`'s callers.
4. **`review_tool`'s `tests_to_run` names helpers and benchmarks from
   `_test.go` files** with `go test -run` commands that run nothing
   (`benchmarkEncoder`, `testFileLevelDictReset`, `BenchmarkDeflate`).
5. **A receiver that comes from a function parameter is not resolved**
   (C4: `testResetOutput`'s `w, err := newWriter(buf)`), and an interface
   call is not an edge (T3: `Run`'s `suite.T()`).

## What can and cannot be said

- Can: on these eight Go questions, an agent with dagayn was as costly in
  tokens as one without, and 0.91x as slow (not significant). The two
  arms were equally accurate.
- Can: dagayn's answer saved tokens and errors on the two questions about
  receiver types and diffs where its answer was complete or nearly
  complete (C4 0.79x, T4 0.87x, C2 0.92x). It cost tokens where its answer
  was empty (C3 1.36x) or listed many tests that do not run the function
  (C1, T1).
- Cannot: that Go questions in general gain nothing. The eight were chosen
  by kind, after dagayn's answers were seen at registration.
