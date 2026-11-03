# Pre-registration: G7, Go questions a short script cannot answer

<!-- derived-from ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#go-work -->
<!-- derived-from ../m3_python/PREREG.md -->

This file was written and committed before any agent run on these
questions. `PREREG.sha256` holds the SHA-256 of every registered file. The
results name the commit that registered them.

## Purpose

Item 10 of the first-class languages plan: the agent experiment for Go,
after G1 to G5, in the design of E13 (Rust) and M3 (Python), with keys from
scip-go and Go coverage instead of from dagayn. M3 did not meet its bar:
where dagayn's answer was empty or partial (a function passed as a value, a
table, a callback), agents paid for the dagayn calls and then read the
source as the baseline did. G7 asks the same four kinds of question, which
grep or a short script cannot answer from syntax alone, on two third-party
Go modules:

| Kind | testify v1.12.1 (959dbda) | compress v1.20.1 (5d880f2) |
|---|---|---|
| Which tests execute a function | T1 `(*CollectT).Errorf` | C1 `statelessEnc` |
| Callers of a method on a receiver whose type the call site does not name | T2 `(*Assertions).Equal` | C2 xxhash `(*Digest).Reset` |
| Callers through an interface or a method value | T3 `(*Suite).T` | C3 `(*compressor).storeFast` |
| What a change left unedited | T4 `ObjectsAreEqual` | C4 flate `(*Writer).Reset` |

Why grep does not answer them: T1 runs only when an assertion fails inside
an `EventuallyWithT` condition, or when a `*CollectT` is the outer `t`; C1
runs only for gzip level -3 and the stateless API, reached through a level
value at run time. T2's call sites read `assert.Equal(...)` with `assert` a
local `*Assertions`, among 315 `.Equal(` calls, most of them the package
function; C2 is one of 240 `.Reset(` calls in compress, on fields whose type
is declared elsewhere.
T3 adds a call through the `TestingSuite` interface; C3 is called only as
`d.step(d)`, a field `init` sets to the method expression. T4 and C4 need
the diff as well as the callers; C4 is again one of 240 `.Reset(` calls.

The prompts are in `tasks_testify.json` and `tasks_compress.json`. Each
target is `git archive` of the tag's commit plus one change commit
(`testify-change.patch`, `compress-change.patch`), so `HEAD~1..HEAD` is the
change T4 and C4 ask about. Both prompts forbid running the tests or the
repository's code (`go test`, `go run`).

## Arms

| Arm | Setup |
|---|---|
| candidate | The installed `dagayn` uv tool (`/Users/manji0/.local/bin/dagayn`, 10.0.0). Its receipt installs from the directory `/Users/manji0/src/dagayn`, whose `main` was 709c89a8. Its Python files equal the registering branch's `dagayn/` at 709c89a8 (`diff -r` differs only in vendored grammar directories), and `_core.cpython-314-darwin.so` was built at 2026-11-03 17:44 JST, 3 minutes after that commit; it reports go extractor 12. Each run clones the target and runs `dagayn build --local-embedding none` (the harness default). The MCP server is `dagayn serve`. |
| baseline | No MCP server. |

No `--scip`, unlike M3: the plan decided (2026-10-26) that Go's bar is met on
the extractor alone and that scip-go is a measurement tool, not a build
requirement. `scip-go` is not on the runs' PATH. Both arms have Go 1.24.13
(`mise`) on PATH and neither has `gopls`, `guru`, or `callgraph`; `go list`,
`go doc`, and `go vet` are allowed by the prompt in both.

## Procedure

- Harness: `eval/run_agent_tokens_eval.py` at the registering commit. The
  one change since M3 reads Go spellings of a symbol as the same symbol
  (`(*T).M`, `*T.M`, `(T).M`, `T[K].M`, and a subtest suffix `TestX/case`,
  in `.go` paths only; see Scoring).
