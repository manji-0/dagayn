# Results: G9, the Go agent experiment under the revised bar

<!-- derived-from ./PREREG.md -->

The questions, keys, scoring, arms, and analysis were registered in commit
323e27b600da8787a73605658b29272f14ea7ebb (`eval/g9-go-prereg`, merged as
#284 in 57ffdf17) before any run. `PREREG.sha256` matches all 25 registered
files, and nothing in the registration changed after it. The deviations in
how the runs were invoked are listed under [What ran](#what-ran).

## Verdict

**The bar is not met.** The LONG part holds and the SHORT part fails.

- **LONG (8 questions): met, through time.** Agents with dagayn took 0.79x
  the time of agents without it (Holm p = 0.0003). Tokens were 0.95x (Holm
  p = 0.44). They gave 77 exact answers out of 80, against 75.
- **SHORT (4 questions): not met.** The token ratio was 1.17x, above the
  1.10 guard. Exact answers were 40 against 37, which passes. The
  permutation p for SHORT tokens (0.007) is report only.

The bar needs both parts. Go is not first-class by the plan's definition.

## What ran

- 2026-11-04 (JST). The plumbing check ran at 22:42. testify and compress
  ran at the same time from 22:42 to 23:04, then kong from 23:04 to 23:16.
  There were 240 runs: 12 questions, 2 arms, 10 runs each, Claude Haiku
  5.5, `--effort medium`, `-j 3`, timeout 1,200 s, with the registered
  seeds.
- Plumbing check: 1 run per arm of `plumbing_kong.json` (the 12 methods of
  `Kong`). Both answers were exact, and the candidate's MCP server reported
  `dagayn:connected`. These runs are not part of the results.
- No run failed, timed out, or lost its MCP server (120 of 120 candidate
  runs `dagayn:connected`). The sensitivity analysis (`--include-failed`)
  is therefore identical to the primary one.
- The candidate was the `dagayn` uv tool, as registered. Its `_core` had
  SHA-256 `cddb3249...b2897d75` and reported go 17 from outside the
  checkout, both before and after the runs. Every candidate graph was a
  full build at go extractor 17, with no SCIP overlay (120 of 120). No
  candidate run's graph metadata changed during the run.
- `scip-go` and `gopls` were not on the runs' PATH. The harness file's
  SHA-256 (`7c3aae7d...a997e98`) is G8's.
- The CLI's estimate of cost was 2.32 USD: 1.07 with dagayn and 1.25
  without. The plumbing check cost 0.02 USD. All of it was under the 25 USD
  cap.
- No run executed the tests or the repository's code. No Bash command in
  the 240 runs ran `go test`, `go run`, `go build`, `go vet`, `go tool`,
  `go list`, `go doc`, `gopls`, or `scip-go`.
- Raw transcripts are in the running worktree's `out-g9-*/raw/` (not
  committed). `results/` holds each batch's `results.csv` and
  `summary.json`, `analysis.json`, and `analysis_include_failed.json`. The
  plumbing check's files are not included, as in G8.

Deviations from the registered procedure. None of them changes the
questions, keys, arms, or analysis.

- **Interpreter.** The harness and `analyze.py` ran under the running
  worktree's `.venv/bin/python` (Python 3.14.7), not `uv run python`.
  `uv run` tried to rebuild the checkout's editable Rust package and failed
  with the rustc on PATH (1.98). The registration allows either and asks
  the results to say which ran. The candidate is the uv tool in both cases.
- **Invocation.** The registered commands ran from two `sh` scripts,
  because zsh does not split the unquoted `$COMMON` into arguments. The
  commands, arguments, and order are the registered ones: the plumbing
  check, then testify and compress in parallel, then kong.

## Pooled (registered test)

LONG, the primary test (Holm over the three measures):

| Measure | dagayn / none | p | Holm p |
|---|---|---|---|
| Total tokens | 0.95x | 0.22 | 0.44 |
| Time | **0.79x** | 0.0001 | **0.0003** |
| F1 | +0.011 | 0.60 | 0.60 |

SHORT, the guard (a point estimate):

| Measure | dagayn / none | p (report only) |
|---|---|---|
| Total tokens | **1.17x** | 0.007 |

| Bar check | Result |
|---|---|
| LONG: tokens at most 0.85x, Holm p < 0.05 | no (0.95x, 0.44) |
| LONG: time at most 0.85x, Holm p < 0.05 | **yes** (0.79x, 0.0003) |
| LONG: exact not 3 or more fewer | yes (77 / 80 against 75 / 80) |
| SHORT: token ratio at most 1.10 | **no** (1.17x) |
| SHORT: exact not fewer | yes (40 / 40 against 37 / 40) |

Per repository (all twelve questions of each):

| Repository | Tokens, Holm p | Time, Holm p |
|---|---|---|
| testify | 0.80x, 0.0006 | 0.58x, 0.0003 |
| compress | 1.01x, 1.0 | 0.79x, 0.0015 |
| kong | **1.31x, 0.0003** | 1.05x, 0.66 |

## Per question

Each value is the median of 10 runs. The Holm correction is over the
twelve questions, per measure. "Turns" is the CLI's `num_turns`. "Checks"
counts the Bash, Read, Grep, and Glob calls after the first dagayn call.
"First call failed" counts the runs whose first dagayn call returned
`ambiguous` or a validation error.

| Q | Exact (dagayn / none) | Tokens (dagayn / none) | Token ratio, Holm p | Time ratio, Holm p | Turns (dagayn / none) | dagayn calls | Checks | First call failed | dagayn's registered first answer |
|---|---|---|---|---|---|---|---|---|---|
| T9 LONG reach | 10 / 10 | 257k / 255k | 0.96x, 1.0 | 0.80x, 0.58 | 9 / 11 | 3 | 4 | 0 | 10 / 10 among 58 rows |
| T10 LONG name | 10 / 10 | 226k / 292k | 0.72x, 0.060 | **0.65x, 0.0096** | 8.5 / 8.5 | 1 | 5 | 4 | 17 / 18 |
| T11 LONG name | 10 / 9 | 161k / 222k | 0.75x, 0.17 | **0.45x, 0.011** | 7 / 6 | 2 | 3.5 | 3 | 18 / 18 |
| C9 LONG reach | 10 / 7 | 376k / 585k | **0.68x, 0.026** | 0.82x, 0.73 | 17.5 / 16 | 4 | 7 | 1 | 9 / 9 and 2 extra |
| C10 LONG name | 10 / 10 | 237k / 166k | **1.35x, 0.014** | 0.86x, 1.0 | 11 / 7.5 | 1 | 7 | 0 | 0 / 1, a use of the value |
| C11 LONG name | 7 / 9 | 224k / 226k | 1.07x, 1.0 | 0.74x, 0.58 | 9 / 8 | 1 | 6 | 0 | 3 / 3 |
| K1 LONG reach | 10 / 10 | 183k / 204k | 0.92x, 1.0 | 0.91x, 1.0 | 9.5 / 11 | 2.5 | 4.5 | 0 | 3 / 3 and 1 extra |
| K2 LONG reach | 10 / 10 | 308k / 195k | 1.42x, 0.060 | 1.33x, 0.24 | 11 / 11 | 2 | 7 | 1 | 1 / 2 among 118 rows, cut short |
| T12 SHORT | 10 / 8 | 167k / 182k | 0.80x, 0.69 | **0.50x, 0.018** | 8 / 6 | 1 | 3 | 0 | 6 / 6 |
| C12 SHORT | 10 / 9 | 165k / 157k | 1.04x, 1.0 | 0.75x, 0.22 | 7.5 / 7.5 | 1 | 4.5 | 0 | 5 / 5 |
| K3 SHORT | 10 / 10 | 151k / 97k | **1.66x, 0.0012** | 1.05x, 1.0 | 7 / 5 | 1.5 | 3 | 5 | 3 / 3 |
| K4 SHORT | 10 / 10 | 149k / 119k | **1.37x, 0.0012** | 0.96x, 1.0 | 7.5 / 6 | 1 | 4 | 2 | 0 / 1, a use of the value |

The registration said a LONG question may turn out short for the baseline.
Every LONG question's baseline median was 6 turns or more (T11 6, C10 7.5,
C11 8, T10 8.5). The SHORT questions' baselines took 5 to 7.5.

