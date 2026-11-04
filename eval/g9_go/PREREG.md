# Pre-registration: G9, the Go agent experiment under the revised bar

<!-- derived-from ../g8_go/PREREG.md -->
<!-- constrained-by ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04-after-g8 -->

This file was written and committed before any agent run on these
questions. `PREREG.sha256` holds the SHA-256 of every registered file. The
results name the commit that registered them.

## Purpose

G8 did not meet its bar: tokens 1.20x (Holm p = 0.0003), time 0.97x, 73
against 79 of 80 exact. Six fixes followed (#278, #279, #277, #280, #282,
#283; go extractor 17). After G8, the owner redefined Go's agent bar
([decisions](../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04-after-g8)):
0.85x is tested only on LONG questions, those of the kinds where the
baseline needs many steps, and SHORT questions get a guard instead of a
gain requirement. This is a change of the goal after a result. G9 tests Go
against the redefined bar on twelve new questions and decides whether Go
is first-class for 11.1.0.

| Q | Repository | Length (stratum) | Kind | Target | Key |
|---|---|---|---|---|---|
| T9 | testify | LONG (reach) | 1, which tests execute it | `assert/assertion_compare.go::compareTwoValues` | 10 tests |
| T10 | testify | LONG (name) | 2, callers on a receiver | `assert/assertion_forward.go::Assertions.True` | 18 |
| T11 | testify | LONG (name) | 2, callers on a receiver | `assert/assertion_forward.go::Assertions.False` | 18 |
| T12 | testify | SHORT | 2, callers on a receiver | `assert/assertion_forward.go::Assertions.Contains` | 6 |
| C9 | compress | LONG (reach) | 1, which tests execute it | `zstd/encoder.go::Encoder.ReadFrom` | 9 tests |
| C10 | compress | LONG (name) | 3, function value | `gzhttp/writer/zstdkp/zstdkp.go::NewWriter` | 1 |
| C11 | compress | LONG (name) | 2, callers on a receiver | `gzip/gzip.go::Writer.Reset` | 3 |
| C12 | compress | SHORT | 3, interface | `zstd/enc_base.go::fastBase.CRC` | 5 |
| K1 | kong | LONG (reach) | 1, which tests execute it | `exit.go::exitCodeFromError` | 3 tests |
| K2 | kong | LONG (reach) | 1, which tests execute it | `context.go::flipBoolValue` | 2 tests |
| K3 | kong | SHORT | 2, callers on a receiver | `model.go::Value.IsBool` | 3 |
| K4 | kong | SHORT | 3, function value | `help.go::LineIndenter` | 1 |

The prompts and keys are in `tasks_testify.json`, `tasks_compress.json`,
and `tasks_kong.json`. Each target is `git archive` of the tag's commit,
with no change commit: the draw holds no kind 4 question. The prompts
forbid running the tests or the repository's code (`go test`, `go run`),
as G7's and G8's did.

## How the questions are picked

<!-- derived-from ../g8_go/RESULTS.md#where-dagayn-won-and-lost -->

### Question length

The owner's revised bar
([decisions](../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04-after-g8))
tests 0.85x only on LONG questions, those of the kinds where the baseline
needs many steps. The kinds and their lengths are fixed here, before the
draw, from the baseline (no dagayn) runs of G7 and G8 alone. No G9 run
exists.

G8's four kinds do not separate by length on their own. Test reach (kind 1)
is long in every run. Kinds 2 to 4 hold both long and short questions:

| Kind | Baseline median turns, every registered run | Median |
|---|---|---|
| 1, which tests execute a function | 8.5, 8.5, 13, 17, 19.5, 22.5 | 15 |
| 2, callers on a receiver | 5, 5.5, 7, 9, 11.5, 13 | 8 |
| 3, callers through an interface or a function value | 5, 5.5, 5.5, 7.5, 8, 9 | 6.5 |
| 4, what a change left unedited | 4.5, 5.5, 6, 6, 11.5, 13 | 6 |

In kinds 2 to 4, what separates the long questions from the short ones is
how often the target's name is called in the repository. The baseline
answers with a grep for the name, and reads each hit. `name_calls` is the
number of lines in the repository's `.go` files (the files `go test ./...`
sees) where the name is followed by `(`, the lines `grep -E '\bName\('`
finds. `keys/length_basis.py` prints, for all sixteen G7 and G8 questions:

