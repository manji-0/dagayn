# Pre-registration: G10, the Go agent experiment on new questions and an untuned repository

<!-- derived-from ../g9_go/PREREG.md -->
<!-- constrained-by ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-05-after-g9 -->
<!-- constrained-by ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04-after-g8 -->

This file was written and committed before any agent run on these
questions, and before any dagayn call on their targets. `PREREG.sha256`
holds the SHA-256 of every registered file. The results name the commit
that registered them.

**G10 measures dagayn through its CLI**: the candidate arm runs
`dagayn tool ...` in the shell, with no MCP server, against a baseline with
no dagayn ([Arms](#arms)). A pass says nothing about the MCP transport.

## Purpose

G9 met the LONG part of the bar (time 0.79x, Holm p = 0.0003) and missed
the SHORT guard (tokens 1.17x against 1.10). Its twelve questions have
since been used for tuning (#287 against C11, #304 against the SHORT runs,
#319 against K3, T10, and T11), and kong, its one untuned repository, is
now tuned on too (#292). After G9 the owner changed the SHORT guard from
1.10 to 1.25, before this draw
([decisions](../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-05-after-g9)).
G10 tests Go against that bar on twelve new questions, half of them on a
fourth repository that no fix since G7 was tuned on.

| Q | Repository | Length (stratum) | Kind | Target | Key |
|---|---|---|---|---|---|
| T13 | testify | LONG (reach) | 1, which tests execute it | `internal/spew/config.go::ConfigState.convertArgs` | 2 tests |
| T14 | testify | LONG (name) | 2, callers on a receiver | `mock/mock.go::Arguments.Error` | 14 |
| C13 | compress | LONG (reach) | 1, which tests execute it | `s2/index.go::Index.LoadStream` | 2 tests |
| C14 | compress | LONG (name) | 3, interface | `gzip/gunzip.go::crcUpdater.Reset` | 1 |
| K5 | kong | LONG (reach) | 1, which tests execute it | `help.go::formatEnvs` | 4 tests |
| K6 | kong | SHORT | 2, callers on a receiver | `context.go::Context.Run` | 10 |
| H1 | chi | LONG (reach) | 1, which tests execute it | `middleware/throttle.go::ThrottleWithOpts` | 6 tests |
| H2 | chi | LONG (reach) | 1, which tests execute it | `middleware/recoverer.go::prettyStack.parse` | 2 tests |
| H3 | chi | LONG (name) | 3, interface | `mux.go::Mux.Get` | 82 |
| H4 | chi | SHORT | 4, what a change left unedited | `middleware/terminal.go::cW` | 4 |
| H5 | chi | SHORT | 3, interface | `mux.go::Mux.Group` | 7 |
| H6 | chi | SHORT | 3, interface | `middleware/logger.go::DefaultLogFormatter.NewLogEntry` | 1 |

The prompts and keys are in `tasks_testify.json`, `tasks_compress.json`,
`tasks_kong.json`, and `tasks_chi.json`. The testify, compress, and kong
targets are `git archive` of the tag's commit with no change commit. The
chi target is `git archive` of v5.5.0 plus one change commit
(`chi-change.patch`), because H4 is a kind 4 question. The prompts forbid
running the tests or the repository's code (`go test`, `go run`), as G7's
to G9's did.

## How the questions are picked

<!-- derived-from ../g9_go/PREREG.md#how-the-questions-are-picked -->

### Question length

G9's rule, unchanged (`../g9_go/keys/g9pools.py`, imported by
`keys/g10pools.py`):

- **LONG**: kind 1 (which tests execute a function), or a kind 2 to 4
  question whose target's `name_calls` is at least 200.
- **SHORT**: a kind 2 to 4 question whose target's `name_calls` is below
  200.

`name_calls` is the number of lines in the repository's `.go` files (the
files `go test ./...` sees) where the target's name is followed by `(`. The
rule's basis is in [G9's pre-registration](../g9_go/PREREG.md#question-length).

### Repositories

| Repository | Tag (commit) | Why |
|---|---|---|
| testify | v1.12.1 (959dbda) | G7's to G9's |
| compress | v1.20.1 (5d880f2) | G7's to G9's |
| kong | v1.15.0 (14c97a2) | G9's |
| chi | v5.5.0 (3a33ba5) | added: no fix since G7 was tuned on it |

chi is `github.com/go-chi/chi/v5`, cloned with its history (842 commits;
v5.5.0 is of 2026-10-14). It has two packages, `chi` and `middleware`, with
189 production functions and 136 top-level tests. It builds and tests with Go
1.25.3, and both coverage runs ended with 0 failures. Its `go.mod` says
`go 1.24`. Its two other `go.mod` files are under `_examples`, which the Go
tools, scip-go, and the prompts' scope all leave out.

How it was chosen. #292 asks for a Go repository with git history that no
fix since G7 was tuned on. The candidates were widely used Go modules of a
size near kong's. A candidate had to pass four checks, in this order:

1. Never used here. A search of the repository (code, tests, fixtures,
   `eval/`, `docs/plans/`, `CHANGELOG.md`) and of the session's scratch
   clones found no use of chi, logrus, urfave/cli, or zap. cobra was
   dropped: it appears in a fixture of `scip_overlay.rs`.
2. One module in the prompts' scope. scip-go and the coverage runs see the
   root module only, while the prompts count every `.go` file of the
   repository outside `_x`, `.x`, and `testdata`. A nested module there
   would put callers in the prompts' scope that the keys cannot see.
   **zap** v1.28.0 failed: `benchmarks`, `exp`, `assets`, and
   `zapgrpc/internal/test` are modules of their own.
3. A `go` directive the key toolchain (Go 1.25.3) builds. **urfave/cli**
   v3.14.0 failed: `go 1.26`.
4. Each stratum non-empty, so the new repository can carry LONG questions
   of both strata. **logrus** v1.10.2 passed checks 1 to 3, but its name
   stratum is empty (36 / 0 / 22 candidates in reach / name / short).
   **chi** passed with 41 / 1 / 25.

Only pool sizes were looked at for check 4, as G9 did when it gave kong no
name question. gorilla/mux was not checked.

### Pools

`keys/g10pools.py` takes G9's pools (`../g9_go/keys/g9pools.py`, imported
unchanged: G8's four kinds, less G7's and G8's targets, with `name_calls` and
the length), and also removes G9's twelve targets. It runs on the four tags'
`git archive`s. The testify, compress, and kong inputs are G9's files; the
script reproduces G9's `pools.json` from them exactly, and G9's
`keys/g9keys.py` reproduces G9's twelve keys from them. chi's inputs come from
G8's `keys/build_inputs.sh`, unchanged.

| Pool (LONG / SHORT) | testify | compress | kong | chi |
|---|---|---|---|---|
| Kind 1 | 56 / 0 | 218 / 0 | 39 / 0 | 41 / 0 |
| Kind 2 | 3 / 6 | 6 / 52 | 1 / 2 | 0 / 0 |
| Kind 3 | 0 / 2 | 2 / 6 | 0 / 2 | 1 / 21 |
| Kind 4 | 0 / 17 | 7 / 60 | 0 / 11 | 0 / 4 |

### Draw

`keys/g10draw.py` draws from `pools.json` with G9's draw
(`../g9_go/keys/g9draw.py`, imported unchanged). The seed is the first 8 hex
digits of the SHA-256 of `pools.json`, a new seed because the pools are new.
The strata are G9's: **reach** (LONG, kind 1), **name** (LONG, kinds 2 to 4
with `name_calls` at least 200), and **short** (SHORT, kinds 2 to 4 below
200). Only the allocation is G10's:

