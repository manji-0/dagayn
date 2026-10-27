# Results: M3, Python questions a short script cannot answer

<!-- derived-from ./PREREG.md -->

The questions, keys, scoring, arms, and analysis were registered in commit
d2746bc30e56243ad48474555c1ff2e215f7477e, pushed before any run.
`PREREG.sha256` still matches every registered file except
`run_agent_tokens_eval.py` (three fixes, below).

## Verdict

**The bar is not met.** Pooled over the eight questions, agents with
dagayn used 1.05x the tokens of agents without it (Holm p = 0.56) and
0.93x the time (Holm p = 0.30), at equal accuracy: both arms got all 80
answers exact. The bar asked for 0.85x at Holm p < 0.05. Per the plan,
Python stays measured and maintained, with no further investment.

## What ran

- 2026-10-27 (JST). 160 runs: 8 questions, 2 arms, 10 runs each, Claude
  Haiku 5.5, `--effort medium`. No run failed, timed out, or lost its MCP
  server, so the sensitivity analysis (`--include-failed`) equals the
  primary one.
- The CLI's estimate of cost: 1.51 USD (0.76 per arm), under the 4 to 10
  USD estimate and the 25 USD cap.
- No candidate run's graph metadata changed during the run.
- No run executed the test suite or the repository's code. Six X1 runs
  without dagayn ran `python3 -c "import httpcore ..."` to find the
  installed httpcore source and grep it.
- Raw transcripts:
  `/private/tmp/claude-501/-Users-manji0-src-dagayn/91ef8d31-82cc-4ad1-9f44-3d027ad27229/scratchpad/m3run/out-httpx/raw`
  and `.../m3run/out-dagayn/raw` (not committed). `results/` holds each
  batch's `results.csv` and `summary.json`, and `analysis.json`.

### After registration (not pre-registered)

1. **First launch stopped, 17 runs discarded unread.** Concurrent
   `git clone`s of the dagayn target failed, and the graph build had
   inherited `uv run`'s `VIRTUAL_ENV`, so the Python indexer read the
   harness's environment instead of the target's (PREREG says the targets
   have none). Both batches were stopped, and the 17 finished runs set
   aside without being scored or read. Fixed in be6dab1f, and both batches
   restarted into new output directories. The commit message says 34 runs;
   the count was 17.
2. **The dagayn batch stopped again on its first three clones.** The
   archive commit leaves thousands of loose objects and starts
   `git gc --auto` in the background, which deleted them under the first
   clones. The target is now packed before any clone (fa7c4b08). The batch
   was resumed into the same directory; the harness skips finished runs, so
   it ran the 159 runs that had not finished.

Neither fix touches a question, a key, the scoring, or the analysis.

## Pooled (registered test)

| Measure | dagayn / none | p | Holm p |
|---|---|---|---|
| Total tokens | 1.05x | 0.28 | 0.56 |
| Time | 0.93x | 0.10 | 0.30 |
| F1 | +0.000 | 1.0 | 1.0 |

| Bar check | Result |
|---|---|
| tokens at most 0.85x | no (1.05x) |
| tokens Holm p < 0.05 | no (0.56) |
| exact not 3 or more fewer | yes (80 / 80 against 80 / 80) |

Per repository: httpx tokens 1.02x (Holm p = 1.0), time 0.85x (Holm p =
0.057); dagayn tokens 1.07x (Holm p = 0.75), time 1.02x (Holm p = 1.0).

## Per question

Medians of 10 runs. Holm over the eight questions, per measure. "Checks"
are Bash, Read, Grep, and Glob calls after the first dagayn call.

| Q | Exact (dagayn / none) | Tokens (dagayn / none) | Ratio, Holm p | Time ratio, Holm p | dagayn calls | Checks | dagayn's first answer |
|---|---|---|---|---|---|---|---|
| X1 test reach | 10 / 10 | 230k / 164k | **1.33x, 0.012** | 0.87x, 1.0 | 4.5 | 5.5 | 0 / 9 |
| X2 receiver | 10 / 10 | 142k / 178k | 0.86x, 0.30 | **0.71x, 0.007** | 1 | 5.5 | 10 / 10 |
| X3 table | 10 / 10 | 219k / 160k | **1.38x, 0.022** | 1.19x, 0.18 | 1 | 7 | 0 / 4 |
| X4 unedited | 10 / 10 | 105k / 164k | **0.68x, 0.008** | **0.70x, 0.012** | 1 | 2 | 7 rows = 5 + 2 edited |
| D1 test reach | 10 / 10 | 648k / 562k | 1.07x, 1.0 | 1.07x, 0.95 | 4 | 15 | 5 / 9 and 33 wrong |
| D2 receiver | 10 / 10 | 240k / 211k | 1.07x, 1.0 | 0.83x, 0.64 | 1 | 7.5 | 7 / 7 and 40 rows in `tests/` |
| D3 table | 10 / 10 | 154k / 131k | 1.26x, 0.20 | 1.14x, 0.83 | 1 | 5 | 1 / 2 |
| D4 unedited | 10 / 10 | 131k / 144k | 0.92x, 1.0 | 1.05x, 1.0 | 1.5 | 1 | 10 rows = 8 + 2 edited |

## Why every answer was exact

