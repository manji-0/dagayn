# Pre-registration: G8, the Go agent experiment rerun

<!-- derived-from ../g7_go/PREREG.md -->
<!-- derived-from ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04 -->

This file was written and committed before any agent run on these
questions. `PREREG.sha256` holds the SHA-256 of every registered file. The
results name the commit that registered them.

## Purpose

G7 (plan item 10) did not meet its bar: tokens 1.00x, time 0.91x (Holm p =
0.21), at 78 against 74 of 80 exact. Four of the five defects it found were
then fixed in 11.0.0 (#260, #258, #250, #270), against G7's own questions.
G8 measures Go again with the design of G7 and decides whether Go is
first-class for 11.1.0 ([decisions](../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04)).

- **Primary:** eight new questions in G7's four kinds, four on testify
  v1.12.1 and four on compress v1.20.1, drawn from the keys' data before
  any dagayn answer to them existed. The bar is tested on these only.
- **Secondary, report only:** G7's eight questions, unchanged
  (`../g7_go/tasks_*.json`), rerun on the same dagayn. They show what the
  fixes did to G7's numbers and decide nothing.

| Kind | testify v1.12.1 (959dbda) | compress v1.20.1 (5d880f2) |
|---|---|---|
| Which tests execute a function | T5 `diffArguments` (15 tests) | C5 `(*File).OpenRaw` (3) |
| Callers of a method on a receiver whose type the call site does not name | T6 `(*Assertions).JSONEq` (10) | C6 gzip `(*Writer).init` (3) |
| Callers through an interface or a function value | T7 `(*Suite).SetT` (3) | C7 gzkp `NewWriter` (3) |
| What a change left unedited | T8 `YAMLEq` (13) | C8 zstd `(*blockHeader).setSize` (4) |

The prompts are in `tasks_testify.json` and `tasks_compress.json`. Each
target is `git archive` of the tag's commit plus one change commit
(`testify-change.patch`, `compress-change.patch`), so `HEAD~1..HEAD` is the
change T8 and C8 ask about. The prompts forbid running the tests or the
repository's code (`go test`, `go run`), as G7's did.

## How the questions were picked

<!-- derived-from ../g7_go/RESULTS.md#what-can-and-cannot-be-said -->

G7's eight questions were chosen after dagayn's answers to them were seen.
G8's were picked by a fixed rule and a seeded draw, and the commits show
the order:

| Commit | What it holds |
|---|---|
| f688a96b | the rule (this section's first version), `keys/pools.py`, `keys/draw.py`, and `pools.json` |
| c72ada0d | `draw.json`, the two change commits, the prompts, and the keys |
| the registering commit | `dagayn_answers.json`, the first time dagayn saw these questions, and this file |

1. `keys/build_inputs.sh` builds, for each repository, the tag's archive
   (the harness's target with no change commit): the scip-go v0.2.7 index
   (`scipdump`, `sciprels`), every `func` declaration (`gofuncs`), and two
   per-test coverage runs of every package without `-short`
   (`per_test_cover.py`). None of it uses dagayn.
2. `keys/pools.py` lists the candidates of each question kind from those
   files. It leaves out functions in `_test.go` files and the eight G7
   targets. A caller is a `func` declaration that holds a scip-go reference
   to the target followed by `(`; any other reference is a value.
   - Kind 1, which tests execute a function: 2 to 15 top-level `Test`
     functions executed it in both coverage runs, and fewer than half of
     those tests contain its name.
   - Kind 2, callers of a method on a receiver whose type the call site does
     not name: a method with 3 to 20 callers whose name 3 or more `func`
     declarations of the repository share.
   - Kind 3, callers through an interface or a function value: the only
     method of the repository that implements a method of an interface the
     repository declares, with at least one call through that interface; or
     a function or method whose every value reference is stored in a field or
     package variable, each of which is called.
   - Kind 4, what a change left unedited: a function or method with 6 to 15
     callers, in 2 or more files, 2 or more of them outside test files.
   - Kinds 2 and 4 also need the key to be the direct callers: no value
     reference, no call through an interface the method implements, and no
     call outside a function.
3. `keys/draw.py` draws from `pools.json`. Its seed is the first 8 hex
   digits of the SHA-256 of `pools.json` (`5a6c7b55...`, seed 1517058901).
   For each repository, testify then compress, and each kind, 1 to 4, it
   takes one target uniformly from the pool sorted by name, drawing again
   on a target the repository already has. For kind 4 it then draws 2 of
   the target's callers outside test files.
4. The kind 4 change commit edits the target's body, so that its behaviour
   changes and its signature does not, and the two drawn callers, and
   nothing else. It adds or removes no reference to any of the repository's
   four targets. It was written after the draw, by hand.

| Pool | testify | compress |
|---|---|---|
| Kind 1 | 59 | 221 |
| Kind 2 | 13 | 60 |
| Kind 3 | 3 | 11 |
| Kind 4 | 19 | 68 |

The rule was written by someone who had read G7's results, and it was
adjusted while looking at the pools' sizes, not at any draw: kind 2 first
required 3 types to declare the name, which left 2 testify candidates, and
a file with no `func` was first skipped by mistake, which hid interface
methods from kind 3. `draw.py pools.json` reproduces `draw.json`.

## The change commits

- testify, "assert: ignore the whitespace around YAML documents in
  YAMLEq's shortcut": `YAMLEq`'s same-bytes shortcut compares the trimmed
  strings. The drawn callers `YAMLEqf` and `(*Assertions).YAMLEq` are
  rewritten without a change in behaviour (a local variable).
- compress, "zstd: keep a block header's size within its 21 bits":
  `setSize` masks its argument to 21 bits. The drawn callers `encodeLits`
  and `encodeRawTo` are rewritten without a change in behaviour (nested
  `appendTo` calls).

Both targets build and pass `go vet` on the changed packages. The whole
testify suite and every compress test run without failure under the
change (both coverage runs: 0 non-zero exits).

## Arms

| Arm | Setup |
|---|---|
| candidate | dagayn at main eab069d7 (11.0.0, go extractor 15), as the `dagayn` uv tool reinstalled from that commit (`uv tool install --force --reinstall --python 3.13 --editable <checkout at eab069d7>`, with the 1.99.1 toolchain first on PATH). Before the runs, `dagayn._core.extractor_versions()` must report go 15, and every run's `graph_before` must show go 15. Each run clones the target and runs `dagayn build --local-embedding none` (the harness default). The MCP server is `dagayn serve`. |
| baseline | No MCP server. |

No `--scip`, and `scip-go` is not on the runs' PATH, as in G7: Go's bar is
met on the extractor alone (decision of 2026-10-26). Both arms have Go
1.24.13 (`mise`) on PATH and neither has `gopls`, `guru`, or `callgraph`.

## Procedure

- Harness: `eval/run_agent_tokens_eval.py` at the registering commit,
  unchanged since G7.
- Model `claude-haiku-5-5`, `--effort medium`, 10 runs per question per
  arm, 3 runs in parallel per invocation, timeout 1,200 s per run.
- Four invocations, two at a time: the primary pair first, then the
  secondary pair. Each pair is one testify and one compress invocation.

```bash
git clone https://github.com/stretchr/testify.git testify-src
git clone https://github.com/klauspost/compress.git compress-src
COMMON="--candidate $HOME/.local/bin/dagayn --baseline none --keep-home --reps 10 -j 3 --timeout 1200"
# Primary (G8)
uv run python eval/run_agent_tokens_eval.py --tasks-file eval/g8_go/tasks_testify.json \
  --source-repo testify-src --seed 20261105 --out out-g8-testify $COMMON &
uv run python eval/run_agent_tokens_eval.py --tasks-file eval/g8_go/tasks_compress.json \
  --source-repo compress-src --seed 20261106 --out out-g8-compress $COMMON &
wait
# Secondary (G7's questions, G7's seeds)
uv run python eval/run_agent_tokens_eval.py --tasks-file eval/g7_go/tasks_testify.json \
  --source-repo testify-src --seed 20261103 --out out-g7-testify $COMMON &
uv run python eval/run_agent_tokens_eval.py --tasks-file eval/g7_go/tasks_compress.json \
  --source-repo compress-src --seed 20261104 --out out-g7-compress $COMMON &
wait
# Analysis
python eval/g7_go/analyze.py out-g8-testify out-g8-compress --seed 20261105
python eval/g7_go/analyze.py out-g8-testify out-g8-compress --seed 20261105 --include-failed
python eval/g7_go/analyze.py out-g7-testify out-g7-compress --seed 20261103
```

- `--keep-home` keeps the CLI's login and passes `--setting-sources
  project,local --disable-slash-commands`. Neither target has a
  `CLAUDE.md` or `AGENTS.md`.
- No run happened before registration. Before the primary pair, the run
  task makes one plumbing check per target, 1 run per arm, on the
  question G7's check used (the methods of `CollectT`; the exported methods
  of xxhash's `Digest`). It checks the login, the MCP connection
  (`dagayn:connected`), the change commit, and scoring. It is not part of
  the results.

## Answer keys

Each key was built without dagayn, from scip-go v0.2.7 and Go coverage on
the harness's target (archive plus change commit), by
`keys/make_keys.py`, which also writes the prompts from one template per
kind. `make_keys.py` without `--write` recomputes all eight keys from the
inputs and agrees with the tasks files. Every entry was read against the
source.

Tooling: Go 1.25.3 (`GOTOOLCHAIN=go1.25.3`, from the module cache's
`golang.org/toolchain`), scip-go v0.2.7 built with it (SHA-256
`b1d476a7...b546f`, the same binary as G7's), and the G7 tools `scipdump`,
`gofuncs`, `per_test_cover.py`, and `edited_functions.py`. `sciprels`
prints the index's implementation relationships.

```bash
B=build/g8   # git-ignored
GOTOOLCHAIN=go1.25.3 GOBIN=$PWD/$B go install github.com/scip-code/scip-go/cmd/scip-go@v0.2.7
(cd eval/g7_go/keys/scipdump && GOTOOLCHAIN=go1.25.3 GOFLAGS=-mod=mod go build -o ../../../../$B/scipdump .)
(cd eval/g8_go/keys/sciprels && GOTOOLCHAIN=go1.25.3 GOFLAGS=-mod=mod go build -o ../../../../$B/sciprels .)
(cd eval/g7_go/keys/gofuncs && go build -o ../../../../$B/gofuncs .)
# targets with make_target() from run_agent_tokens_eval.py, then for each:
sh eval/g8_go/keys/build_inputs.sh $B $B/g8-testify/src $B/inputs-g8 testify
sh eval/g8_go/keys/build_inputs.sh $B $B/g8-compress/src $B/inputs-g8 compress
python eval/g8_go/keys/make_keys.py $B/inputs-g8 $B/g8-testify/src $B/g8-compress/src
```

- **T5 (15), C5 (3), test reach.** The `Test` functions that executed the
  target in both coverage runs. `diffArguments` runs only from
  `(*Mock).MethodCalled` when a call matches no expectation but a closest
  one exists, so the 15 tests are the mock tests of mismatched arguments;
  none of them names `diffArguments`. `OpenRaw` runs in `TestReader`
  (through `readTestFile`) and in `TestWriterCopy` and `TestWriterCopyKP`
  (through `(*Writer).Copy`).
- **T6 (10), C6 (3), callers.** T6: the ten `TestJSONEqWrapper_*` tests,
  where `assert := New(t)` makes `assert.JSONEq` the method, not the
  package function. C6: `NewWriterLevel`, `NewWriterWindow`, and
  `(*Writer).Reset`, among 29 declarations named `init`.
- **T7 (3), C7 (3), interface and function value.** T7: `(*Suite).Run` and
  `TestSuiteGetters` call it directly, and `Run` calls `suite.SetT` on a
  `TestingSuite`, whose only `SetT` in the repository is `Suite`'s. C7:
  `TestGzipDoubleClose` calls it directly; `NewWrapper` stores it as the
  default `GzipWriterFactory.New`, which `(*GzipResponseWriter).init` and
  `(*config).validate` call. Not counted: `NewWrapper`, which only assigns.
- **T8 (13), C8 (4), the change.** The callers less the functions the
  change edits (`edited_functions.py`: exactly the target and the two
  drawn callers). T8: 12 `TestYAMLEq_*` tests and `require.YAMLEq`. C8:
  `encode`, `encodeRaw`, `encodeRLE`, and `(*Encoder).encodeAll`.

The keys of T5 to T7 and C5 to C7, and the callers of T8 and C8, are the
same when derived on the archives without the change.

## What dagayn answers (checked after the keys were committed)

`keys/dagayn_answers.py` makes G7's first calls on graphs that the
worktree's build of eab069d7 made as the harness makes them
(`dagayn_answers.json`; all four graphs at go 15, full builds):

| Q | dagayn's first answer against the key |
|---|---|
| T5 | `tests_for` 0 rows, `counts_as_tested` true; `callers_of(depth=6)` 15 / 15 among 77 tests |
| T6 | `callers_of` 10 / 10, nothing extra |
| T7 | `callers_of` 2 / 3: misses `Run` (the interface call) |
| T8 | `callers_of` 15 rows = the 13 + the 2 edited callers; `review_tool` names no unchanged caller |
| C5 | `tests_for` 1 row (`readTestFile`, a helper); `callers_of(depth=6)` 3 / 3, 3 tests in 6 rows |
| C6 | `callers_of` 3 / 3, nothing extra |
| C7 | `callers_of` 1 / 3: misses the two calls through `GzipWriterFactory.New`, and does not say that `NewWrapper` stores the function |
| C8 | `callers_of` 6 rows = the 4 + the 2 edited callers; `review_tool` says nothing beyond the diff needs checking |

G7's questions on the same build, against G7's keys (G7's numbers in
brackets): T1 `tests_for` 0 rows and not tested, depth 6 gives 1 test,
`TestError`, which is not in the key, 0 / 7 [10 wrong rows, 0 / 7]; T2 9 / 9 [9 / 9
and itself]; T3 13 / 14 [13 / 14]; T4 12 rows = 10 + 2 edited [same]; C1
`tests_for` 0 rows, 4 / 4 among 65 tests [same]; C2 10 / 10 [same]; C3
1 row, `init`, now named as a use of the value [0 rows]; C4 11 rows = 9 + 2
edited [same].

Expected before any run: dagayn's answer is complete on T6 and C6, on T8
and C8 after removing the edited callers, and on C5 at depth 6. It misses
one of three on T7 and two of three on C7, and buries T5's 15 tests among
77. Going by G7, the bar is not certain to hold: G7 met equal accuracy but
not 0.85x where the answers were this good.

## Scoring

As G7:

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

Two-sided, alpha 0.05, as G7 (`../g7_go/analyze.py`, unchanged), on the
eight primary questions:

- H1 total tokens differ, H2 time differs, H3 F1 differs.
- Effect: the mean over questions of the per-question difference in mean
  log tokens (log time; F1), candidate minus baseline. p from 10,000
  permutations of the arm labels within each question (seed 20261105).
  Holm over the three p-values.
- Secondary: the same per repository (Holm over three) and per question
  (Holm over eight, per measure). Descriptive: exact matches, turns, dagayn
  calls, Bash / Read / Grep / Glob calls after the first dagayn call.
- The G7 rerun gets the same analysis (seed 20261103), reported beside
  G7's numbers per question. It is not part of any test of the bar.

## The bar

G7's, from the plan's item 10: the bar is met when, pooled over the eight
primary questions,

- the token ratio is at most 0.85 with Holm p below 0.05, or the time ratio
  is at most 0.85 with Holm p below 0.05 (both from the same three-measure
  Holm family), and
- the dagayn arm has no fewer than the baseline's exact matches minus 2
  (3 or more fewer fails).

Go is first-class when this bar and the M2 gate both hold. If the bar is
not met, the defects the runs show become the rest of the Go work. The
criteria do not change after the runs.

## Exclusions and exceptions

- A run is failed when it has no result event, ends with an error, times
  out, or (candidate) its MCP server did not connect. Failed runs are
  recorded, not rerun, and left out of the primary analysis. The
  sensitivity analysis (`--include-failed`) counts them with F1 0 and the
  largest tokens and time of their question and arm.
- A candidate run whose graph metadata changed during the run
  (`graph_before` and `graph_after` in its meta file), or whose graph is
  not at go 15, is reported.
- If a key proves wrong, it stays registered; the results report the error
  and score both ways.

## Budget

The CLI uses its login, so `total_cost_usd` is the CLI's estimate, not a
bill. G7's 160 runs cost 1.94 USD, so G8's 320 are expected near 4 USD.
The cap is 25 USD. The harness has no cost limit, so the operator reads
the running total from the finished runs while the batches run, and stops
them and reports if it projects past the cap.

## Threats to validity

- The questions are a draw from rule-defined pools, not a sample of Go
  questions. The rule was written after G7's results were read; it is
  committed with the pools, and the draw cannot be steered without changing
  `pools.json`, which changes the seed.
- Pools of 3 (testify kind 3) give the draw little room.
- The change commits were written by hand after the draw. Their size and
  wording may differ from what a real change of those functions would be.
- Keys come from scip-go and coverage. The interface and function-value
  entries (T7, C7) count a call that can run the target at run time; every
  such entry was read against the source.
- Test reach is per function and per top-level test: a test counts when
  any block of the function ran, as in M2 and G7.
- Six runs and the candidate's graph builds share the machine, so time is
  compared only between the arms of one invocation.
- A concurrent session could reinstall the dagayn uv tool during the runs
  (11.1.0 work lands at the same time); the meta files' `graph_before`
  record the extractor versions each run built with.
- Agents can break the rule not to run the tests; transcripts are checked
  and any such run is reported.

## Changes after registration

(none)