| Repository | reach | name | short | Questions |
|---|---|---|---|---|
| testify | 1 | 1 | 0 | 2 |
| compress | 1 | 1 | 0 | 2 |
| kong | 1 | 0 | 1 | 2 |
| chi | 2 | 1 | 3 | 6 |
| All | 5 | 3 | 4 | 12 |

That is 8 LONG questions and 4 SHORT, as in G9, and 240 runs.

Why chi carries half. The fixes since G7 were tuned on testify, compress,
and, since G9, kong, and G9's own questions have been used for tuning too.
chi is the only part of G10 the fixes have not seen, so it is the only
evidence of how dagayn does on Go it was not tuned on. Weighting it as one
repository in four would let three tuned repositories decide the result.
The tuned repositories stay in, two questions each, as a check that the
fixes did not break what they were tuned for.

- chi's name stratum has one candidate, so its name question is that
  candidate. Its other LONG questions are test reach.
- 3 of the 4 SHORT questions are chi's. G9's SHORT loss came from the one
  untuned repository (kong: K3 1.66x, K4 1.37x), so the guard should
  mostly be tested where nothing was tuned.
- testify and compress have no SHORT question and kong no name question.
  kong's name stratum has one candidate.
- `analyze.py` also reports chi's LONG and SHORT questions on their own, as
  a secondary result.

