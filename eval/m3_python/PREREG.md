# Pre-registration: M3, Python questions a short script cannot answer

<!-- derived-from ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#why-python-cost-agents-tokens -->
<!-- constrained-by ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#measure-first -->

This file was written and committed before any agent run on these
questions. `PREREG.sha256` holds the SHA-256 of every registered file. The
results name the commit that registered them.

## Purpose

E1, E10, and E11 found that dagayn cost agents tokens on Python (1.16x to
1.40x): the questions were ones a short `ast` script answers, and agents
re-checked the graph's rows. M3 asks the four kinds of question the plan
names, which a script cannot answer from syntax alone:

| Kind | dagayn (8827f986) | httpx 0.28.1 (b5addb6) |
|---|---|---|
| Which tests reach a function | D1 `scip_overlay._timeout` | X1 `_main.format_response_headers` |
| Callers of a method on a receiver whose type no import names | D2 `EmbeddingStore.close` | X2 `Headers.get` |
| Callers through a table or a decorator registry | D3 `queue_worker._execute_embed` | X3 `GZipDecoder.decode` |
| What a change left unedited | D4 `WorkerLock.acquire` | X4 `Response.close` |

The prompts are in `tasks_dagayn.json` and `tasks_httpx.json`. Each target
is `git archive` of the commit plus one change commit (`dagayn-change.patch`,
`httpx-change.patch`), so `HEAD~1..HEAD` is the change D4 and X4 ask about.
Both prompts forbid running the test suite or the repository's code.

## Arms

| Arm | Setup |
|---|---|
| candidate | The installed `dagayn` uv tool (`/Users/manji0/.local/bin/dagayn`, 10.0.0). Its receipt installs from `/Users/manji0/src/dagayn`, which was at 8827f986 with no tracked changes; its Python files equal `git archive 8827f986 dagayn`, and `_core.cpython-314-darwin.so` was built at 2026-10-27 21:12 JST, 8 minutes after that commit. Each run clones the target and runs `dagayn build --scip` (no embeddings). The MCP server is `dagayn serve`, what `dagayn install --mode fts-only` writes. |
| baseline | No MCP server. |

`dagayn install` also writes hooks and, in its embedding modes,
`serve --local-embedding`. The runs use neither: the graph has no
embeddings, and a sidecar or a hook could rewrite the measured graph during
the run. E1 to E15 measured the same way.

## Procedure

- Harness: `eval/run_agent_tokens_eval.py` at the registering commit.
- Model `claude-haiku-5-5`, `--effort medium`, 10 runs per question per arm:
  8 questions x 2 arms x 10 = 160 runs.
- Two invocations run at the same time, 3 runs in parallel each, timeout
  1,200 s per run:
  - httpx: `--tasks-file eval/m3_python/tasks_httpx.json --source-repo <git
    copy of httpx at b5addb6> --seed 20261028`
  - dagayn: `--tasks-file eval/m3_python/tasks_dagayn.json --seed 20261029`
  - both: `--candidate /Users/manji0/.local/bin/dagayn --baseline none
    --build-args=--scip --keep-home --reps 10 -j 3 --timeout 1200`