## The wrong answers

There were 11 inexact answers: 3 with dagayn and 8 without.

- **C11, 3 runs with dagayn (015, 018, 075) and 1 without (005):
  `gzip/example_test.go::ExampleReader_Multistream` added.** Its
  `zw.Reset(&buf)` is on the standard library's `*gzip.Writer`:
  `example_test.go` is package `gzip_test` and imports `compress/gzip`. The
  key stands. In all three dagayn runs, `callers_of` returned the 3 key
  rows with `results_complete: true`. Its summary then said that "80
  calls named Reset go to no definition ... so callers may be missing:
  read gzip/example_test.go::ExampleReader_Multistream, ...". The first
  `next` call was `source_of` on that function. Run 075 also added
  `gzhttp/writer/gzstd/stdlib.go::NewWriter`, which also uses the standard
  library's gzip.
- **C9, 3 runs without dagayn:** missed
  `TestConcurrentBlocks_ReadFrom` and
  `TestConcurrentBlocks_InterleavedWriteReadFrom`.
- **T12, 2 runs without dagayn:** missed the 5 HTTP tests of
  `http_assertions_test.go`.
- **T11, 1 run without dagayn (056):** gave 2 of the 18 callers.
- **C12, 1 run without dagayn (030):** missed `Encoder.ReadFrom` and
  `Encoder.readFromJobs`, and added `Encoder.nextBlock`.