- Model `claude-haiku-5-5`, `--effort medium`, 10 runs per question per arm:
  8 questions x 2 arms x 10 = 160 runs, as M3.
- Two invocations run at the same time, 3 runs in parallel each, timeout
  1,200 s per run:
  - testify: `--tasks-file eval/g7_go/tasks_testify.json --source-repo <clone
    of github.com/stretchr/testify at v1.12.1> --seed 20261103`
  - compress: `--tasks-file eval/g7_go/tasks_compress.json --source-repo
    <clone of github.com/klauspost/compress at v1.20.1> --seed 20261104`
  - both: `--candidate /Users/manji0/.local/bin/dagayn --baseline none
    --keep-home --reps 10 -j 3 --timeout 1200`
- `--keep-home` keeps the CLI's login and passes `--setting-sources
  project,local --disable-slash-commands`. Neither target has a
  `CLAUDE.md` or `AGENTS.md`.
- Before registration, one plumbing check per target ran on a question
  outside G7 (the methods of `CollectT`; the exported methods of xxhash's
  `Digest`), 1 run per arm. It checked the login, the MCP connection
  (`dagayn:connected`), the change commit, scoring, and `analyze.py`. Both
  arms answered both exactly. It is not part of the results.

## Answer keys

Each key was built without dagayn, from scip-go v0.2.7 and Go coverage on
the harness's target (archive plus change commit), and every entry was
read against the source. `keys/check_keys.py` recomputes the
machine-derived part from files kept outside the repository and agrees
with all eight keys.

Tooling. Go 1.25.3 came from `GOTOOLCHAIN=go1.25.3`, the module cache's
`golang.org/toolchain@v0.0.1-go1.25.3.darwin-arm64` (scip-go v0.2.7 needs
1.25; the local Go is 1.24.13). The scip-go binary is the one G6 built
(`github.com/scip-code/scip-go` v0.2.7, go1.25.3, SHA-256 `b1d476a7...b546f`);
`keys/scipdump` prints its index as TSV, and `keys/gofuncs` lists every
`func` declaration's lines with `go/ast`. testify's whole suite passes under
1.25.3 with the change except two tests the change breaks
(`TestBytesEqual`, `TestNotEqualWithSliceTooLongToPrint`), whose coverage
still counts.

- **T1, C1 (test reach).** `keys/per_test_cover.py` builds each package's
  test binary once (`go test -c -cover -coverpkg=<module>/...`) and runs each
  top-level `Test` and `Fuzz` function alone (`-test.run '^Name$'`, from the
  package directory) with its own cover profile. A test executes a function
  when a block with a count above 0 starts inside the function's lines.
  Two runs each (testify without `-short`; compress with `-short`, plus a
  third run without `-short` on flate, gzip, gzhttp, zlib, and zip) gave the
  same sets. `keys/reach_key.py` keeps the `Test` functions. By hand:
  - T1 (7): `Errorf` appends to the collector. It runs when an assertion
    fails on the `*CollectT` an `EventuallyWithT` condition receives
    (`TestEventuallyWithTFalse`, `TestEventuallyWithTTrue`,
    `TestEventuallyWithT_ConcurrencySafe`,
    `TestEventuallyWithT_ReturnsTheLatestFinishedConditionErrors` in
    `assert`; `TestEventuallyWithTFalse` and `TestEventuallyWithTTrue` in
    `require`), and in `TestEventuallyWithTFailNow`, where a `*CollectT` is
    the outer `t` and `Fail` calls its `Errorf`. Not counted:
    `TestEventuallyWithTSucceedQuickly`, whose condition never fails.
  - C1 (4): `statelessEnc` runs under `StatelessDeflate`, which
    `TestRegressions` and `TestWriterMemUsage` call directly, and which
    `gzip.Writer.Write` calls at level -3: `TestFile1xM3` (`testFile(1, -3,
    t)`) and `TestNewGzipLevelHandler` (levels from
    `gzip.StatelessCompression`). huff0, s2, and zstd tests import flate but
    never use the stateless level, and ossfuzz has no tests, so `-short`
    changes nothing there.
- **T2, T3, T4, C2, C3, C4 (callers).** `keys/scip_refs.py` lists every
  reference occurrence of the target's scip-go symbol by enclosing `func`
  declaration; each site was read to be a call:
  - T2 (9): `assert := New(t)` in `TestEqualWrapper` and the five HTTP
    tests, and `suite.Equal` / `s.Equal` on suites that embed `Suite`, which
    embeds `*assert.Assertions`. Not counted: `require.Assertions.Equal`
    (`r.Equal`, `require.Equal(1, 1)`), the package functions.
  - T3 (14): the 13 functions scip-go sees calling `(*Suite).T`, plus
    `Run`, which calls `suite.T()` on a `TestingSuite`; every suite in the
    repository embeds `Suite` and none declares its own `T`.
  - T4 (10): the 12 functions calling `ObjectsAreEqual` (including
    `assert.ObjectsAreEqual` in `mock`), less `NotEqual` and `isType`, which
    the change edits (`keys/edited_functions.py`; the change also edits
    `ObjectsAreEqual`, which does not call itself).
  - C2 (10): `d.current.crc`, `e.crc`, `d.crc` (fields of type
    `*xxhash.Digest`) and `xxhash.New()` results.
  - C3 (3): `compressor.init` sets `d.step = (*compressor).storeFast` for
    levels 1 to 6, the default level, and custom window sizes; `d.step(d)`
    is called in `write`, `syncFlush`, and `close`. Not counted: `init` and
    `TestWriterReset`, which only assign the field. Nothing calls
    `storeFast` by name.
  - C4 (10): the 12 functions calling `(*flate.Writer).Reset` (including
    `gzip.Writer.init`'s `compressor.Reset(w)` and huff0's
    `BenchmarkDeflate`), less `zlib.Writer.Reset` and `newFlateWriter`,
    which the change edits. Not counted: the `flate.Resetter` interface
    calls on readers and every other type's `Reset`.

The caller sets are the same on the base and the change commit: the
patches add no call to a target and remove none.

## What dagayn answers (checked before registration)

`keys/dagayn_answers.py` makes the first calls an agent would make, at the
default detail level, on graphs built as the harness builds them
(`dagayn_answers.json`):

| Q | dagayn's first answer against the key |
|---|---|
| T1 | `tests_for` 10 rows, all other types' `Errorf` methods (wrong), `test_reach.counts_as_tested` false; `callers_of(depth=6)` 62 rows, no test: 0 / 7 |
| T2 | `callers_of` 9 / 9 and one wrong row: the method itself (its body calls the package function `Equal`, which the graph binds to the method) |
| T3 | `callers_of` 13 / 14: misses `Run` (the interface call) |
| T4 | `callers_of` 12 rows = the 10 + the 2 edited callers; `review_tool` names no unchanged caller |
| C1 | `tests_for` 0 rows while `counts_as_tested` is true (nearest `TestRegressions`); `callers_of(depth=6)` 4 / 4 among 65 tests, 61 of which do not execute it |
| C2 | `callers_of` 10 / 10, nothing extra |
| C3 | `callers_of` 0 / 3 (a call through a field holding a method expression is not an edge) |
| C4 | `callers_of` 11 rows = 9 of the 10 + the 2 edited callers: misses `testResetOutput`, whose writer comes from a function parameter; `review_tool` names no unchanged caller |

Expected before any run: dagayn's answer is complete only on C2, and on T4
after removing the edited callers. It is complete but for one row on T2,
T3, and C4, and it is empty or wrong on T1 and C3. C1's right answer is
buried in 65 rows. Going by M3, the agent will check the near-complete
answers and redo T1, C1, and C3 by reading.

Defects seen while checking, recorded for the plan and not fixed here:
`tests_for` on a Go method returns other types' methods of the same name
(T1); a method's call to the package function of its own name binds to the
method (T2, and `Assertions.Fail` is missing from `Fail`'s callers the same
way); `review_tool`'s `tests_to_run` names helper functions and benchmarks
from `_test.go` files with `go test -run` commands that run nothing
(`benchmarkEncoder`, `testFileLevelDictReset`, `BenchmarkDeflate`).

## Scoring

- The answer is the list in the last ` ```json ` block of the final reply;
  no block scores F1 0 and no exact match. Runs are not excluded for it.