- `--keep-home` keeps the CLI's login and passes `--setting-sources
  project,local --disable-slash-commands`. A probe on the dagayn target
  showed only the project `CLAUDE.md` in context: no user `CLAUDE.md`, user
  hooks, plugins other than the CLI's built-ins, skills, or user MCP servers.
- Before the runs, one plumbing check ran on a question outside M3 (the
  methods of `MultiDecoder`, 1 run per arm) to check the login, the MCP
  connection, the change commit, and scoring. It is not part of the results.

## Answer keys

Each key was built without dagayn and every entry was checked by reading
the source. `keys/check_keys.py` recomputes the machine-derived part from
the coverage and trace files (kept outside the repository) and agrees with
every entry; the entries no test run exercises were classified by hand.

- **D1, X1 (test reach).** Coverage with test contexts:
  `COVERAGE_CORE=ctrace pytest --cov=<pkg> --cov-context=test` on the
  archived commit (dagayn with pytest-xdist, httpx under Python 3.12).
  `keys/cov_reach.py` lists the tests whose setup, call, or teardown
  executed a line of the function's body. Checked by hand:
  - D1: `_timeout` runs only inside `run_scip_overlay`'s loop over planned
    jobs. Five tests in `test_scip_overlay.py` call it with an indexer
    override, and two each in `test_scip_python.py` and
    `test_scip_typescript.py` call it through `_overlay` on a fixture with a
    bundled indexer. `test_build_tool.py::TestScip` builds with `scip=True`
    but plans no job (only `pom.xml`, which needs consent), and
    `test_cli_build.py` replaces the build. 9 entries.
  - X1: `trace` (passed to httpcore as the `trace` extension in `main`)
    calls `print_response_headers` on every response, so every
    `test_main.py` test that sends a request reaches it: 9 tests;
    `test_help` and `test_errors` send none.
- **X2, X3, X4, D2, D3, D4 (callers).** A runtime trace of every call among
  the repository's functions during the whole test suite
  (`keys/calltrace.py`, a `sys.setprofile` pytest plugin), then
  `keys/sweep.py` lists every `.name(...)` call and attribute read in scope,
  and each receiver was classified by reading the enclosing function:
  - X2 (10): `request.headers` in four test apps, `response.headers` in
    `download_response` and `get_lexer_for_response`, `self.headers` in
    `Request.__init__`, `Response.charset_encoding`, `Response.links`, and
    `h` in `test_headers`. Not counted: dicts (`link`, `header_dict`,
    `kwargs`, `scope`, `message`, ...), `Client.get`, `Cookies.get`,
    `QueryParams.get`.
  - X3 (4): `Response.iter_bytes` and `aiter_bytes` call
    `decoder.decode` on `_get_content_decoder()`, which builds a decoder
    from `SUPPORTED_DECODERS`; `MultiDecoder.decode` and `flush` call
    `child.decode` on the decoders it holds. Not counted: `TextDecoder`,
    `LineDecoder`, the chunkers, `bytes.decode`, `DeflateDecoder`'s
    recursive `self.decode`.
  - X4 (5): `Response.close` is called in `Client.stream`, `Client.send`,
    `Client._send_handling_auth`, `Client._send_handling_redirects`,
    `Response.iter_raw`, and two tests. The change edits `Client.send`,
    `Client.stream`, and `Response.close` (`keys/edited_functions.py`), so
    5 are left.
  - D2 (7): `EmbeddingStore.__exit__`, `embedding_text_modes.run`,
    `search._get_cached_emb_store`, `build._prune_orphaned_embeddings`, and
    `scan_embed_work`, `write_embed_work`, `embed_graph` in `tools/docs.py`.
    The trace saw 3; the other 4 bind the receiver to `EmbeddingStore(...)`
    or to the store cache in the same function. Every other `.close()` in
    `dagayn/` is on a `GraphStore`, a connection, a file, or a queue.
  - D3 (2): `run_worker` calls `_TASK_EXECUTORS.get(task["kind"])` and the
    table holds `_execute_embed` (by hand: every test that runs the worker
    replaces the table); `TestEmbedTaskStructure._run` calls it directly.
  - D4 (8): `WorkerLock.acquire` is called in `__enter__`,
    `ensure_worker`, `run_worker`, and seven tests. The change edits
    `acquire`, `run_worker`, and `test_second_holder_cannot_acquire`, so 8
    are left. Not counted: `thread_lock.acquire` in `write_lock.py`.

The caller sets are the same on the base and the change commit: the
patches add no call to a target and remove none.

## What dagayn answers (checked before registration)

`keys/dagayn_answers.py`, the first calls an agent would make, at the
default detail level, on graphs built as the harness builds them
(`dagayn_answers.json`):

| Q | dagayn's first answer against the key |
|---|---|
| X1 | `tests_for` 0 rows; `callers_of(depth=6)` finds no test: 0 / 9 |
| X2 | `callers_of` 10 / 10, nothing extra |
| X3 | `callers_of` 0 / 4 (the table and the list are not edges) |
| X4 | `callers_of` 7 rows = the 5 + the 2 edited callers; `review_tool` names no unchanged caller |
| D1 | `tests_for` 0 rows while `test_reach.counts_as_tested` is true; `callers_of(depth=6)` 5 / 9 tests and 33 tests that do not reach it |
| D2 | `callers_of` 7 / 7, 40 rows in `tests/` (out of scope) and one wrong row in scope (`get_docs_section`, a `GraphStore`) |
| D3 | `callers_of` 1 / 2: misses `run_worker` (the table) |
| D4 | `callers_of` 10 rows = the 8 + the 2 edited callers; `review_tool` names no unchanged caller |

Expected before any run: dagayn's answers are complete only on X2, D2, and
(after removing the edited callers) X4 and D4; they are wrong or empty on
X1, X3, D1, and D3, where the agent has to find the rest itself.

## Scoring

- The answer is the list in the last ` ```json ` block of the final reply;
  no block scores F1 0 and no exact match. Runs are not excluded for it.