No key was found wrong, so nothing is scored a second way.

## Where the tokens went

This section is exploratory. It comes after the registered test and
decides nothing. It reads the raw transcripts (not committed). A
"request" here is one model call: one distinct assistant message ID in
the transcript. Each request re-reads the whole context, so a run's tokens
are close to the number of requests times the context size.

### SHORT: one more request than the baseline

Pooled over the 40 runs per arm, the medians were:

| | dagayn | none |
|---|---|---|
| Tokens | 156k | 120k |
| Requests | 6 | 5 |
| Context of the first request | 21.8k | 21.1k |
| Checks (Bash, Read, Grep, Glob) | 4 | 5 |

- **Before the first file check.** In 36 of 40 runs, the first request
  loaded the dagayn schemas (`ToolSearch`) and the second made the dagayn
  call. These two requests used a median of 45k tokens. No run checked a
  file before its first dagayn call.
- **After the dagayn call.** The runs made a median of 4 more requests
  (108k tokens) and 4 checks. The baseline's whole run was 5 requests and
  5 checks. Each request's context was 22k to 33k tokens. So one extra
  request is about 25k to 35k tokens, close to the whole median gap (36k).
- **The search was repeated.** Every `callers_of` reply with status `ok`
  on a SHORT question said `results_complete: true`. In 36 of the 40 runs,
  the agent then searched for the target's name with Grep or `grep`
  (C12 10, K3 10, K4 8, T12 8). The transcripts do not say why.

Per question:

| Q | Token ratio | Requests (dagayn / none) | What the transcripts show |
|---|---|---|---|
| K3 | 1.66x | 6 / 4 | 5 first calls named the bare `IsBool`, which is `ambiguous` among 5 definitions, so these runs made a second call. The complete reply named two calls through the `BoolMapper` interface (`mapper.go:767`, `model.go:338`) and said to read those lines. All 10 runs then grepped `IsBool`. The baseline answered with one grep and a few reads. |
| K4 | 1.37x | 6 / 5 | dagayn's reply had one row, `TestHelpTree`, named as a use of the value, as registered. 8 runs grepped `LineIndenter` to find where the value goes. 4 runs called dagayn without `ToolSearch`, and 2 of them sent a call with no `target`, which failed validation. |
| C12 | 1.04x | 6 / 5.5 | The first call was complete (5 / 5) in all 10 runs. All 10 then grepped `CRC`. |
| T12 | 0.80x | 6 / 6 | The first call was complete (6 / 6) in all 10 runs. The baseline ran 50 Bash commands across its 10 runs and 2 of its runs missed the HTTP tests. |