- Each entry is normalized (`symbols` in the harness): backticks, a
  leading `./`, a trailing `[...]`, and a trailing `()` are dropped. In a
  `.go` path, a receiver written `(*T)`, `(T)`, `*T`, or `T[K]` is read as
  `T`, and a subtest suffix (`/...`) is dropped.
- Exact match: the normalized set equals the key. F1 against the key.
- Tokens: input, output, cache read, and cache write, summed over models.
  Time: the CLI's `duration_ms`.

## Hypotheses and test

Two-sided, alpha 0.05, as E8 to E13 and M3 (`analyze.py`):

- H1 total tokens differ, H2 time differs, H3 F1 differs.
- Effect: the mean over questions of the per-question difference in mean
  log tokens (log time; F1), candidate minus baseline. p from 10,000
  permutations of the arm labels within each question (seed 20261103).
  Holm over the three p-values.
- Secondary: the same per repository (Holm over three) and per question
  (Holm over eight, per measure). Descriptive: exact matches, turns, dagayn
  calls, Bash / Read / Grep / Glob calls after the first dagayn call.

## The bar

From the plan's item 10, "tokens or time at most 0.85x at equal accuracy":
the bar is met when, pooled over the eight questions,

- the token ratio is at most 0.85 with Holm p below 0.05, or the time ratio
  is at most 0.85 with Holm p below 0.05 (both from the same three-measure
  Holm family), and