| Q | Kind | Target | Baseline median turns | `name_calls` | Length |
|---|---|---|---|---|---|
| T5 | 1 | `mock/mock.go::diffArguments` | 22.5 (G8) | 2 | LONG |
| C5 | 1 | `zip/reader.go::File.OpenRaw` | 13 (G8) | 3 | LONG |
| T1 | 1 | `assert/assertions.go::CollectT.Errorf` | 8.5 (G7), 8.5 (G8 rerun) | 124 | LONG |
| C1 | 1 | `flate/stateless.go::statelessEnc` | 17 (G7), 19.5 (G8 rerun) | 3 | LONG |
| T2 | 2 | `assert/assertion_forward.go::Assertions.Equal` | 13 (G7), 11.5 (G8 rerun) | 373 | LONG |
| C2 | 2 | `zstd/internal/xxhash/xxhash.go::Digest.Reset` | 7 (G7), 9 (G8 rerun) | 273 | LONG |
| C7 | 3 | `gzhttp/writer/gzkp/gzkp.go::NewWriter` | 9 (G8) | 270 | LONG |
| C4 | 4 | `flate/deflate.go::Writer.Reset` | 11.5 (G7), 13 (G8 rerun) | 273 | LONG |
| T6 | 2 | `assert/assertion_forward.go::Assertions.JSONEq` | 5 (G8) | 52 | SHORT |
| C6 | 2 | `gzip/gzip.go::Writer.init` | 5.5 (G8) | 127 | SHORT |
| T3 | 3 | `suite/suite.go::Suite.T` | 8 (G7), 7.5 (G8 rerun) | 23 | SHORT |
| T7 | 3 | `suite/suite.go::Suite.SetT` | 5 (G8) | 9 | SHORT |
| C3 | 3 | `flate/deflate.go::compressor.storeFast` | 5.5 (G7), 5.5 (G8 rerun) | 1 | SHORT |
| T4 | 4 | `assert/assertions.go::ObjectsAreEqual` | 6 (G7), 6 (G8 rerun) | 16 | SHORT |
| T8 | 4 | `assert/assertions.go::YAMLEq` | 5.5 (G8) | 54 | SHORT |
| C8 | 4 | `zstd/blockenc.go::blockHeader.setSize` | 4.5 (G8) | 12 | SHORT |

The rule:

- **LONG**: kind 1, or a kind 2 to 4 question whose target's `name_calls`
  is at least 200.
- **SHORT**: a kind 2 to 4 question whose target's `name_calls` is below
  200.

On the 24 baseline medians above, LONG has 13 (median 11.5 turns, 7 to
22.5) and SHORT has 11 (median 5.5, 4.5 to 8). The threshold 200 lies in
the gap between the largest short question (127, C6) and the smallest long
one (270, C7). The basis is thin: twelve kind 2 to 4 questions, one gap,
and the rule was found by looking at them. It was chosen from the baseline
alone. dagayn's results on these questions were known (G8's results), and
the rule does not use them.

### Pools

