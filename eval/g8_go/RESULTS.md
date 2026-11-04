# Results: G8, the Go agent experiment rerun

<!-- derived-from ./PREREG.md -->

The questions, keys, scoring, arms, and analysis were registered in commit
3c3f5e8dd78400c6fe039d2dc8fcd08ff0f527f3 (`eval/g8-go-prereg`, merged as
#273 in b681bd1e) before any run. `PREREG.sha256` matches all 26 registered files, and
nothing in the registration changed after it. The deviations in how the
runs were invoked are listed under [What ran](#what-ran).

## Verdict

**The bar is not met.** Pooled over the eight primary questions, agents
with dagayn used 1.20x the tokens of agents without it, and the difference
is significant in the wrong direction (Holm p = 0.0003). Time was 0.97x
(Holm p = 0.38). Agents with dagayn gave 73 exact answers out of 80, and
agents without it gave 79 (F1 -0.010, Holm p = 0.075). The bar asked for
0.85x tokens or time at Holm p < 0.05, with no more than 2 fewer exact
answers. Both parts fail. Go is not first-class by the plan's definition.

G7's eight questions, rerun on the same build (report only), moved from
1.00x to 1.07x tokens (Holm p = 0.27) and from 0.91x to 0.89x time (Holm
p = 0.0075), at 74 against 73 exact.

## What ran

- 2026-11-04 (JST). The plumbing check ran first, then the primary pair
  from 14:03 to 14:28, then the secondary pair from 14:29 to 14:53. There
  were 320 runs: 16 questions, 2 arms, 10 runs each, Claude Haiku 5.5,
  `--effort medium`, `-j 3`, timeout 1,200 s, with the registered seeds.
- Plumbing check: 1 run per arm per target, on the methods of `CollectT`
  (7) and the exported methods of xxhash's `Digest` (9). All four answers
  were exact, the candidate's MCP server reported `dagayn:connected`, and
  both graphs were full builds at go 15. On each target, `HEAD~1..HEAD`
  was the registered change commit. These runs are not part of the
  results.
- No run failed, timed out, or lost its MCP server. The sensitivity
  analysis (`--include-failed`) is therefore identical to the primary one.
- Every candidate graph was a full build at go extractor 15, with no SCIP
  overlay (160 of 160). No candidate run's graph metadata changed during
  the run. The candidate's `_core` had the same SHA-256 (`9492e39e...`)
  and reported go 15 both before and after the runs.
- The CLI's estimate of cost was 4.21 USD: 2.19 for the primary set (1.23
  with dagayn, 0.96 without) and 2.02 for the secondary set (1.00 and
  1.02). The plumbing check cost 0.02 USD. All of it was under the 25 USD
  cap.
- No run executed the tests. No run in the primary set ran any `go`
  command. In the secondary set, four runs wrote a `go/ast` or `go/types`
  program under `/tmp` and ran it on the checkout, which the prompt
  allows: T2 030 and 071 and C4 027 without dagayn, and C4 029 with
  dagayn.
- Raw transcripts are under
  `/Users/manji0/src/dagayn/dagayn-video/experiment_g8_haiku/runs/` (not
  committed). `results/` holds each batch's `results.csv` and
  `summary.json`, `analysis.json` and `analysis_include_failed.json` (the
  primary set), and `analysis_g7_rerun.json`.

Deviations from the registered procedure. None of them changes the
questions, keys, arms, or analysis.

- **Candidate executable.** The candidate was
  `.claude/worktrees/agent-a723154d7e156cb5a/.venv/bin/dagayn`. This is
  the registering worktree's editable build on Python 3.14, and it is the
  same build that produced `dagayn_answers.json`. The registration named
  the `dagayn` uv tool reinstalled with `--python 3.13`. The installed uv
  tool was a non-editable build from the main checkout at an unrecorded
  commit. Reinstalling it would also have changed the dagayn that other
  sessions on this machine run. The worktree's tree matches eab069d7
  under `dagayn/`, `crates/`, and the lock files (`git diff eab069d7` is
  empty). Its `_core` was built from that tree before the first
  registration commit, and it reports go 15.
- **Invocation.** The harness and `analyze.py` ran under the worktree's
  `.venv/bin/python` instead of `uv run python`, so that no `uv sync`
  could rebuild the candidate mid-run. The harness file is the
  registered one. The source repositories were the clones under
  `build/g8/`.

## Pooled (registered test, primary questions)

| Measure | dagayn / none | p | Holm p |
|---|---|---|---|
| Total tokens | **1.20x** | 0.0001 | **0.0003** |
| Time | 0.97x | 0.38 | 0.38 |
| F1 | -0.010 | 0.038 | 0.075 |

| Bar check | Result |
|---|---|
| tokens at most 0.85x, Holm p < 0.05 | no (1.20x, more tokens, Holm p = 0.0003) |
| time at most 0.85x, Holm p < 0.05 | no (0.97x, 0.38) |
| exact not 3 or more fewer | no (73 / 80 against 79 / 80) |

Per repository: testify used 1.19x tokens (Holm p = 0.012) and 0.95x
time (Holm p = 0.24). Compress used 1.21x tokens (Holm p = 0.0036) and
1.00x time (Holm p = 1.0).

## Per question (primary)

Each value is the median of 10 runs. The Holm correction is over the
eight questions, per measure. "Checks" counts the Bash, Read, Grep, and
Glob calls after the first dagayn call. "First call failed" counts the
runs whose first dagayn call returned `not_found` or `ambiguous`.

| Q | Exact (dagayn / none) | Tokens (dagayn / none) | Token ratio, Holm p | Time ratio, Holm p | Turns (dagayn / none) | dagayn calls | Checks | First call failed | dagayn's registered first answer |
|---|---|---|---|---|---|---|---|---|---|
| T5 test reach | 7 / 10 | 1.12M / 1.06M | 1.13x, 0.61 | 1.03x, 1.0 | 22.5 / 22.5 | 1 | 16 | 0 | 15 / 15 among 77 tests |
| T6 receiver | 10 / 10 | 168k / 118k | 1.31x, 0.13 | 0.88x, 1.0 | 7.5 / 5 | 1 | 4 | 4 | 10 / 10 |
| T7 interface | 10 / 10 | 128k / 100k | 1.17x, 0.39 | 0.94x, 1.0 | 7 / 5 | 1 | 3.5 | 3 | 2 / 3 |
| T8 unedited | 6 / 9 | 164k / 152k | 1.14x, 0.61 | 0.95x, 1.0 | 6 / 5.5 | 1 | 0 | 1 | 15 rows = 13 + 2 edited |
| C5 test reach | 10 / 10 | 239k / 269k | 0.85x, 0.45 | **0.72x, 0.039** | 13 / 13 | 2.5 | 6.5 | 8 | 3 / 3 in 6 rows |
| C6 receiver | 10 / 10 | 128k / 93k | 1.35x, 0.12 | 1.10x, 1.0 | 6.5 / 5.5 | 1 | 3 | 0 | 3 / 3 |
| C7 function value | 10 / 10 | 257k / 184k | **1.39x, 0.0096** | 1.14x, 0.64 | 12.5 / 9 | 1 | 8.5 | 3 | 1 / 3 |
| C8 unedited | 10 / 10 | 148k / 106k | 1.33x, 0.086 | 1.08x, 1.0 | 7 / 4.5 | 1 | 0 | 1 | 6 rows = 4 + 2 edited |

## The wrong answers

Every key was checked again against the source and the per-test coverage.
All of them stand, so nothing is scored a second way.

- **T8, 4 runs with dagayn and 1 without: `require/require.go::YAMLEq`
  missed.** `require.YAMLEq` calls `assert.YAMLEq` (`require.go:2127`).
  Eight of the ten dagayn runs got it as a row of `callers_of`. Runs 003
  and 061 still dropped it. Run 047's only call was `ambiguous`, and run
  012 made no dagayn call. Their stated reason was that it is "a function of the same name in
  another package". The prompt does exclude calls to other packages'
  functions of the same name, and the agents read that rule as excluding a
  caller that only shares the name. Runs 003 and 047 also added the ten
  `TestYAMLEqWrapper_*` tests from their own grep. Those tests call the
  method on `assert := New(...)`, not the function. dagayn had not listed
  them.
- **T5, 3 runs with dagayn (004, 031, 050):
  `Test_Mock_On_WithArgMatcherThatPanics` missed.** These runs argued that
  the matcher's panic stops the call before `diffArguments`. In fact,
  `Arguments.Diff` recovers a matcher's panic as a mismatch
  (`mock.go:993-996`), so the closest call exists and `diffArguments`
  runs before `fail` panics. Both coverage runs record it. dagayn's
  `callers_of(depth=6)` at `detail_level="minimal"` kept 21 of 87 rows.
  These runs then read `mock_test.go` whole and reasoned about each test,
  as the baseline does.

## Where dagayn won and lost

The pattern is G7's and M3's, and it is stronger here. Every question
whose baseline answered in a median of 5.5 turns or fewer cost more with
dagayn, even where dagayn's answer was complete.

- **C7 (1.39x, Holm p = 0.0096): a function value called through a
  field.** `callers_of gzkp NewWriter` returned 1 of 3 callers with
  `results_complete: true`. The summary said that "calls dispatched at
  run time are not edges", and it did not say that `NewWrapper` stores
  the function in `GzipWriterFactory.New`. The agents then searched for
  the factory by grep (a median of 8.5 checks, the most after T5). All 20
  runs, in both arms, were exact.
- **C6 (1.35x), T6 (1.31x), C8 (1.33x): complete answers cost turns
  anyway.** dagayn's answer was complete on C6 and T6 (3 / 3 and
  10 / 10 rows). It was also complete on C8 once the diff's own
  callers were removed. The baseline answered with one grep and one read
  (4.5 to 5.5 turns). The dagayn arm added the tool-schema load
  (`ToolSearch`) and the dagayn call, and on C6 and T6 it then made a
  median of 3 to 4 checks that repeat the baseline's grep.