### LONG

- **Gains.** C9 (0.68x tokens) replaced the baseline's search of the zstd
  tests (16 turns, 585k tokens). T10 and T11 (0.45x to 0.65x time) had
  complete or nearly complete answers on a name called on hundreds of
  lines. On T10 and T11, 7 first calls named `(*Assertions).True` or
  `(*Assertions).False` without a path. These are `ambiguous` because two
  nodes match: `assert` and `require` each have an `Assertions` type. This
  is not the G8 spelling defect (#275). Path-qualified `(*T).M` targets
  resolved in every run (`resolution: exact_name`).
- **Losses.** C10 (1.35x) and K2 (1.42x) are the two questions where the
  registered first answer was short. On C10, the one row was `NewWrapper`,
  named as a use of the value, and all 10 runs grepped `NewWriter` after
  it (a median of 7 checks). On K2, `callers_of(depth=6)` returned 118
  rows, and the runs made a median of 7 checks. Both arms were exact on
  every run of both questions.
- Across all 120 dagayn runs, 110 searched for the target's name after the
  first dagayn call. 6 runs called dagayn without `ToolSearch`. In 4 runs,
  the first call had no `target` and failed validation.

## Defects in dagayn's Go answers

These are the defects the runs show. They are recorded for the plan and
are not fixed here. No issue has been filed for them yet.

1. **The unbound-call hint points at calls on a standard-library type**
   (C11). When more than six calls of the name go to no definition, the
   summary names functions to read, and the first `next` call reads the
   first one. On C11 that was `ExampleReader_Multistream`, whose call is
   on `compress/gzip`'s `*Writer`, imported by name in that file. 3 of 10
   dagayn runs added it, against 1 of 10 without dagayn. It is the only
   accuracy loss with dagayn.
2. **A function used as a value still sends agents to search** (C10, K4).
   The reply lists the function that stores the value, as #277 intended
   and as registered. It does not say where the stored value is called.
   The agents then searched for it: C10 1.35x, K4 1.37x, both exact in
   every run.
3. **Test reach at depth 6 is a long list on kong** (K2). 118 rows, 109 of
   them tests, for 2 tests that run the function. K2 cost 1.42x tokens
   and 1.33x time.
4. **A complete answer does not end the search.** On SHORT, every `ok`
   reply said `results_complete: true`, and 36 of 40 runs still grepped
   the name. With the schema load and the call, that is one request more
   than the baseline. This item is an inference from the transcripts. No
   run says why it checked.

## What can and cannot be said

- What can be said: on eight LONG Go questions drawn by a fixed rule, an
  agent with dagayn at go extractor 17 took less time (0.79x, Holm
  p = 0.0003) at about the same tokens (0.95x) and the same accuracy (77
  against 75 exact).
- What can be said: on the four SHORT questions, it used more tokens than
  the 1.10 guard allows (1.17x). The cost came from the two kong questions
  (K3 1.66x, K4 1.37x).
- What can be said: kong is the one repository the fixes since G8 were not
  tuned on, and it is the one where dagayn cost tokens (1.31x, Holm
  p = 0.0003) and saved no time (1.05x). testify, where #278 to #283 were
  tuned, gained in both tokens (0.80x) and time (0.58x).
- What cannot be said: that the LONG time gain holds for Go questions in
  general. The questions are a draw from rule-defined pools, the LONG and
  SHORT rule rests on one gap in twelve earlier questions, and the bar was
  redefined after G8 missed it. Time is compared only within one
  invocation, because the runs and the candidate's graph builds share the
  machine.
- What cannot be said: why the agents repeated the search after a
  complete answer. The transcripts show that they did, not why.