`keys/g9pools.py` takes G8's four kinds unchanged (`../g8_go/keys/pools.py`
and its rule in [G8's pre-registration](../g8_go/PREREG.md#how-the-questions-were-picked)),
removes G7's and G8's sixteen targets, and adds `name_calls` and the length
to every candidate. It runs on three repositories, each the tag's
`git archive`:

| Repository | Tag (commit) | Why |
|---|---|---|
| testify | v1.12.1 (959dbda) | G7's and G8's |
| compress | v1.20.1 (5d880f2) | G7's and G8's |
| kong | v1.15.0 (14c97a2) | added: third-party, with history, and none of the fixes since G8 was tuned on it |

kong is `github.com/alecthomas/kong`, cloned with its history. It is one
package of 347 production functions that builds and tests with Go 1.25.3
(269 top-level tests, 0 failures in both coverage runs). The key inputs
come from G8's `keys/build_inputs.sh`, unchanged. G8 used testify and
compress, and the fixes since G8 were tuned on their questions, so a
third repository is the only part of G9 those fixes have not seen.

| Pool (LONG / SHORT) | testify | compress | kong |
|---|---|---|---|
| Kind 1 | 57 / 0 | 220 / 0 | 41 / 0 |
| Kind 2 | 5 / 7 | 7 / 52 | 1 / 3 |
| Kind 3 | 0 / 2 | 3 / 7 | 0 / 3 |
| Kind 4 | 0 / 18 | 7 / 60 | 0 / 11 |

The testify and compress pools equal G8's `pools.json` less G8's eight
targets.

### Draw

`keys/g9draw.py` draws from `pools.json`. Its seed is the first 8 hex digits
of the SHA-256 of `pools.json`. There are three strata:

- **reach** (LONG): kind 1.
- **name** (LONG): kinds 2 to 4 with `name_calls` at least 200.
- **short** (SHORT): kinds 2 to 4 with `name_calls` below 200.

| Repository | reach | name | short |
|---|---|---|---|
| testify | 1 | 2 | 1 |
| compress | 1 | 2 | 1 |
| kong | 2 | 0 | 2 |

That is 8 LONG questions (4 reach, 4 name) and 4 SHORT, four per
repository. kong has one name candidate, so its LONG questions are test
reach. Each draw picks a kind uniformly among the stratum's kinds that
still have a candidate, then a target uniformly from it, sorted by name. A
repository draws no target twice and at most one kind 4 target, because
each repository gets one change commit. For a kind 4 target the draw then
takes 2 of its callers outside test files, which the change edits with the
target, as in G8.

8 and 4 follow the suggested sizes. The primary test needs the LONG
questions, and SHORT questions are only a guard on a point estimate, so
they get the smaller share.

### Order of commits

The commits on `eval/g9-go-prereg` show the order:

| Commit | What it holds |
|---|---|
| 9b28e8bc | the rule (this section), `keys/g9pools.py`, `keys/g9draw.py`, `keys/length_basis.py`, `pools.json`, `analyze.py`, and the plan's decisions |
| 9e9c2c67 | `draw.json`, `keys/g9keys.py`, and the prompts and keys |
| the registering commit | `dagayn_answers.json` and `keys/g9answers.py`, the first time dagayn saw these questions, `plumbing_kong.json`, and this file in full |

`g9draw.py pools.json` reproduces `draw.json` (seed 2154903086). The rule
above is as committed in 9b28e8bc; the registering commit adds only this
subsection to it.

## Answer keys

Each key was built without dagayn, from scip-go v0.2.7 and two per-test
coverage runs on the tags' archives, by `keys/g9keys.py`. It imports G8's
`make_keys.py` unchanged for the prompt templates and the key rule of each
kind. The one change is the package name in the prompt, which G9 reads from
the file's `package` clause: G8 took the directory's name, which is empty
for a kong file at the repository root. `g9keys.py` without `--write`
recomputes all twelve keys from the inputs and agrees with the tasks files.

Tooling: G8's (Go 1.25.3, scip-go v0.2.7 with SHA-256 `b1d476a7...b546f`,
`scipdump`, `sciprels`, `gofuncs`, `per_test_cover.py`).

```bash
B=build/g9   # git-ignored; scip-go, scipdump, sciprels, gofuncs built as in ../g8_go/PREREG.md
git clone https://github.com/alecthomas/kong.git $B/kong-src   # testify-src, compress-src as in G8
# base-NAME/src: make_target() from run_agent_tokens_eval.py on the tag's commit, no change
for n in testify compress kong; do
  sh eval/g8_go/keys/build_inputs.sh $B $B/base-$n/src $B/inputs-base $n
done
python eval/g9_go/keys/g9pools.py $B/inputs-base testify=$B/base-testify/src \
  compress=$B/base-compress/src kong=$B/base-kong/src > eval/g9_go/pools.json
python eval/g9_go/keys/g9draw.py eval/g9_go/pools.json > eval/g9_go/draw.json
python eval/g9_go/keys/g9keys.py $B/inputs-base testify=$B/base-testify/src \
  compress=$B/base-compress/src kong=$B/base-kong/src
python eval/g9_go/keys/length_basis.py $B/base-testify/src $B/base-compress/src
```

The callers keys were read against the source. The test reach keys are the
coverage runs', as in G7 and G8, and were spot-checked.

