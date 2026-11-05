# Results: G10, the Go agent experiment on new questions and an untuned repository

<!-- derived-from ./PREREG.md -->

The questions, keys, scoring, arms, transport, and analysis were registered
on `eval/g10-prereg` (registering commit b2c0265a, merged as #323 in
1aef1166) before any run. `PREREG.sha256` matches all 30 registered files,
and nothing in the registration changed after it. The deviations in how the
runs were invoked are listed under [What ran](#what-ran).

G10 measured dagayn through its CLI (`dagayn tool ...` in the shell), with no
MCP server, against a baseline with no dagayn. Nothing here is a result about
the MCP transport.

## Verdict

<!-- constrained-by ./PREREG.md#the-bar -->

**The bar is not met.** Neither part holds.

- **LONG (8 questions): not met.** Agents with dagayn used 1.03x the tokens
  and 0.98x the time of agents without it (Holm p = 1.0 for both). They
  gave 75 exact answers out of 80, against 76, which passes the accuracy
  rule (not 3 or more fewer).
- **SHORT (4 questions): not met.** The token ratio was 0.92x, within the
  1.25 guard. Exact answers were 39 against 40, so the "not fewer" rule
  fails. The one inexact answer (H5, run 116) kept a row that dagayn's reply
  listed from `_examples/`, a directory outside the question's scope.
- **chi alone (secondary, decides nothing):** LONG tokens 1.14x and time
  1.12x (Holm p = 0.077 for both), exact 25 against 27. SHORT tokens 0.98x,
  exact 29 against 30.

The bar needs both parts. Go is not first-class by the plan's definition.

## What ran

<!-- derived-from ./PREREG.md#procedure -->

- 2026-11-05 (JST, from the run files' times). The plumbing check ran at
  20:56. testify and compress ran at the same time from 20:57 to 21:05,
  then kong (21:05 to 21:18) and chi (21:05 to 21:29) at the same time.
  There were 240 runs: 12 questions, 2 arms, 10 runs each, Claude Haiku
  5.5, `--effort medium`, `-j 3`, timeout 1,200 s, with the registered
  seeds.
- Plumbing check: 1 run per arm of `plumbing_chi.json` (the 12 methods of
  tree.go's `node`) on chi with its change commit. Both answers were exact.
  The candidate made 1 `dagayn tool` call, its init event listed no MCP
  server, and its graph was at go extractor 18. These runs are not part of
  the results.
- No run failed or timed out (240 of 240 have a result event and no
  error). No candidate init event listed an MCP server. The sensitivity
  analysis (`--include-failed`) is therefore identical to the primary one.
- Every candidate run made at least one `dagayn tool` call (120 of 120).
  No run called `ToolSearch`.
- The candidate was the `dagayn` uv tool at `$HOME/.local/bin/dagayn`,
  reinstalled from d8ecb51f with the 1.99.1 toolchain first on PATH, on
  Python 3.14. The operator checked it from outside a checkout and it
  reported go 18. Its `_core` had SHA-256 `fe67d82f...7c633542f7`, both
  before and after the runs (`core-sha-before.txt`, `core-sha-after.txt`).
  Every candidate graph was a full build at go extractor 18, with no SCIP
  overlay (120 of 120 `graph_before`). No candidate run's graph metadata
  changed during the run.
- Harness: `eval/run_agent_tokens_eval.py` from a detached checkout of
  main 1aef1166. Its SHA-256 is `ec5d7844...f9d25162`, the registered
  file (byte-identical to d8ecb51f's). It ran with `--candidate-transport
  cli`.
- The CLI's estimate of cost was 2.41 USD: 1.18 with dagayn and 1.23
  without. The plumbing check cost under 0.01 USD. All of it was under the
  25 USD cap.
- No run executed the tests or the repository's packages: no Bash command
  in the 240 runs ran `go test`, and no `go run`, `go build`, or `go vet`
  had a target in the repository. Five runs built and ran an analysis
  program that the agent wrote under `/tmp`. Each program read the
  repository's files as data with `go/parser`, and three of them also
  type-checked them with `go/types`. These runs are 058-H3 and 087-H3 with dagayn, and 077-H3,
  103-H3, and 100-H5 without. The prompt forbids `go run` by name and
  allows "writing and running scripts for the analysis". The registration
  says such runs are reported, not excluded, so they stay in. One run
  without dagayn (023-T14) ran `which go gopls`, which is a lookup, not a
  run. `gopls` was not on PATH.
- Raw transcripts are in the run directory's `out-g10-*/raw/` (not
  committed). `results/` holds each batch's `results.csv` and
  `summary.json`, `analysis.json`, and `analysis_include_failed.json`. The
  plumbing check's files are not included, as in G8 and G9.

Deviations from the registered procedure. None of them changes the
questions, keys, arms, or analysis.

- **Source repositories.** The registered commands name `build/g10/`.
  The runs used the clones that the registration's author built, in the
  session's scratch directory (`.../scratchpad/g10/build/{testify,compress,kong,chi}-src`).
  Each clone has the registered tag at the registered commit: testify
  v1.12.1 at 959dbdac, compress v1.20.1 at 5d880f23, kong v1.15.0 at
  14c97a2b, and chi v5.5.0 at 3a33ba57. The harness builds each run's
  target from `source_commit` (plus chi's patch), so the targets are the
  registered ones.
- **Interpreter.** The harness and `analyze.py` ran under a checkout's
  `.venv/bin/python` (Python 3.14.7), not `uv run python`, as in G9. The
  registration allows either. The candidate is the uv tool in both cases.
- **Invocation.** The registered commands ran from two `sh` scripts (the
  plumbing check, then the runs), because zsh does not split `$COMMON`.
  The arguments and order are the registered ones.

The registered analysis was re-run from the raw runs for this file. Both
outputs matched the committed `results/analysis.json` and
`results/analysis_include_failed.json` byte for byte.

## Pooled (registered test)

<!-- derived-from ./PREREG.md#hypotheses-and-test -->

LONG, the primary test (Holm over the three measures):

| Measure | dagayn / none | p | Holm p |
|---|---|---|---|
| Total tokens | 1.03x | 0.43 | 1.0 |
| Time | 0.98x | 0.59 | 1.0 |
| F1 | +0.004 | 0.92 | 1.0 |

SHORT, the guard (a point estimate):

| Measure | dagayn / none | p (report only) |
|---|---|---|
| Total tokens | 0.92x | 0.12 |

| Bar check | Result |
|---|---|
| LONG: tokens at most 0.85x, Holm p < 0.05 | no (1.03x, 1.0) |
| LONG: time at most 0.85x, Holm p < 0.05 | no (0.98x, 1.0) |
| LONG: exact not 3 or more fewer | yes (75 / 80 against 76 / 80) |
| SHORT: token ratio at most 1.25 | yes (0.92x) |
| SHORT: exact not fewer | **no** (39 / 40 against 40 / 40) |

Per repository (each repository's questions, secondary):

| Repository | Questions | Tokens, Holm p | Time, Holm p |
|---|---|---|---|
| testify | T13, T14 | 0.96x, 0.60 | 0.91x, 0.48 |
| compress | C13, C14 | **0.84x, 0.044** | **0.81x, 0.017** |
| kong | K5, K6 | 1.00x, 1.0 | 0.97x, 1.0 |
| chi | H1 to H6 | 1.06x, 0.55 | 1.01x, 0.78 |

chi alone (secondary): LONG (H1, H2, H3) tokens 1.14x (p = 0.035, Holm
p = 0.077), time 1.12x (p = 0.026, Holm p = 0.077), F1 -0.0004, exact 25
against 27. SHORT (H4, H5, H6) tokens 0.98x (p = 0.77), exact 29 against
30.

## Per question

Each value is the median of 10 runs. The Holm correction is over the
twelve questions, per measure. "Turns" is the CLI's `num_turns`. "Checks"
counts the Bash, Read, Grep, and Glob calls after the first dagayn call.
No first dagayn call failed: there was no `ambiguous` reply and no
validation error in the 120 runs. The last column, from the transcripts,
says how many runs got every key entry among the rows of their dagayn
replies, and what the first reply held.

| Q | Exact (dagayn / none) | Tokens (dagayn / none) | Token ratio, Holm p | Time ratio, Holm p | Turns (dagayn / none) | dagayn calls | Checks | Key among dagayn's rows |
|---|---|---|---|---|---|---|---|---|
| T13 LONG reach | 10 / 9 | 288k / 303k | 0.94x, 1.0 | 1.01x, 1.0 | 9 / 10 | 2 | 6 | 9 runs; `tests_for` 0 rows, its nearest test 1 of 2 |
| T14 LONG name | 10 / 10 | 158k / 164k | 0.97x, 1.0 | 0.82x, 0.13 | 5.5 / 7 | 1 | 3.5 | 0 runs; 12 of 14 |
| C13 LONG reach | 10 / 10 | 208k / 290k | **0.69x, 0.015** | 0.76x, 0.061 | 8.5 / 13 | 3 | 4.5 | 10 runs; `tests_for` 0 rows |
| C14 LONG name | 10 / 10 | 119k / 107k | 1.01x, 1.0 | 0.87x, 0.98 | 5.5 / 6 | 1 | 3.5 | 10 runs; 1 of 1 |
| K5 LONG reach | 10 / 10 | 759k / 591k | 1.32x, 0.23 | 1.12x, 0.98 | 20.5 / 23 | 4 | 15.5 | 0 runs; no key test in any reply |
| H1 LONG reach | 10 / 10 | 137k / 109k | 1.15x, 0.81 | 1.19x, 0.19 | 7 / 5 | 2 | 3.5 | 10 runs; `tests_for` 2 of 6 |
| H2 LONG reach | 10 / 10 | 212k / 137k | **1.54x, 0.0012** | **1.38x, 0.0012** | 9 / 6 | 3.5 | 5 | 8 runs; `tests_for` 0 rows |
| H3 LONG name | 5 / 7 | 624k / 632k | 0.84x, 0.81 | 0.87x, 0.98 | 12 / 12.5 | 1 | 10 | 0 runs; 76 of 82, and 13 rows from `_examples/` |
| K6 SHORT | 10 / 10 | 155k / 188k | 0.75x, 0.23 | 0.84x, 0.91 | 6 / 9 | 1 | 4 | 10 runs; 10 of 10, and 2 rows from `_examples/` |
| H4 SHORT | 10 / 10 | 72k / 98k | **0.74x, 0.029** | 0.78x, 0.21 | 3 / 5 | 1 | 1 | 10 runs; 4 of 4, and the 2 edited callers |
| H5 SHORT | 9 / 10 | 136k / 123k | 1.10x, 1.0 | 0.91x, 1.0 | 6 / 6.5 | 1 | 4 | 0 runs; 5 of 7 (the other 2 named in the summary), and 1 row from `_examples/` |
| H6 SHORT | 10 / 10 | 97k / 73k | 1.16x, 0.36 | 1.06x, 1.0 | 4 / 5 | 1 | 2 | 10 runs; 1 of 1 |

For a reach question, "key among dagayn's rows" counts a key test that
appeared as a row of any dagayn reply in the run (`tests_for`, or
`callers_of` with or without `depth`). On H4 the reply lists all six
callers of `cW`, and the agent removes the two that the commit edited.

The registration said that a LONG question may turn out short for the
baseline. Here two of chi's LONG questions did. H1's baseline took a median
of 5 turns and H2's took 6. C14 also took 6 and T14 took 7. In G9, every
LONG baseline median was 6 turns or more. The SHORT baselines took 5 to 9
turns.

### SHORT fixed cost (registered, report only)

`fixed_cost.py` counts the tokens of every request up to and including the
first one that makes a dagayn call. In all 40 SHORT candidate runs, that
was request 1. No run called `ToolSearch`.

| | H4 | H5 | H6 | K6 | Pooled |
|---|---|---|---|---|---|
| Median fixed cost | 21.5k | 21.4k | 21.5k | 21.4k | 21.5k |
| Runs with `ToolSearch`, then the call | 0 | 0 | 0 | 0 | 0 / 40 |

With each candidate run's fixed cost removed, the SHORT token ratio is
0.73x. G9's fixed cost, under MCP, was a median of 45k over two requests
(22k for `ToolSearch` and 23k for the call). G10's fixed cost is one request
the size of the prompt. The baseline's first request was about the same
size: a median of 21.1k tokens, against 21.5k for the candidate. The
candidate's prompt is longer by the CLI condition line. So most of what
`fixed_cost.py` counts here is the cost of any first request, not an
overhead of dagayn.

## The wrong answers

<!-- derived-from ./PREREG.md#scoring -->

There were 10 inexact answers: 6 with dagayn and 4 without. No key was
found wrong, so nothing is scored a second way.

- **H5, 1 run with dagayn (116): `_examples/limits/main.go::main`
  added.** This is the SHORT miss that fails the bar. All 10 dagayn runs
  got the same first reply. It had 6 rows: 5 key callers and
  `_examples/limits/main.go::main`. The summary named the other 2 key
  callers (`TestMountingSelf`, `TestWalkInlineMiddlewaresAcrossSubrouter`)
  as calls through the `Router` interface. The prompt excludes
  directories that start with `_`, as the Go tools do. Run 116's own grep
  excluded `./_` paths, but its final reply says that dagayn returned 6
  callers and that it added the 2 calls through the interface by hand. It
  kept the `_examples` row. The other 9 runs dropped it. This is measured:
  the wrong entry came from dagayn's rows.
- **H3, 4 runs with dagayn (017, 070, 072, 109) and 3 without (052, 064,
  083): `middleware/sunset_test.go::main` added.** That `func main()` is
  inside a `/** ... */` comment that starts at line 112 ("EXAMPLE
  USAGES"). The key stands. It is a grep artifact in both arms. dagayn's
  reply did not list it.
- **H3, 1 run with dagayn (043): missed `mux_test.go::TestRouterFromMuxWith`.**
  That test calls `with.Get(...)` on `with := r.With(...)`, a `Router`.
  dagayn's 89 rows leave out 6 of the 82 key callers, this test among
  them. The summary says that 74 calls go through `Router` and
  names 3 of 24 functions to read. Run 043 wrote its own scanner, and that
  scanner also missed this call. Whether dagayn's missing row contributed
  cannot be told from the transcript.
- **T13, 1 run without dagayn (039):** gave `TestSpew` and missed
  `TestPrintSortedKeys`.

H3 has the most inexact answers in both arms (5 and 3 of 10). Its key has
82 entries, as the registration's threats section expected. H3 accounts
for the LONG exact gap (5 against 7); T13 goes the other way (10 against
9).

## Where the tokens went

This section is exploratory. It comes after the registered test and
decides nothing. It reads the raw transcripts (not committed). A
"request" is one model call: one distinct assistant message ID in the
transcript. Its tokens are about the size of the context at that point. A
request count includes the final answer. "Measured" means counted from the
transcripts. "Inferred" means a reading of why, which no transcript
states: the agents' thinking is not in the transcripts.

### The CLI calls themselves

Measured, over the 120 candidate runs:

- 227 Bash commands called `dagayn tool`. They returned 355 JSON replies:
  354 had status `ok` and `results_complete: true`, and 1 was `not_found`
  (039-K5, a name the agent guessed, `Context.getHelpOptions`). 11 more
  calls piped the output through `python3`, `jq`, or `head`, so the
  transcript has no JSON reply for them.
- No call failed on its arguments. Every `(*T).M` target resolved
  (`resolution: exact_name`). One command ended with exit code 1 (110-H2),
  because of a zsh error from its own `echo ====`, not from dagayn.
- The largest reply was H3's 89-row `callers_of` (18.5k characters). Every
  other reply was under 6k.
- `next` was rarely followed. Of 345 replies that had a `next`, the agent
  later made the first `next` call in 25 (C13 12, H2 5, T13 4, K5 2, H6 2,
  all others 0). Most first `next` calls were `source_of`. The agents read
  the same lines with `sed` or `cat` instead.

### Repeating the search after a complete answer

The condition line tells the agent not to recount a complete `ok` reply
with grep. Measured: the number of dagayn runs that searched for the
target's name (a `grep`, `rg`, or Grep pattern containing it) after the
first dagayn call:

| T13 | T14 | C13 | C14 | K5 | K6 | H1 | H2 | H3 | H4 | H5 | H6 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 9 | 10 | 10 | 10 | 6 | 10 | 5 (9 for `Throttle`) | 6 (10 for `Recoverer`) | 10 | **2** | 10 | 9 |

H4 is the exception. Its reply said that every call of `cW` is bound to a
definition and nothing is missing, and only 2 runs searched again. H4 is
also the largest SHORT gain (0.74x). Every other SHORT reply said something
that could read as uncertain, even when its rows were complete. Inferred:
the agents searched again whenever the summary hedged.

- K6: "41 calls named Run go to no definition ... (testing.T and
  testing.B) ... so callers may be missing". The rows held all 10 callers.
- H6: "no caller is missing from the list; the rows are matched by name
  (match "bare_name"), not bound by type". The one row was the key.
- H5: two of the key callers were in the summary, not in the rows.

### SHORT

Pooled over 40 runs per arm, the medians were 103k tokens against 102k, 4
requests against 4, and a first request of 21.5k against 21.1k tokens.
The registered ratio (0.92x) is the mean of the per-question log ratios,
not a ratio of these pooled medians: H4 and K6 (0.74x, 0.75x) weigh against
H5 and H6 (1.10x, 1.16x). In
G9, under MCP, the medians were 6 requests against 5. Measured: the CLI
removed the `ToolSearch` request. Inferred: that is most of the change in
the SHORT ratio from G9's 1.17x to 0.92x. The questions also differ, so
this is not a test of that (see [below](#cli-against-mcp)).

- **H4 (0.74x) and K6 (0.75x).** The first reply was complete. H4 needed
  only to remove the two edited callers (3 requests against 4). K6's
  baseline took 9 turns to grep `Run(` through the tests. With dagayn, the
  runs grepped too, but they did it in fewer steps.
- **H6 (1.16x).** Measured: the baseline took 3 requests (one grep for
  `NewLogEntry`, a few lines of output, then a read). The dagayn runs took 4:
  the dagayn call, then the same grep in 9 of 10 runs. The reply could not
  save anything on a name that a single grep answers, so the call was one
  request more.
- **H5 (1.10x).** The reply put 2 of 7 callers in the summary as line
  numbers, and every run grepped `Group` (10 of 10).

### LONG losses

**H2 (1.54x tokens, 1.38x time).** The question asks which tests run
`(prettyStack).parse`. The chain is `parse` ← `printPrettyStack` ←
`PrintPrettyStack` or `defaultLogEntry.Panic` ← `Recoverer`, and the tests
pass `Recoverer` as a value (`r.Use(Recoverer)`).
Measured:

- The candidate runs made a median of 3 requests with dagayn calls and 3
  without. The baseline made 4 requests without, and then the answer (a
  median of 7 requests against 5). The dagayn calls replaced about one read
  and added three requests.
- `tests_for(prettyStack.parse)` (5 runs) returned 0 rows,
  `results_complete: true`, and `test_reach.nearest_test:
  TestRecoverer, hops: 4`. Its summary said: "No test calls it directly;
  middleware/recoverer_test.go::TestRecoverer reaches it through 4 caller
  hop(s). callers_of with depth 6 lists every caller, tests included".
- In 6 runs (the 5 above and 004), the agent called `callers_of` on
  `parse` with a depth. It got 4 functions (`printPrettyStack`, `defaultLogEntry.Panic`,
  `PrintPrettyStack`, `Recoverer`) and no test. The summary said "No caller
  the graph records is missing; calls dispatched at run time are not
  edges". One run (004) asked for `depth=10`, and the reply said "within 6
  hops".
- The tests appeared only in `callers_of(Recoverer)` (8 runs). It had 8
  rows: 5 `main` functions under `_examples/` and 3 tests, all with
  relation `references`. Its summary said: "8 use it as a value ...
  callers may be missing: read where the value goes". One of the 3 tests,
  `TestRecovererAbortHandler`, does not reach `parse`, because it panics
  with `http.ErrAbortHandler`, which `Recoverer` re-panics.
- `tests_for(Recoverer)` returned 1 row, `TestRecoverer`, with
  `results_complete: true`. Its evidence was `coverage_source: heuristic`,
  "test node name references target symbol". It did not list
  `TestRecovererNoColor` (in the key) or `TestRecovererAbortHandler`, which
  use `Recoverer` in the same way.
- Every run then read `recoverer_test.go`, as the baseline did, and
  searched for `Recoverer` (10 of 10). All 10 were exact.

Inferred: the reply sent the agents to a depth call that could not show
the tests, so they read the files the baseline read, after three requests
of graph calls. On a question that the baseline answers in 6 turns, those
three requests are the whole loss.

**K5 (1.32x tokens).** The question asks which tests run `formatEnvs`.
`formatEnvs` runs only when help is printed for a flag with an environment
variable. Measured:

- The median other requests (not dagayn) were 13.5 in both arms. The
  candidate added a median of 4 dagayn calls over about 4 requests, and
  replaced none (19.5 requests against 14.5). At a context of 21k to 40k
  tokens per request, that is about the 168k median gap.
- No dagayn reply in any run held a key test. `callers_of(formatEnvs)`
  gave `DefaultHelpValueFormatter`. `callers_of(DefaultHelpValueFormatter)`
  gave `kong.go::New`, "1 use it as a value ... read where the value goes".
  This is G9's defect 2 (#289), unchanged.
- `tests_for(formatEnvs)` returned 0 rows and said: "kong_test.go::TestBoolPtr
  reaches it through 2 caller hop(s)". `TestBoolPtr` does not run
  `formatEnvs` (it is not in the coverage key). No run used it.
- 2 of 63 first `next` calls were followed.

Inferred: the graph could not answer this question (the reach goes through
a function stored as a value and through flag data), and the agents had to
do the baseline's search anyway, after paying for the graph calls.

**H1 (1.15x tokens).** Measured:

- `tests_for(ThrottleWithOpts)` (5 runs) returned 2 rows,
  `results_complete: true`, and the summary "Found 2 result(s)". The other
  4 key tests call `ThrottleBacklog`, which calls `ThrottleWithOpts`. The
  reply did not mention them. When `tests_for` finds no direct test, its
  summary names the nearest test and suggests a depth call. When it finds
  one or more, it says nothing about tests through callers.
- `callers_of(ThrottleWithOpts)` (direct) listed `ThrottleBacklog`. All 10
  runs made a second dagayn request on `ThrottleBacklog` or a depth call,
  which gave all 6 tests (the depth call also gave `BenchmarkThrottle` and
  `_examples/limits/main.go::main`).
- 9 of 10 runs then grepped `Throttle` and read `throttle_test.go`, as the
  baseline did. The median was 5 requests, against the baseline's 4.

An agent that obeyed the condition line, and took the complete `tests_for`
reply as the answer, would have answered 2 of 6 in every run. That did not
happen here only because the agents searched anyway.

### LONG gains

- **C13 (0.69x tokens, Holm p = 0.015).** `tests_for` returned 0 rows and
  named a nearest test. The agents followed the depth call more often than
  on any other question (12 of 48 first `next` calls). Every run had both
  key tests among its rows. The baseline took 13 turns to search the s2
  tests.
- **T14 (0.82x time) and H3 (0.84x tokens).** These are not significant.
  On both, the reply held most of the callers, and the agents still
  searched the whole name (10 of 10).

## LONG time against G9's (across experiments, report only)

<!-- derived-from ../g9_go/RESULTS.md#where-the-tokens-went -->

This compares two experiments with different questions, a different
transport (G9 MCP, G10 CLI), different dagayn builds (go extractor 17 and
18), and a different repository mix. It tests nothing. It cannot separate
those causes.

| Repository | G9 time (tokens) | G10 time (tokens) |
|---|---|---|
| testify | 0.58x (0.80x), 4 questions | 0.91x (0.96x), 2 questions |
| compress | 0.79x (1.01x), 4 questions | 0.81x (0.84x), 2 questions |
| kong | 1.05x (1.31x), 4 questions | 0.97x (1.00x), 2 questions |
| chi | none | 1.01x (1.06x), 6 questions |
| **LONG, pooled** | **0.79x** (0.95x), 8 questions | **0.98x** (1.03x), 8 questions |

These are the registered per-repository families, over all of each
repository's questions, LONG and SHORT. On LONG questions alone, from the
per-question effects, testify and compress give 0.86x time here (T13, T14,
C13, C14). In G9 they gave 0.70x (T9 to T11, C9 to C11). kong gives 1.12x
here (K5), and gave 1.10x in G9 (K1, K2). chi gives 1.12x (H1 to H3).

- G9's LONG time gain came from testify (0.58x) and compress (0.79x). The
  same two repositories gave 0.91x and 0.81x here, on new questions. So
  the tuned repositories kept part of the gain, and compress is the one
  repository with a significant gain here (secondary family: time 0.81x,
  Holm p = 0.017; tokens 0.84x, Holm p = 0.044).
- chi carries 3 of the 8 LONG questions, and lost time on 2 (H1 1.19x, H2
  1.38x). Two of those were short for the baseline (5 and 6 turns): the
  tests sit next to the code in one file, and one grep plus one read finds
  them.
- What cannot be told: whether the CLI, the new questions, or chi
  explains the change.

## Defects in dagayn's Go answers

<!-- derived-from ../g9_go/RESULTS.md#defects-in-dagayns-go-answers -->

These are the defects that the runs show. They are recorded for the plan
and are not fixed here. The PR that adds this file has an issue draft for
each. No issue has been filed.

1. **Rows from directories the Go tools ignore** (H5, H3, H2, K6, H1). Go
   ignores directories whose names start with `_` or `.`, and `testdata`.
   chi's `_examples/` is also its own module. dagayn lists functions there
   as callers, with no marker: 13 of H3's 89 rows, 1 of H5's 6, 5 of the 8
   rows of `callers_of(Recoverer)`, and 2 of K6's 12. The summaries' "read"
   lists and `next` calls point into them too (H3's first "read" is
   `_examples/custom-handler/main.go::customHandler`). It caused the one
   SHORT miss, which fails the bar.
2. **The test-reach hint does not match `callers_of` with a depth** (H2,
   K5, C13). When `tests_for` finds no direct test, its summary names
   `nearest_test` and says "callers_of with depth 6 lists every caller,
   tests included". On H2 that call returned no test and said that no
   caller is missing, because the chain to the test passes a value use
   (`r.Use(Recoverer)`). On K5, the nearest test was `TestBoolPtr`, which
   does not run `formatEnvs`. The only path is `kong.New`, which stores
   `DefaultHelpValueFormatter` as a value. On C13 the nearest test was
   `ExampleIndexStream`, an Example, which the question excludes. A
   `depth=10` request was answered "within 6 hops" with no note.
3. **`tests_for` with direct hits says nothing about tests through
   callers** (H1). It returned 2 of 6 tests with `results_complete: true`
   and no hint. The other 4 call the target through `ThrottleBacklog`, one
   hop away. An agent that followed the condition line would have been
   wrong in every run.
4. **`tests_for` on a function used as a value picks tests by name** (H2).
   `tests_for(Recoverer)` returned only `TestRecoverer` (`coverage_source:
   heuristic`, "test node name references target symbol") with
   `results_complete: true`. Two other tests use `Recoverer` in the same
   way, and one of them is in the key.
5. **"Callers may be missing" for calls on concrete standard-library
   types** (K6, T14). K6's 41 unbound `Run` calls were all on `testing.T`
   and `testing.B`, which are structs and cannot be a `*kong.Context`. The
   summary still said "callers may be missing" and named 22 functions to
   read. The rows were complete. All 10 runs searched again. T14's summary
   counted 245 such calls on `testing.T` and named 164 functions to read.
6. **A method call on a value declared by a conversion is unbound** (T14).
   `Test_Arguments_Error` and `Test_Arguments_Error_Nil` call
   `args.Error(3)` on `var args = Arguments([]interface{}{...})`. Neither
   is a row. Inferred: they are among the "11 on values whose type the
   graph does not know". The "read" list did not name them first. All 10
   runs were exact because they searched.
7. **A function used as a value still sends agents to search** (K5). This
   is G9's defect 2 (#289), still open.

## CLI against MCP

<!-- derived-from ./PREREG.md#why-the-cli -->

G10 has no MCP arm. The pilot that chose the transport compared the CLI arm
with the MCP arm, on six G9 questions that had been tuned on, with 5 runs
per arm, and it was not registered. It gave CLI/MCP token ratios of 0.74x
on SHORT and 0.54x on LONG.

- What can be said: under the CLI, no run spent a request on `ToolSearch`,
  and the first dagayn call was request 1 in every SHORT run (G9 under
  MCP: request 2 in 36 of 40). The candidate's first request was about the
  size of the baseline's (21.5k against 21.1k tokens).
- What can be said: no CLI call failed on its arguments, and no first call
  was `ambiguous`.
- What cannot be said: that the CLI is cheaper or faster than MCP on these
  questions, or by how much. G10 compares the CLI with no dagayn, on new
  questions, and its arms do not include MCP. G9 compared MCP with no
  dagayn on other questions. The pilot's 0.54x and 0.74x were measured on
  tuned questions and do not carry over to G10's.
- What cannot be said: that the SHORT ratio's change from 1.17x (G9) to
  0.92x (G10) comes from the transport. The questions and the guard also
  changed.

## What can and cannot be said

- What can be said: on eight new LONG Go questions, half of them on chi,
  an agent with dagayn at go extractor 18 through the CLI used about the
  same tokens (1.03x) and time (0.98x) as one without it, with about the
  same accuracy (75 against 76 exact).
- What can be said: on the four SHORT questions, it used fewer tokens
  (0.92x, within the 1.25 guard). The one wrong answer took a row that
  dagayn listed from `_examples/`, a directory outside the question's
  scope. That one answer fails the SHORT guard's accuracy rule.
- What can be said: on chi, the repository that no fix was tuned on, the
  LONG questions cost more with dagayn (tokens 1.14x, time 1.12x, Holm
  p = 0.077, secondary). H2 (1.54x) was significant after Holm. On compress,
  which was tuned on, both tokens and time were lower (secondary).
- What can be said: after a reply that was complete for what it covered,
  at least half of the runs searched for the name again on every question
  but one, even with the condition line telling them not to. They did not search again on the
  one question whose summary said plainly that nothing was missing (H4).
- What cannot be said: why the agents searched again. The transcripts show
  that they did, and which summaries came before the search, but not why.
- What cannot be said: anything about the MCP transport, or about Go
  questions in general. The questions are a draw from rule-defined pools,
  6 of the 12 are on one small repository, and time is compared only
  within one invocation, because the runs and the candidate's graph builds
  share the machine.