A repository draws no target twice and at most one kind 4 target. For a kind
4 target the draw takes 2 of its callers outside test files, which the
change commit edits with the target, as in G8.

### Order of commits

The commits on `eval/g10-prereg` show the order:

| Commit | What it holds |
|---|---|
| f85cfcf5 | the rule (this section up to "Draw"), `keys/g10pools.py`, `keys/g10draw.py`, `pools.json`, `analyze.py`, and `fixed_cost.py` |
| 30f21fe7 | `draw.json`, `chi-change.patch`, `keys/g10keys.py`, and the prompts and keys |
| b97e7e7c | `keys/g10answers.py` (not run), `plumbing_chi.json`, this file with the transport left open, and `PREREG.sha256` |
| the registering commit | the owner's transport decision (CLI): the candidate build and harness at d8ecb51f, the `cli` condition line in the tasks files and `plumbing_chi.json` (and in `keys/g10keys.py`, which writes them), the CLI dagayn call in `analyze.py` and `fixed_cost.py`, this file, and `PREREG.sha256` |

The first three were committed on c309242c as b993cc48, 0b3feb60, and
665d191b, and rebased onto d8ecb51f without change. `g10draw.py pools.json`
reproduces `draw.json` (seed 1748109591). The subsections above, from
"Question length" to "Draw", are as committed in f85cfcf5. Later commits
replace its one-line status note with the header above and add the rest of
this file. No commit after 30f21fe7 changes a question, prompt, or key.

## The change commit

<!-- derived-from ../g8_go/PREREG.md#the-change-commits -->

The draw gave chi a kind 4 target, `cW` (`middleware/terminal.go`), with
the edited callers `DefaultLogFormatter.NewLogEntry`
(`middleware/logger.go`) and `prettyStack.decorateSourceLine`
(`middleware/recoverer.go`). G8's rule applies: the change edits the
target's body, so that its behaviour changes and its signature does not,
and the two drawn callers, and nothing else. It adds or removes no
reference to any of chi's six targets. It was written after the draw, by
hand.

- chi, "middleware: write cW's string as it is when it has no arguments":
  `cW` writes `s` with `io.WriteString` when `args` is empty, so a `%` in
  it is no longer read as a verb. `NewLogEntry` writes `"from " + clientIP
  + " - "` in one `WriteString` call instead of three, and
  `decorateSourceLine` cuts `lineno` with a scoped variable instead of
  reusing `idx`. Neither changes behaviour.

`git diff HEAD~1 HEAD` on the target edits exactly `cW`, `NewLogEntry`, and
`decorateSourceLine` (`../g7_go/keys/edited_functions.py`). The change
passes `gofmt` and `go vet`, the module builds, and both coverage runs on
the changed target ended with 0 failures in 136 tests. `NewLogEntry` is
also H6's target. H6 asks for its callers, which the change does not touch.