- **T9 (10), C9 (9), K1 (3), K2 (2), test reach.** The `Test` functions that
  executed the target in both coverage runs. T9: the comparison tests of
  `assertion_compare_test.go` (`TestGreater`, `TestLess`, `TestPositive`,
  and others), 3 of which name `compareTwoValues`. C9: nine zstd tests of
  `ReadFrom` and the encoder. K1: `TestUsageOnError`,
  `TestShortUsageOnError`, and `TestCustomShortUsageOnError`. K2:
  `TestNegatableFlag` and `TestBoolPtrNegated`.
- **T10 (18), T11 (18), T12 (6), C11 (3), K3 (3), callers on a receiver.**
  T10 and T11: the `*Wrapper` tests of `forward_assertions_test.go`, the
  HTTP tests, and one suite test each. T10 counts `TestUnInitializedSuites`,
  where `suite.Assert().True(true)` takes its receiver from a call's
  result. T11 counts `subtestPanicSuite.TestSubtestPanic`, where `s.False`
  goes through the `*assert.Assertions` that `suite.Suite` embeds. C11:
  gzkp's `NewWriter` (`gzw.Reset(w)` on a pooled `*gzip.Writer`),
  `TestWriterReset`, and `benchmarkGzipN`. The `zw.Reset` of
  `gzip/example_test.go` is the standard library's `compress/gzip` and is
  not counted. K3: `formatFlag` (`flag.IsBool()` on a `*Flag`, which embeds
  `*Value`), `Flag.String`, and `Value.Summary`.
- **C10 (1), K4 (1), function value.** C10: `NewWrapper` stores zstdkp's
  `NewWriter` as the default `ZstdWriterFactory.New`, which
  `(*GzipResponseWriter).init` calls. `NewWrapper` only assigns it. K4:
  `TestHelpTree` sets `HelpOptions.Indenter` to `LineIndenter`, and
  `(*HelpOptions).CommandTree` calls `h.Indenter`. `TestHelpTree` only
  assigns it.
- **C12 (5), interface.** The five `Encoder` methods that call `CRC()` on
  the `encoder` interface, whose only implementation in the repository is
  `fastBase`'s: `ReadFrom`, `encodeAll`, `readFromJobs`, `writeBlocks`, and
  `writeJobs`.

## What dagayn answers (checked after the keys were committed)

`keys/g9answers.py` makes G8's first calls (`../g8_go/keys/dagayn_answers.py`,
imported unchanged) on graphs that the candidate built as the harness
builds them (`dagayn_answers.json`; all three graphs full builds at go 17):

| Q | Length | dagayn's first answer against the key |
|---|---|---|
| T9 | LONG | `tests_for` 3 rows; `callers_of(depth=6)` 10 / 10 among 58 rows, nothing extra |
| T10 | LONG | `callers_of` 17 / 18: misses `TestUnInitializedSuites`, and the summary names its two lines (`suite_test.go:778` and `:786`) among 3 unbound calls of `True` |
| T11 | LONG | `callers_of` 18 / 18, nothing extra, and every call named `False` is bound |
| T12 | SHORT | `callers_of` 6 / 6, nothing extra |
| C9 | LONG | `tests_for` 6 rows; `callers_of(depth=6)` 9 / 9 and 2 tests not in the key (`TestHeader_Decode`, `TestEncoder_EncoderSilesia`) |
| C10 | LONG | `callers_of` 0 / 1: one row, `NewWrapper`, named as a use of the value, with "read where the value goes" |
| C11 | LONG | `callers_of` 3 / 3, nothing extra; the summary says 80 calls named `Reset` go to no definition |
| C12 | SHORT | `callers_of` 5 / 5, nothing extra, confidence medium |
| K1 | LONG | `tests_for` 0 rows; `callers_of(depth=6)` 3 / 3 and 1 test not in the key among 10 rows |
| K2 | LONG | `tests_for` 0 rows; `callers_of(depth=6)` 1 / 2 among 118 rows (109 tests), cut short |
| K3 | SHORT | `callers_of` 3 / 3, nothing extra |
| K4 | SHORT | `callers_of` 0 / 1: one row, `TestHelpTree`, named as a use of the value |

Expected before any run: dagayn's answer is complete on T9, T11, C11, T12,
C12, and K3, nearly complete on T10 and C9, and short on C10, K2, and K4.
K2's 2 tests are buried among 109. G7 and G8 did not reach 0.85x where the
answers were this good, so the LONG bar is not likely to hold on these
answers alone.