- **First calls that named the method as Go writes it.** In 20 of the 80
  dagayn runs, the first call failed. Thirteen of those wrote the
  receiver in Go's spelling: `zip/reader.go::(*File).OpenRaw` was
  `not_found`, and `(*File).OpenRaw` and `(*Suite).SetT` were `ambiguous`,
  with unrelated candidates (C5 8, T7 3, T6 2). The other seven used a
  bare name or a file path that does not start at the repository root
  (`gzkp.go::NewWriter`, C7 3).
- **C5 (0.85x tokens, 0.72x time, Holm p = 0.039): the one gain.**
  `callers_of(depth=6)` listed the 3 tests among 6 rows, even after a
  failed first call in 8 runs. The baseline traced `OpenRaw` through
  `readTestFile` and `(*Writer).Copy` by hand in the same number of
  turns.
- **T5 (1.13x, 7 against 10 exact): a long, truncated list.** The
  baseline read `mock_test.go` (a median of 22.5 turns in both arms). The
  dagayn arm read it too after the call (16 checks), and three of its runs
  reasoned their way out of a test that coverage shows.

## Secondary: G7's questions on the same build (report only)

This rerun used G7's tasks, keys, and seeds on the 11.0.0 candidate. It
does not test the bar. G7's numbers (go extractor 12) are in brackets.