## Answer keys

Each key was built without dagayn, from scip-go v0.2.7 and two per-test
coverage runs, by `keys/g10keys.py`. It imports G9's `keys/g9keys.py`
unchanged (G8's prompt templates and key rule of each kind, with the
package name from the file's `package` clause). testify, compress, and kong
use the inputs the pools were built from. chi's inputs were built again on
its harness target (archive plus change commit), as G8 did for its change
commits. `g10keys.py` without `--write` recomputes all twelve keys from
the inputs and agrees with the tasks files.

Tooling: G8's (Go 1.25.3, scip-go v0.2.7 with SHA-256 `b1d476a7...b546f`,
`scipdump`, `sciprels`, `gofuncs`, `per_test_cover.py`), copied from G9's
build directory.

```bash
B=build/g10   # git-ignored; scip-go, scipdump, sciprels, gofuncs built as in ../g8_go/PREREG.md
git clone https://github.com/go-chi/chi.git $B/chi-src   # testify-src, compress-src, kong-src as in G9
# base-NAME/src: make_target() from run_agent_tokens_eval.py on the tag's commit, no change
# chg-chi/src: make_target() on chi's commit with chi-change.patch and its message
for n in testify compress kong chi; do
  sh eval/g8_go/keys/build_inputs.sh $B $B/base-$n/src $B/inputs-base $n
done
python eval/g10_go/keys/g10pools.py $B/inputs-base testify=$B/base-testify/src \
  compress=$B/base-compress/src kong=$B/base-kong/src chi=$B/base-chi/src > eval/g10_go/pools.json
python eval/g10_go/keys/g10draw.py eval/g10_go/pools.json > eval/g10_go/draw.json
mkdir -p $B/inputs-keys && cp $B/inputs-base/{testify,compress,kong}.* $B/inputs-keys/
sh eval/g8_go/keys/build_inputs.sh $B $B/chg-chi/src $B/inputs-keys chi
python eval/g10_go/keys/g10keys.py $B/inputs-keys testify=$B/base-testify/src \
  compress=$B/base-compress/src kong=$B/base-kong/src chi=$B/chg-chi/src
```

The callers keys were checked against a grep for `.Name(` and the
enclosing declarations: every key entry has such a call, and every
other grep hit is a call on another type (`t.Error`, `t.Run`, an
`*http.Request`'s `Header.Get`, xxhash's `crc.Reset`, and so on) or a
comment. The test reach keys are the coverage runs', as in G7 to G9, and
were spot-checked.

- **T13 (2), C13 (2), K5 (4), H1 (6), H2 (2), test reach.** T13:
  `TestPrintSortedKeys` and `TestSpew` of `internal/spew`. C13:
  `TestSeeking` and `TestIndex` of `s2`. K5: `TestEnvarAutoHelp`,
  `TestMultipleEnvarAutoHelp`, and their two `WithEnvPrefix` variants.
  H1: the six throttle tests that build the middleware through
  `ThrottleBacklog` or `ThrottleWithOpts`; `TestThrottleRetryAfterRounding`
  calls neither. H2: `TestRecoverer` and `TestRecovererNoColor`.
- **T14 (14), K6 (10), callers on a receiver.** T14: eleven methods of
  `TestExampleImplementation` in `mock/mock_test.go` (`args.Error(n)` on
  `mock.Arguments`), `Test_Arguments_Error`, `Test_Arguments_Error_Nil`,
  and `TestRoundTripper.RoundTrip` in `http/test_round_tripper.go`. K6: ten
  tests of `kong_test.go` and `options_test.go` that call `ctx.Run(...)` on
  a `*kong.Context`; the `ctx.Run` calls of `_examples` are outside the
  scope.