## Arms

| Arm | Setup |
|---|---|
| candidate | dagayn at main 4fdda6e9 (go extractor 17, rust 28): the `dagayn` uv tool at `$HOME/.local/bin/dagayn`, installed non-editable from the main checkout on Python 3.14 before registration and not reinstalled for G9. Its `_core` (`_core.cpython-314-darwin.so`) has SHA-256 `cddb3249...b2897d75`. Before the runs, `$HOME/.local/share/uv/tools/dagayn/bin/python -c "import dagayn._core as c; print(c.extractor_versions())"`, run outside the checkout, must report go 17, and every run's `graph_before` must show go 17. Each run clones the target and runs `dagayn build --local-embedding none` (the harness default). The MCP server is `dagayn serve`. |
| baseline | No MCP server. |

No `--scip`, and `scip-go` is not on the runs' PATH, as in G7 and G8. Both
arms have Go 1.24.13 (`mise`) on PATH and neither has `gopls`, `guru`, or
`callgraph`.

## Procedure

- Harness: `eval/run_agent_tokens_eval.py`, unchanged since G7 (its
  SHA-256 is G8's).
- Model `claude-haiku-5-5`, `--effort medium`, 10 runs per question per
  arm, 3 runs in parallel per invocation, timeout 1,200 s per run.
- Three invocations: testify and compress at the same time, then kong.
- Before them, a plumbing check: 1 run per arm of `plumbing_kong.json` (the
  12 methods of `Kong`) on kong, the one new target. It checks the login,
  the MCP connection (`dagayn:connected`), and scoring. It is not part of
  the results. G8's plumbing check covered testify and compress with the
  same harness.

```bash
B=build/g9
COMMON="--candidate $HOME/.local/bin/dagayn --baseline none --keep-home --reps 10 -j 3 --timeout 1200 --model claude-haiku-5-5 --effort medium"
# Plumbing check (not part of the results)
uv run python eval/run_agent_tokens_eval.py --tasks-file eval/g9_go/plumbing_kong.json \
  --source-repo $B/kong-src --seed 20261110 --out out-g9-plumbing $COMMON --reps 1
# Runs
uv run python eval/run_agent_tokens_eval.py --tasks-file eval/g9_go/tasks_testify.json \
  --source-repo $B/testify-src --seed 20261107 --out out-g9-testify $COMMON &
uv run python eval/run_agent_tokens_eval.py --tasks-file eval/g9_go/tasks_compress.json \
  --source-repo $B/compress-src --seed 20261108 --out out-g9-compress $COMMON &
wait
uv run python eval/run_agent_tokens_eval.py --tasks-file eval/g9_go/tasks_kong.json \
  --source-repo $B/kong-src --seed 20261109 --out out-g9-kong $COMMON
# Analysis
python eval/g9_go/analyze.py out-g9-testify out-g9-compress out-g9-kong --seed 20261107
python eval/g9_go/analyze.py out-g9-testify out-g9-compress out-g9-kong --seed 20261107 --include-failed
```

- `--keep-home` keeps the CLI's login and passes `--setting-sources
  project,local --disable-slash-commands`. None of the targets has a
  `CLAUDE.md` or `AGENTS.md`.
- The candidate is the uv tool, which `uv run` in the checkout does not
  rebuild. Either `uv run python` or a checkout's `.venv/bin/python` may
  run the harness; the results say which did.

## Scoring

As G7 and G8:

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

The test is `analyze.py` in this directory. It imports G7's `analyze.py`
unchanged for the effect, the permutation test, Holm's correction, the
handling of failed runs, and the descriptive numbers. Each question's
length is the `length` field of its task.

- **Effect.** For a set of questions: the mean over the questions of the
  per-question difference in mean log tokens (log time; F1), candidate
  minus baseline. A ratio is the exponential of the log effect.
- **LONG test (primary).** Over the 8 LONG questions (T9, T10, T11, C9, C10,
  C11, K1, K2), two-sided, alpha 0.05: H1 total tokens differ, H2 time
  differs, H3 F1 differs. p from 10,000 permutations of the arm labels
  within each question (seed 20261107). Holm over the three p-values.
- **SHORT guard.** Over the 4 SHORT questions (T12, C12, K3, K4): the
  token ratio as a point estimate, and the exact matches of each arm. Its
  permutation p is reported and decides nothing.
- Secondary: the same family per repository, and per question (Holm over
  the twelve, per measure). Descriptive: exact matches, turns, dagayn
  calls, Bash / Read / Grep / Glob calls after the first dagayn call.

## The bar

Go meets the bar when both of these hold:

- **LONG.** Pooled over the 8 LONG questions, the token ratio is at most
  0.85 with Holm p below 0.05, or the time ratio is at most 0.85 with Holm
  p below 0.05 (both from the LONG Holm family), and the dagayn arm has no
  fewer than the baseline's exact matches on LONG minus 2 (3 or more fewer
  fails, G8's accuracy rule).
- **SHORT.** Pooled over the 4 SHORT questions, the token ratio point
  estimate is at most 1.10, and the dagayn arm's exact matches on SHORT
  are not fewer than the baseline's.

`analyze.py` reports `bar.long.met`, `bar.short.met`, and `bar.met`. Go is
first-class when this bar and the M2 gate both hold. If the bar is not
met, the defects the runs show become the rest of the Go work. The
criteria do not change after the runs.

## Exclusions and exceptions

- A run is failed when it has no result event, ends with an error, times
  out, or (candidate) its MCP server did not connect. Failed runs are
  recorded, not rerun, and left out of the primary analysis. The
  sensitivity analysis (`--include-failed`) counts them with F1 0 and the
  largest tokens and time of their question and arm.
- A candidate run whose graph metadata changed during the run
  (`graph_before` and `graph_after` in its meta file), or whose graph is
  not at go 17, is reported.
- If a key proves wrong, it stays registered; the results report the error
  and score both ways.

## Budget

The CLI uses its login, so `total_cost_usd` is the CLI's estimate, not a
bill. In G8's 320 runs, a run cost a mean of 0.026 USD on the test reach
questions (T1, T5, C1, C5), 0.014 on the questions G9's rule calls LONG by
name (T2, C2, C4, C7), and 0.0065 on the rest. G9 has 80 runs of each
stratum, so its 240 runs are expected near 3.7 USD (80 x 0.026 + 80 x 0.014
+ 80 x 0.0065), and near 5 USD if every test reach run costs as much as
T5's did. The cap is 25 USD. The harness has no cost limit, so the operator
reads the running total from the finished runs, and stops the batches and
reports if it projects past the cap.

## Threats to validity

- **The bar was redefined after G8 missed it.** The owner changed the
  goal, and the record says so. The LONG / SHORT rule was found by looking
  at twelve G7 and G8 questions, with dagayn's results on them known. It
  uses only baseline turns and a grep count, and it was fixed before the
  draw, but its threshold rests on one gap (127 to 270). Under it, G8's
  own questions would not meet the new bar (LONG 1.10x, SHORT 1.26x
  tokens, from `analyze.py` on G8's runs, report only).
- **Length is a property of the kind, not of the run.** A LONG question
  may turn out short for the baseline here. The test does not reclassify
  questions after the runs; the results report each question's baseline
  turns.
- **The fixes were tuned on testify and compress.** #278 to #283 were
  checked against G8's questions, which are on the same two repositories.
  kong is the only repository they have not seen. C10 (zstdkp's
  `NewWriter`) is the twin of G8's C7 (gzkp's `NewWriter`), against which
  #277 was fixed. T10 and T11 are near twins of each other.
- The questions are a draw from rule-defined pools, not a sample of Go
  questions. The allocation gives kong no name question, because its pool
  had one candidate.
- Keys come from scip-go and coverage. The interface and function-value
  entries (C10, C12, K4) count a call that can run the target at run time;
  every such entry was read against the source.
- Test reach is per function and per top-level test: a test counts when
  any block of the function ran, as in M2, G7, and G8.
- Six runs and the candidate's graph builds share the machine, so time is
  compared only between the arms of one invocation.
- A concurrent session could reinstall the dagayn uv tool during the runs.
  The meta files' `graph_before` record the extractor versions each run
  built with, and the `_core` hash is checked before and after.
- Agents can break the rule not to run the tests; transcripts are checked
  and any such run is reported.

## Changes after registration

(none)