| Measure | dagayn / none | Holm p |
|---|---|---|
| Total tokens | 1.07x [1.00x] | 0.27 [1.0] |
| Time | 0.89x [0.91x] | 0.0075 [0.21] |
| F1 | +0.008 [+0.003] | 0.32 [1.0] |
| Exact | 74 / 73 [78 / 74] | |

| Q | Exact (dagayn / none) | Token ratio, Holm p | Time ratio, Holm p | Checks | First call failed |
|---|---|---|---|---|---|
| T1 test reach | 5 / 10 [9 / 9] | 1.15x, 1.0 [1.13x] | 0.94x, 1.0 [1.04x] | 6.5 | 7 |
| T2 receiver | 10 / 9 [9 / 9] | 0.91x, 1.0 [0.93x] | 0.74x, 0.33 [0.76x] | 8.5 | 4 |
| T3 interface | 10 / 10 [10 / 10] | 1.13x, 1.0 [0.97x] | 0.98x, 1.0 [1.11x] | 5.5 | 1 |
| T4 unedited | 10 / 10 [10 / 10] | 0.95x, 1.0 [0.87x] | 0.81x, 0.33 [0.90x] | 0 | 0 |
| C1 test reach | 10 / 7 [10 / 10] | 1.19x, 1.0 [1.11x] | 0.99x, 1.0 [1.00x] | 17 | 0 |
| C2 receiver | 10 / 10 [10 / 10] | 0.86x, 1.0 [0.92x] | 0.83x, 0.41 [0.69x] | 5 | 0 |
| C3 method value | 10 / 10 [10 / 10] | **1.76x, 0.0016** [1.36x, 0.022] | 1.20x, 0.16 [1.07x] | 4 | 9 |
| C4 unedited | 9 / 7 [10 / 6] | 0.84x, 1.0 [0.79x] | 0.73x, 0.11 [0.83x] | 7 | 6 |