- Each entry is normalized (`symbols` in the harness): backticks, a
  leading `./`, a parametrize suffix `[...]`, and a trailing `()` are
  dropped, and `path::Class::method` is read as `path::Class.method`.
- Exact match: the normalized set equals the key. F1 against the key.
- Tokens: input, output, cache read, and cache write, summed over models.
  Time: the CLI's `duration_ms`.

## Hypotheses and test

Two-sided, alpha 0.05, as E8 to E13 (`analyze.py`):

- H1 total tokens differ, H2 time differs, H3 F1 differs.
- Effect: the mean over questions of the per-question difference in mean
  log tokens (log time; F1), candidate minus baseline. p from 10,000
  permutations of the arm labels within each question (seed 20261027).
  Holm over the three p-values.
- Secondary: the same per repository (Holm over three) and per question
  (Holm over eight, per measure). Descriptive: exact matches, turns, dagayn
  calls, Bash / Read / Grep / Glob calls after the first dagayn call.

## The bar

From the plan, as E12 and E13 cleared it: the bar is met when, pooled over
the eight questions,

- the token ratio is at most 0.85,
- its Holm p is below 0.05, and
- the dagayn arm has no fewer than the baseline's exact matches minus 2
  (3 or more fewer fails).

If the bar is not met, Python stays measured and maintained, with no
further investment from the plan. The criteria do not change after the
runs.

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
CLI's estimate, not a bill. At E13's 2.05 USD per 100 Rust runs and the
plumbing check (0.005 to 0.01 USD per short run), 160 runs should cost 4 to
10 USD. The cap is 25 USD: the runner checks the running total, and stops
and reports if the projected total passes it.

## Threats to validity

- Both arms load the dagayn target's project `CLAUDE.md` (it names dagayn's
  workflow) and its SessionStart hook (which exits at once outside cloud
  sessions); the CLI's built-in AGENTS.md plugin may load `AGENTS.md`.
  E10 and E11 ran on the same repository.
- The targets have no virtual environment, so the Python SCIP index leaves
  calls into installed packages to dagayn's own resolution. No key entry is
  such a call.
- Six runs and the candidate's graph builds share the machine, so time is
  compared only between the arms of one invocation.
- The questions were chosen by kind, and dagayn's answers to them were seen
  before registration (the table above). They are not a sample of Python
  questions; per-question results say where the effect comes from.
- Agents can break the rule not to run the tests; transcripts are checked
  and any such run is reported.

## Changes after registration

(none)