- **C14 (1), H3 (82), H5 (7), H6 (1), interface.** C14:
  `(*Reader).WriteTo`, whose `crcWriter.Reset()` goes through the `crcer`
  interface that only `crcUpdater` implements in the repository. H3: the 82
  functions that call `Get` on a `*Mux` or a `Router` (the interface only
  `*Mux` implements); nearly all are tests. H5: the seven test functions
  that call `Group` on a `*Mux` or a `Router`, including
  `r.With(mw).Group(...)`. H6: `RequestLogger`'s handler, which calls
  `f.NewLogEntry(r)` on a `LogFormatter`.
- **H4 (4), what the change left unedited.** The callers of `cW` less the
  two the change edited: `defaultLogEntry.Write`,
  `prettyStack.decorateFuncCallLine`, `prettyStack.parse`, and `Test_cW`.

## What dagayn answers

<!-- derived-from ../g9_go/PREREG.md#what-dagayn-answers-checked-after-the-keys-were-committed -->

G9 recorded dagayn's first answers to its questions in the registering
commit. G10 does not: no dagayn call touched a drawn target before this
file was committed, `query_graph` included. `keys/g10answers.py` makes G9's
first calls (G8's `dagayn_answers.py`, imported unchanged) and is
registered unrun. It may run after the registering commit; its output goes
in the results as report only and decides nothing.

## Arms

| Arm | Setup |
|---|---|
| candidate | dagayn at main d8ecb51f (go extractor 18, rust 28, python 17, javascript 16, read from a `_core` built at d8ecb51f with the 1.99.1 toolchain), reached through the CLI. It is the `dagayn` uv tool at `$HOME/.local/bin/dagayn`, reinstalled non-editable from a checkout of d8ecb51f on Python 3.14 with the 1.99.1 toolchain first on PATH, and not reinstalled during G10. The installed tool reports go 17, so the reinstall is required. Before the runs, `$HOME/.local/share/uv/tools/dagayn/bin/python -c "import dagayn._core as c; print(c.extractor_versions())"`, run outside the checkout, must report go 18. The results record the `_core` SHA-256 before and after the runs. Every run's `graph_before` must show go 18. Each run clones the target and runs `dagayn build --local-embedding none` (the harness default). The harness runs it with `--candidate-transport cli`: there is no MCP server, a `dagayn` on the run's PATH runs the uv tool, and the prompt carries `condition_line.cli`. The agent calls `dagayn tool <tool> --arg k=v` through Bash. Since #321 the tool prints one-line JSON. |
| baseline | No dagayn: no MCP server and no `dagayn` on PATH (the harness removes it). The prompt carries `condition_line.none`, G8's. |

No `--scip`, and `scip-go` is not on the runs' PATH, as in G7 to G9. Both
arms have Go 1.24.13 (`mise`) on PATH and neither has `gopls`, `guru`, or
`callgraph`.

### Why the CLI

<!-- derived-from ../g9_go/RESULTS.md#short-one-more-request-than-the-baseline -->

The owner chose the CLI after this file was first committed with the
transport open, and before any G10 run. In G9, 114 of the 120 dagayn runs
first spent a request on `ToolSearch` to load the MCP tool schemas. That is about 22k
tokens per run before the agent learns anything (#308). The CLI has no
schemas to load. An exploratory pilot, not registered, compared the CLI arm
with the MCP arm, not with the baseline. It used six G9 questions (T11,
C10 LONG; T12, C12, K3, K4 SHORT), 5 runs per arm, the harness of #322, and
the condition line below. Its results:

- On SHORT, the sum of the per-question median tokens was 0.74x MCP's. On
  LONG it was 0.54x.
- Both arms gave 30 exact answers out of 30.
- No CLI run called `ToolSearch`, and no CLI call failed on its arguments.

The pilot's raw runs are outside the repository, in the session's
scratchpad (`pilot-cli/`). They decide nothing, because these six
questions had been tuned on.

`condition_line.cli`, the candidate's line, is the pilot's, unchanged. It
names the call form, says the output is one line of JSON, and lists the
patterns. It says that `next` holds calls to run as `dagayn tool`, and
that a complete `ok` reply is not to be recounted with grep.
`condition_line.graph` (G8's MCP line) stays in the tasks files and is
not used.

What the transport changes in the analysis: `analyze.py` and
`fixed_cost.py` count a dagayn call as an `mcp__dagayn__*` tool call or a
Bash command matching the harness's `dagayn tool X` rule. A candidate run
fails on its MCP status only when its init event lists an MCP server, and
a CLI run lists none. The harness at d8ecb51f scores a Bash
`dagayn tool X` call as `mcp__dagayn__X`, so its `dagayn_calls` and
`dagayn_tools` columns count CLI calls. The raw transcript still names
such a call `Bash`. So `analyze.py` replaces G7's "checks after the first
dagayn call" with a version that recognizes the CLI call and does not
count it as a check.

## Procedure

- Harness: `eval/run_agent_tokens_eval.py` at d8ecb51f (SHA-256
  `ec5d7844...f9d25162`). Since G9 it has gained a setup transport
  (#322): `--candidate-transport cli` puts a `dagayn` that runs the
  candidate command on the run's PATH and gives no MCP config. It also
  reads the prompt's `condition_line.cli` and scores a Bash
  `dagayn tool X` call as the tool X. Its MCP path and its scoring of
  answers, tokens, and time are G7's to G9's.
- Model `claude-haiku-5-5`, `--effort medium`, 10 runs per question per
  arm, 3 runs in parallel per invocation, timeout 1,200 s per run.
- Four invocations: testify and compress at the same time, then kong and
  chi at the same time.
- Before them, a plumbing check: 1 run per arm of `plumbing_chi.json` (the
  12 methods of tree.go's `node`) on chi, the one new target, with its
  change commit. It checks the login, the candidate's connection to
  dagayn, the change commit, and scoring. It is not part of the results.
- The harness and `analyze.py` run under `uv run python` or a checkout's
  `.venv/bin/python`; the results say which. G9 used `.venv/bin/python`
  because `uv run` tried to rebuild the editable package with the rustc on
  PATH (1.98).

```bash
B=build/g10
COMMON="--candidate $HOME/.local/bin/dagayn --candidate-transport cli --baseline none --keep-home --reps 10 -j 3 --timeout 1200 --model claude-haiku-5-5 --effort medium"
# Plumbing check (not part of the results)
python eval/run_agent_tokens_eval.py --tasks-file eval/g10_go/plumbing_chi.json \
  --source-repo $B/chi-src --seed 20261116 --out out-g10-plumbing $COMMON --reps 1
# Runs
python eval/run_agent_tokens_eval.py --tasks-file eval/g10_go/tasks_testify.json \
  --source-repo $B/testify-src --seed 20261112 --out out-g10-testify $COMMON &
python eval/run_agent_tokens_eval.py --tasks-file eval/g10_go/tasks_compress.json \
  --source-repo $B/compress-src --seed 20261113 --out out-g10-compress $COMMON &
wait
python eval/run_agent_tokens_eval.py --tasks-file eval/g10_go/tasks_kong.json \
  --source-repo $B/kong-src --seed 20261114 --out out-g10-kong $COMMON &
python eval/run_agent_tokens_eval.py --tasks-file eval/g10_go/tasks_chi.json \
  --source-repo $B/chi-src --seed 20261115 --out out-g10-chi $COMMON &
wait
# Analysis
python eval/g10_go/analyze.py out-g10-testify out-g10-compress out-g10-kong out-g10-chi --seed 20261111
python eval/g10_go/analyze.py out-g10-testify out-g10-compress out-g10-kong out-g10-chi --seed 20261111 --include-failed
```

- Run the commands from an `sh` script: zsh does not split the unquoted
  `$COMMON` (G9's deviation).
- `--keep-home` keeps the CLI's login and passes `--setting-sources
  project,local --disable-slash-commands`. None of the targets has a
  `CLAUDE.md` or `AGENTS.md`.

## Scoring

As G7 to G9:

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
unchanged for the effect, the permutation test, Holm's correction, and the
descriptive numbers. Two parts differ from G7's. Its `load` fails a
candidate run on MCP status only when the run lists an MCP server. Its
"checks after the first dagayn call" recognizes the CLI call
([Why the CLI](#why-the-cli)). Each question's length is the `length`
field of its task.

- **Effect.** For a set of questions: the mean over the questions of the
  per-question difference in mean log tokens (log time; F1), candidate
  minus baseline. A ratio is the exponential of the log effect.
- **LONG test (primary).** Over the 8 LONG questions (T13, T14, C13, C14,
  K5, H1, H2, H3), two-sided, alpha 0.05: H1 total tokens differ, H2 time
  differs, H3 F1 differs. p from 10,000 permutations of the arm labels
  within each question (seed 20261111). Holm over the three p-values.
- **SHORT guard.** Over the 4 SHORT questions (K6, H4, H5, H6): the token
  ratio as a point estimate, and the exact matches of each arm. Its
  permutation p is reported and decides nothing.
- Secondary: the same family per repository; chi alone (its 3 LONG
  questions as a family, and its 3 SHORT questions' token ratio and exact
  matches); and per question (Holm over the twelve, per measure).
  Descriptive: exact matches, turns, dagayn calls, Bash / Read / Grep /
  Glob calls after the first dagayn call.
- **SHORT fixed cost (registered, report only, #292).** `fixed_cost.py`,
  called by `analyze.py`. A request is one distinct assistant message id
  in the raw transcript, with the tokens of its usage. For each candidate
  run of a SHORT question, the fixed cost is the tokens of every request up
  to and including the first one that makes a dagayn call, split into the
  requests that call `ToolSearch` and the request that makes the call. It
  reports the medians per SHORT question and pooled, how many runs spent
  request 1 on `ToolSearch` and request 2 on the dagayn call, and the
  SHORT token ratio with each candidate run's fixed cost taken off. On
  G9's SHORT runs it gives what G9's results reported: 36 of 40 runs with
  `ToolSearch` then the call, a pooled median of 45k tokens (22k
  `ToolSearch`, 23k the call), and 0.83x without the fixed cost. With the
  CLI there is no `ToolSearch`, so G10's fixed cost is mostly the request
  that makes the first `dagayn tool` call, with any reading done before
  it. It decides nothing.

## The bar

<!-- constrained-by ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-05-after-g9 -->

Go meets the bar when both of these hold:

- **LONG.** Pooled over the 8 LONG questions, the token ratio is at most
  0.85 with Holm p below 0.05, or the time ratio is at most 0.85 with Holm
  p below 0.05 (both from the LONG Holm family), and the dagayn arm has no
  fewer than the baseline's exact matches on LONG minus 2 (3 or more fewer
  fails, G8's accuracy rule). Unchanged from G9.
- **SHORT.** Pooled over the 4 SHORT questions, the token ratio point
  estimate is at most **1.25** (G9: 1.10; the owner's change of
  2026-11-05, recorded in #320 before this draw), and the dagayn arm's
  exact matches on SHORT are not fewer than the baseline's.

`analyze.py` reports `bar.long.met`, `bar.short.met`, and `bar.met`. Go is
first-class when this bar and the M2 gate both hold. If the bar is not
met, the defects the runs show become the rest of the Go work. The
criteria do not change after the runs. chi alone decides nothing; the
results report it.

## Exclusions and exceptions

- A run is failed when it has no result event, ends with an error, or
  times out, or (candidate) its init event lists an MCP server and
  `dagayn:connected` is not among them. A CLI run lists none, so for
  G10 only the first three apply. A candidate run that never calls
  `dagayn tool` is not failed; the results count such runs. Failed runs
  are recorded, not rerun, and left out of the primary analysis. The sensitivity analysis
  (`--include-failed`) counts them with F1 0 and the largest tokens and
  time of their question and arm.
- A candidate run whose graph metadata changed during the run
  (`graph_before` and `graph_after` in its meta file), or whose graph is
  not at go 18, is reported.
- If a key proves wrong, it stays registered; the results report the error
  and score both ways.

## Budget

The CLI uses its login, so `total_cost_usd` is the CLI's estimate, not a
bill. In G9's 240 runs, a run cost a mean of 0.0125 USD on the reach
stratum, 0.0104 on the name stratum, and 0.0062 on the short stratum (the
committed `results.csv` files; 2.32 USD in all). G10 has 100 reach runs, 60
name runs, and 80 short runs, so its 240 runs are expected near 2.4 USD
(100 x 0.0125 + 60 x 0.0104 + 80 x 0.0062), and near 4.9 USD if every run
costs as much as G9's most expensive question (C9, 0.0204). H3's key has 82
entries, so its runs may cost more than G9's name questions. The plumbing
check is 2 runs. The cap is 25 USD. The harness has no cost limit, so the
operator reads the running total from the finished runs, and stops the
batches and reports if it projects past the cap.

## Threats to validity

- **The bar was changed again after a miss.** The SHORT guard moved from
  1.10 to 1.25 after G9 missed 1.10, by the owner's decision, before this
  draw. The LONG bar did not change. Under 1.25, G9's SHORT result (1.17x)
  would have passed.
- **Three of four repositories are tuned on.** testify, compress, and kong
  have two questions each. Their results show whether the fixes hold on
  new questions of the code they were tuned on, not whether they
  generalize. chi is the test of that, with 6 of 12 questions.
- **chi's questions are not a random sample of chi.** Its name question is
  forced (one candidate). Its three SHORT questions are 3 of the 4, so the
  SHORT guard is mostly a chi result. H3's key has 82 entries, which may
  make exact matches rare in both arms.
- **chi is smaller than kong** (189 production functions against 298) and
  was chosen by checks on tooling and pool sizes, in a stated order, from
  a short list. Other untuned repositories could give other results.
- **The change commit was written by hand after the draw**, as in G8. It
  edits `NewLogEntry`, which is also H6's target; H6 asks for its callers,
  which the change does not touch.
- **The length rule rests on one gap** (127 to 270) in twelve G7 and G8
  questions (G9's threat, unchanged). A LONG question may turn out short
  for the baseline here; the test does not reclassify questions after the
  runs, and the results report each question's baseline turns.
- **G10 measures dagayn through the CLI, not MCP.** A pass or a miss
  says nothing about the MCP transport, which G7 to G9 measured.
- **The instructions sit in different places.** The CLI arm's
  instructions (call form, patterns, what `next` means, not to recount a
  complete answer) are in the user prompt (`condition_line.cli`).
  Under MCP, the equivalent text is in the server's instructions and the
  tool descriptions. In a real install, the CLI's instructions would come
  from `AGENTS.md` or `CLAUDE.md`, not from the question. So the prompt
  that G10's candidate arm sees is longer and more explicit than G9's.
- **The transport was chosen on tuned questions.** The pilot behind the
  choice (CLI 0.74x MCP's tokens on SHORT, 0.54x on LONG) used six G9
  questions that had been tuned on, with 5 runs per arm. It was not
  registered, and it compared two dagayn transports, not dagayn with no
  dagayn.
- Keys come from scip-go and coverage. The interface entries (C14, H3, H5,
  H6) count a call that can run the target at run time; every such entry
  was checked against the source.
- Test reach is per function and per top-level test: a test counts when
  any block of the function ran, as in M2 and G7 to G9.
- Six runs and the candidate's graph builds share the machine, so time is
  compared only between the arms of one invocation.
- A concurrent session could reinstall the dagayn uv tool during the runs.
  The meta files' `graph_before` record the extractor versions each run
  built with, and the `_core` hash is checked before and after.
- Agents can break the rule not to run the tests; transcripts are checked
  and any such run is reported.

## Changes after registration

(none)