All 160 answers equal their keys. That was checked run by run outside the
harness, by re-reading each final JSON block and comparing it with the key;
the answer sizes are the key sizes in every run (9, 10, 4, 5, 9, 7, 2, 8).
The questions are ones a short script cannot answer, as the plan asked, but
Haiku did not need a script: it read the 3 to 10 files that decide each
answer (the table, the fixture, the trace callback) and got them right.
The plan's premise holds, that agents without dagayn have no shortcut, but
the cost of reading those files is small. So the experiment measures cost
only, at a ceiling of accuracy.

## Where dagayn lost

The losses follow dagayn's first answer, as the registration table
predicted. Where it was empty or incomplete, the agent paid for the
ToolSearch and the dagayn calls and then did the baseline's work anyway.
Where it was right, the agent still checked it.

- **X3 (1.38x): an empty answer the agent did not trust, rightly.**
  `callers_of` returned 0 rows with `results_complete: true` and no `next`.
  The agent's next words, in run 007: "The graph returned no callers, with
  low confidence, so I'll check the source directly. The graph can't see
  dynamic dispatch such as `decoder.decode(...)` on a value taken from a
  dict." Then it read `_decoders.py` and `_models.py` as the baseline did
  (10 turns, 223k tokens).
- **X1 (1.33x): four calls that walked to a dead end.** In run 004,
  `tests_for` gave 0 rows; `callers_of(depth=6)` found `print_response_headers`
  and `trace`; `callers_of trace` gave 0 rows marked complete, because
  `trace` is passed as a value (`functools.partial(trace, ...)` in an
  `extensions` dict) and `main` is called by click through `CliRunner`. Then
  four Bash calls of grep and sed over `_main.py`, `test_main.py`, and
  `conftest.py`, as without dagayn.
- **D3 (1.26x): the closed-answer sentence sent the agent to grep.**
  `callers_of _execute_embed` found only the test helper. Run 002: "The
  graph reports one caller, but it says calls made through dict tables,
  strings, or registration are not edges. I'll grep for those forms." Two
  Greps and a Read found `_TASK_EXECUTORS` and `run_worker`.
- **D1 (1.07x, the most expensive question in both arms): a long cut
  answer, then the whole reading.** Run 003 called `callers_of
  run_scip_overlay(depth=6)` (17,738 characters, 46 of 92 rows) and
  `tests_for _timeout` (0 rows), then: "The graph shows many callers of
  `run_scip_overlay`, but reachability isn't the same as execution. I'll
  read the source." It read `scip_overlay.py`, three test files, and two
  fixtures. Nine of ten D1 runs got a reply over 8,000 characters.
- **D2 and X2: right answers, checked anyway.** dagayn's answer was
  complete on both, and agents still made 5.5 (X2) and 7.5 (D2) checks.
  On D2 the reply was 14,659 characters because 40 of its 47 rows are in
  `tests/`, which the question excludes; no run passed `path_prefix`. X2
  was faster with dagayn (0.71x the time) but not cheaper.
- **X4 is the one clear win (0.68x tokens, 0.70x time).** One `git show`
  and one `callers_of` gave the 7 callers; the agent removed the two the
  diff edits and checked the other `.close()` calls once (2 checks). D4 is
  the same shape but its baseline was already short (0.92x, not
  significant): `WorkerLock.acquire` has 10 call sites and one grep finds
  them.

## What dagayn would need

These are the gaps the transcripts show. M3's outcome is that the plan
does not fund them for Python.

1. **Name the hole instead of declaring the answer closed.** On X1, X3,
   and D3 the reply said `results_complete: true` and "No caller the graph
   records is missing; calls made through strings, getattr, or
   registration are not edges". The agents read that sentence as a reason
   to grep. A reply that names where the graph saw the function used as a
   value (`_TASK_EXECUTORS` in `queue_worker.py`, `functools.partial(trace,
   ...)` in `main`, `SUPPORTED_DECODERS` in `_decoders.py`) would replace
   the grep with one read. This is a reference row, not a new inference
   rule, so it fits the plan's freeze.
2. **Test reach that says what it knows.** On D1, `tests_for` returned 0
   rows while the same reply's `test_reach.counts_as_tested` was true with a
   nearest test, and `callers_of(depth=6)` listed 38 tests of which 33 do
   not run the body. Only coverage answers "which tests execute this".
   Static reach that cannot say so made agents read everything.
3. **Scope rows to the question.** D2's 40 out-of-scope rows doubled the
   reply. The question named `dagayn/`; agents did not pass `path_prefix`
   (the E15 finding again).
4. **A first call that pays for itself.** 78 of 80 candidate runs spent a
   turn on ToolSearch and one or more on dagayn before reading. Where dagayn's
   answer was empty (X1, X3) or partial (D3), that was a pure cost of 20%
   to 40%.

## What can and cannot be said

- Can: on these eight Python questions, an agent with dagayn was no
  cheaper and no faster than one without, and both were always right.
- Can: dagayn's answer saved tokens where it was complete and the
  question needed the diff (X4), and cost tokens where it was empty or
  partial (X1, X3, D3).
- Cannot: that Python questions in general gain nothing. The eight were
  chosen by kind, with dagayn's answers seen at registration.