- the dagayn arm has no fewer than the baseline's exact matches minus 2
  (3 or more fewer fails), as in E12, E13, and M3.

Go is first-class when this bar and the M2 gate both hold. The criteria do
not change after the runs.

## Exclusions and exceptions

- A run is failed when it has no result event, ends with an error, times
  out, or (candidate) its MCP server did not connect. Failed runs are
  recorded, not rerun, and left out of the primary analysis. The
  sensitivity analysis (`--include-failed`) counts them with F1 0 and the
  largest tokens and time of their question and arm.
- A candidate run whose graph metadata changed during the run
  (`graph_before` and `graph_after` in its meta file) is reported.
- If a key proves wrong, it stays registered; the results report the error
  and score both ways.

## Budget

The CLI uses its login (`apiKeySource: none`), so `total_cost_usd` is the
CLI's estimate, not a bill. M3's 160 runs cost 1.51 USD; the plumbing check
cost 0.01 USD per run. The cap is 25 USD. The harness has no cost limit, so
the operator reads the running total from the finished runs' transcripts
while the batches run, and stops them and reports if it projects past the
cap.

## Threats to validity

- The questions were chosen by kind, after dagayn's answers to candidate
  targets were seen (the table above), and C2 was picked from a scan of
  methods whose name 3 or more types share, where dagayn and scip-go
  agreed. They are not a sample of Go questions; per-question results say
  where the effect comes from.
- Keys come from scip-go, whose agreement with dagayn G6 is measuring at the
  same time. Every entry was read against the source, and the interface and
  method-value entries (T3, C3) are by hand.
- Test reach is per function and per top-level test: a test counts when
  any block of the function ran, as in M2.
- Six runs and the candidate's graph builds share the machine, so time is
  compared only between the arms of one invocation.
- A concurrent session could reinstall the dagayn uv tool during the runs;
  the meta files' `graph_before` record the extractor versions each run
  built with, and the results report any run not at go 12.
- Agents can break the rule not to run the tests; transcripts are checked
  and any such run is reported.

## Changes after registration

(none)