- **The C3 fix (#260) did not reach the agents.** In 9 of 10 runs, the
  first call was `callers_of flate/deflate.go::(*compressor).storeFast`,
  which returned `not_found`. The row that #260 added (`init`, named as a
  use of the value) was never shown to them. C3 went from 1.36x to 1.76x.
- **T1 lost accuracy (5 against 10 exact).** After #258, `tests_for`
  returns no rows. `callers_of(depth=6)` on `CollectT.Errorf` found 505
  rows and showed 63. Five runs missed `TestEventuallyWithTFailNow`
  (G7: 1 run per arm). Seven first calls were `ambiguous`, including
  `CollectT.Errorf`, which names a single node.
- **C4 and T2 kept their gains.** C4 was at 0.84x tokens and 0.73x time,
  with 9 against 7 exact. T2 was at 0.74x time, and the wrong row from
  defect 3 is gone.

## Defects in dagayn's Go answers

These are the defects the runs show. They are recorded for the plan and
are not fixed here.

1. **A Go method named in Go's own spelling is not found**
   ([#275](https://github.com/manji-0/dagayn/issues/275)).
   `path::(*T).M` returns `not_found`. `(*T).M` and a `T.M` that names one
   node return `ambiguous`, and the other candidates are that node's
   callers or neighbours. For example, `CollectT.Errorf` offers
   `EventuallyWithT`, and `Suite.SetT` offers `Suite.Run` and
   `TestingSuite`. 13 of 80 primary first calls and 21 of 80 secondary
   first calls failed this way. This failure hid the C3 fix.
2. **`callers_of` on a function stored in a field does not say where it
   is stored** (C7, [#235](https://github.com/manji-0/dagayn/issues/235)).
   It returns 1 of 3 callers with `results_complete: true`. The two calls
   through `GzipWriterFactory.New`, and the assignment in `NewWrapper`
   that sets it up, are absent from the reply.
3. **An interface call is not an edge** (T7: `Run`'s `suite.SetT` on a
   `TestingSuite`; G7 defect 5,
   [#224](https://github.com/manji-0/dagayn/issues/224),
   [#255](https://github.com/manji-0/dagayn/issues/255)). The plan freezes
   the receiver-type inference that this needs.
4. **Test reach at depth 6 is a long list that is cut short.** On T5,
   `detail_level="minimal"` keeps 21 of 87 rows. On G7's T1 it keeps 63 of
   505 rows. `tests_for` returns 0 rows on both. The agents then read the
   test files whole, at the baseline's cost.
5. **A complete answer does not end the search.** On C6 and T6 the reply
   was complete and said `results_complete: true`. The agents still made a
   median of 3 to 4 checks. On these questions, the schema load and the
   call cost more turns than the baseline's single grep. This item is an
   inference from the turn counts. No run says why it checked.

## What can and cannot be said

- What can be said: on eight Go questions drawn by a fixed rule, an agent
  with dagayn 11.0.0 used more tokens than one without it (1.20x,
  significant), took the same time, and was less often exact (73 against
  79).
- What can be said: the one gain was a test-reach question whose
  depth-6 callers list was short (C5: 0.72x time). The largest cost was
  a call through a stored function value, where dagayn's answer was 1 of
  3 (C7: 1.39x).
- What can be said: on G7's questions, 11.0.0's fixes did not move the
  pooled tokens (1.00x to 1.07x). The C3 fix was hidden by the target
  spelling (defect 1).
- What cannot be said: that Go questions in general cost more with
  dagayn. The eight questions are a draw from rule-defined pools, not a
  sample of Go work. The T8 prompt's rule on same-name functions in other
  packages misled agents in both arms, more often with dagayn (4 runs)
  than without it (1 run).
