# Pure Rust migration (Phase 5)

<!-- derived-from ../RUST-CORE-MIGRATION-WIP.md#phase-5-optional-outer-surface-migration -->
<!-- constrained-by ../RUST-CORE-MIGRATION-WIP.md#compatibility-contract -->

## Goal

Replace the remaining Python layer (CLI, MCP server, tool bodies, daemon,
install flows, embeddings) with Rust, so dagayn ships as one static binary.
This is the Phase 5 decision that the core migration spec left open.

The core is already Rust: parsing, graph storage, FTS, flows, communities,
and post-processing live in `crates/` (about 92k lines). Python still holds
about 55k lines, 45k of them in scope here, plus 53k lines of pytest.

## Measured baseline

Measured on this repository (774 files, 14.9k nodes) at 7.1.1, macOS arm64.

| Path | Time | Peak RSS | Where the time goes |
|---|---|---|---|
| `dagayn --version` | 0.16 s | — | interpreter and import |
| `dagayn status` | 0.22 s | 124 MB | interpreter and import |
| `import dagayn.server.main` | 0.49 s | 128 MB | fastmcp, mcp, pydantic: 0.28 s |
| hook `dagayn update --skip-flows`, no source change | 1.71 s | 389 MB | ~1.5 s in Rust: FTS rebuild 0.53 s, bare-name resolution 0.43 s, centrality 0.21 s |
| `get_minimal_context` (MCP path) | 0.15 s | — | the same git subprocesses, issued up to three times |

## Expected effects

What a pure Rust build buys:

- **Startup**: CLI and hook invocations drop from 0.16–0.22 s to a few ms,
  and from ~120 MB to tens of MB. Hooks run on every Write, Edit, and Bash.
- **MCP server**: no 0.5 s / 128 MB interpreter start per agent session.
- **Git status**: `gix` replaces `git` subprocesses on the freshness path.
- **Distribution**: 4 static binaries instead of 3 Python ABIs × 4 targets.
  Windows, deferred by the core spec, becomes a build-matrix entry.
- **Supply chain**: fastmcp, pydantic, networkx, watchdog, and numpy leave
  the dependency set.
- **One toolchain**: no PyO3 boundary, no ruff/pyrefly beside clippy.

What it does not buy: the hook update cost was not Python. It came from a
post-processing loop over an unchanged graph, fixed in Phase 5.0.

Costs: about 45k lines to port, an MCP SDK change, and a test strategy that
cannot be a line-for-line port of 53k lines of pytest.

## Reopened frozen decisions

Phase 5 reverses two decisions frozen in the core spec:

- **Binding**: PyO3/maturin (`dagayn._core`) is removed with the Python layer.
- **Distribution**: wheels carry a binary instead of an extension module.

## Phases

### 5.0 Fix the no-op cost first (done)

Done before any porting; none of it depends on the language.

- Manifest bridge discovery is limited to the VCS scope. It stored nodes for
  gitignored manifests and the gitignored files tracked manifests point at,
  the next update pruned them, and that update's
  post-processing stored them again, so every hook run rebuilt FTS,
  centrality, and bare-name edges. Steady-state hook update: 1.71 s to 0.34 s.
- `get_minimal_context` assesses sync once and derives freshness from it
  instead of running the same `git` commands up to three times: 148 ms to
  80 ms on the MCP path.
- The unused `igraph` dependency is dropped.

After 5.0 the remaining hook cost is about 0.34 s, mostly interpreter start
and `git` subprocesses, which is what 5.2 removes. An update that does change
files still costs about 1.3 s, and the profile above puts most of
post-processing in Rust already; making FTS, centrality, and bare-name
resolution incremental is independent of this plan. Startup, distribution, and maintenance have to justify 5.1 onward.

### 5.1 MCP response snapshots as the parity oracle (done)

`dagayn/contracts/state_types.py` is mostly open dicts (`extra="allow"`), so a
serde port of it would freeze nothing. The contract is what crosses the MCP
boundary, so the oracle sits there:

- `tools/mcp_snapshot.py` copies each parity fixture into a git repository
  with a fixed commit identity and date, builds it with the CLI, starts
  `dagayn serve` over stdio, and calls the read-only cases of each
  fixture. It never imports the server; `DAGAYN_CLI_CMD` and
  `DAGAYN_MCP_SERVER_CMD` point it at another implementation.
- Payloads are snapshotted, not the MCP envelope. Paths, timestamps, and
  session ids are normalized; an unknown absolute path or timestamp fails
  instead of being frozen.
- `tools_list.json` freezes tool names, parameter names and primitive types,
  required parameters, and prompts, for the default and the full tool sets.
  Full JSON Schemas differ between pydantic and schemars and are not compared.
- `graph.json` (every node and edge, via `tools/parity_export.py`) and
  `metadata.json` freeze the database the build wrote, which is the contract
  between whatever builds and whatever serves. Two manifest fixtures from
  `tests/fixtures/cross_artifact_manifest/` cover manifest bridges, which no
  read tool surfaces.
- `tests/test_mcp_snapshots.py` runs the cases in the normal suite.

Building the oracle found five sources of run-to-run drift, now fixed:
community ids and names (HashMap order and count ties), flow ids and list
order (entry point order and criticality ties), ADP cycle rotation, and the
edge order in `review_tool(mode="changes")`.

serde response types come with the Rust implementation in 5.2 and 5.3. The
existing pytest suite stays as it is until the Python it tests is deleted.

Not covered yet: sync states other than `commit_synced` (a second commit or a
dirty worktree in the fixture), embeddings, and mutating tools.

### 5.2 Rust CLI

- `clap` binary named `dagayn`, with the current subcommands and flags, so
  `hooks.json`, `.mcp.json`, and installed skills keep working.
- Start with commands that need no MCP: `status`, `update`, `build`.

First slice (done): `crates/dagayn-cli` builds a `dagayn` binary whose only
command is `build`, on `crates/dagayn-build` (file discovery, parse-and-store
moved out of the PyO3 crate, graph metadata, the write lock shared with
Python, post-processing). Manifest bridge extraction moved to
`dagayn-postproc`, and the Python build calls it through `_core`. CI runs
`tests/test_mcp_snapshots.py` with `DAGAYN_CLI_CMD` set to the binary: the
graph, the metadata, and every tool response match the Python build on all
nine fixtures.

Second slice (done): `update`, with `--base`, `--skip-flows`,
`--skip-postprocess`, `--budget-seconds`, the `hook-skip` marker, and the
non-blocking lock a hook run takes. `tests/test_update_parity.py` (run in CI
with `DAGAYN_RUST_CLI`) updates every fixture through three rounds of edits,
deletions, and renames with both CLIs, at full and `--skip-flows`
post-processing, and requires the same graph, metadata, and summary after
each. On this repository a warm hook update with nothing to do takes 0.09 s
and 90 MB instead of 0.32 s and 190 MB; after one Markdown edit it took
1.05 s instead of 1.27 s, nearly all of it whole-graph post-processing in
Rust (bare-name resolution, centrality), which only incremental
post-processing removes.

Deliberate differences from the Python CLI:

- Paths that Python walks in hash order (changed files, dependents) are
  sorted, so the same change stores rows in the same order every time.
- A manual `update` that times out waiting for the lock fails with exit
  status 1. Python prints `Incremental: 0 files updated` and exits 0, which
  reads as success.
- `tests/test_update_parity.py` starts both runs from a Python build, to
  isolate `update`; a Rust build is covered by the snapshot step.

Third slice (done): `status`, with the embedding coverage, the stored VCS
facts, and the sync assessment (commit, extractor, and diff tiers). The
parity test compares its output with Python's on every fixture in each sync
state, with and without embeddings. A warm `status` here takes 0.07 s and
74 MB instead of 0.28 s and 141 MB. `build` and `update` also refuse a graph
that records another existing repository, as Python's `_get_store` does.

The binary now covers every command the generated hooks call (`status`,
`update`).

Fourth slice (done): the installed `dagayn` runs these commands in Rust.
`crates/dagayn-cli` is a library as well as the binary, `_core.run_cli`
exposes it, and the `dagayn` console script (`dagayn/_cli_launcher.py`)
imports only `_core` before handing the command line to it. `run` answers
`Fallback` for anything it does not handle the way Python would, decided
before it takes the lock, deletes or opens the database, or prints, and the
launcher then runs the Python CLI in the same process. A second binary in
the wheel was the alternative; it was rejected because the release binary
is 102 MB (it links every grammar, as `_core` does) and starting Python and
loading `_core` costs only 0.01 s and 20 MB. Measured on this repository
(warm, nothing to update):

| `DAGAYN_HOOK_UPDATE=1 dagayn update --skip-flows` | Time | Memory |
|---|---|---|
| Python CLI | 0.33 s | 190 MB |
| `dagayn` console script (Rust in `_core`) | 0.11 s | 108 MB |
| standalone binary | 0.09 s | 90 MB |

`status` through the console script: 0.08 s and 92 MB.

What falls back to Python: every other subcommand, `--help`, `--version`,
any argument clap rejects (including argparse's prefix abbreviations), and
these, which are not ported yet: `--scip`, embeddings, jj and SVN working
copies, `CRG_DATA_DIR`, a legacy `.dagayn.db`, seeding a linked worktree's
first graph, a root Python would reject (so the user gets Python's
message), and a corrupt database (Python quarantines it). Without `--repo`,
the Python commands resolve the root differently from each other (`update`
ignores `CRG_REPO_ROOT`, `build` and `status` honour it and weigh editor
workspace variables), stop at a nested jj workspace, accept SVN or the
working directory, and refuse the home directory and the filesystem root;
Rust handles only the case they agree on (no `CRG_REPO_ROOT`, no
`CLAUDE_PROJECT_DIR`, `CURSOR_PROJECT_DIR`, or `WORKSPACE_FOLDER_PATHS`, and
a git checkout above the working directory that is not a wide root) and
falls back otherwise.
The standalone binary prints the reason and exits 1 instead.
`DAGAYN_PYTHON_CLI=1` runs everything in Python; `python -m dagayn`, which
the parity tests use as the Python side, always does. Prebuilt Wheels
installs each wheel into a fresh venv and compares the two `status` outputs.

Who this reaches: hooks that call `dagayn update` or `dagayn status`
directly (the plugin's `hooks/hooks.json`, commit-time checks, the git
`post-commit` hook, Cursor and opencode). The Claude Code and Codex edit
hooks that `dagayn install` writes run `dagayn queue add update` instead;
see the fifth slice.

Fifth slice (done): `dagayn queue add` of `update`, `postprocess`, or
`prepare` with `--repo` no longer loads the Python CLI. It was not ported to
Rust: the launcher calls the same `TaskQueue.enqueue` and `ensure_worker`
the `queue` command does, because importing `dagayn.task_queue` and
`dagayn.paths` takes 0.02 s and 27 MB, and the 0.17 s and 71 MB per edit
was argparse and the command modules. Each edit hook now costs 0.02 s and
28 MB here. Any other spelling (no `--repo`, `embed`, prefixes, `--help`)
and `DAGAYN_PYTHON_CLI=1` still go through the full CLI.

The queue worker still runs updates in Python. Measured inside one warm
process on this repository, an update with nothing to do takes 0.10-0.14 s
in Python and 0.07 s through `_core.run_cli`, and an update after one
Markdown edit about 1.05 s in both, almost all of it whole-graph
post-processing. The worker pays Python's startup once per burst, so
switching it gains little until post-processing is incremental. When it is
switched, the Rust budget watchdog must not run in the worker (it exits the
process unconditionally when the budget elapses, with no cancel), and the
result must keep `skipped`, `skip_reason`, `changed_files`, and
`dependent_files`.

Incremental post-processing (measured, not built). After one edit on this
repository a `--skip-flows` update takes about 1.1 s, of which
post-processing is about 0.96 s:

| Step | Time |
|---|---|
| Bare-name edge resolution | 0.48-0.53 s |
| Centrality score persistence | 0.22 s |
| Native binding resolution | 0.11 s |
| Orphaned structure pruning | 0.08 s |
| Endpoint demotion, summaries, the rest | about 0.1 s |

Bare-name resolution is about 20 passes of 10-90 ms each, with no hotspot.
An edit re-parses 18-32 files (123 for a widely imported module), which
hold 0.2-3% of the 54,600 unresolved `CALLS` edges (19% for that module), so
the edge count would allow scoping. But about 85% of the time is in passes
that learn from the whole graph (`mark_observed_method_calls`,
`resolve_returned_receivers` three times, `mark_stdlib_method_calls` and
`mark_glob_imported_external_calls` and `mark_deref_calls` twice each,
`resolve_pyo3_methods`, `reconcile_tested_by_with_calls`): an edit can teach
them something that resolves an unchanged file's edge, so scoping them is not
equivalent to the global pass on the same state. The pure resolvers that
could be scoped safely (`bind_bare_call_targets`, re-exports, inheritance)
take about 60 ms together. Centrality is already limited to the communities
the changed files belong to.

What is left are speedups that keep the result the same: each learning pass
rebuilds the map of function names to files (about 6 ms) and the import
targets (about 5 ms) and re-scans every unresolved `CALLS` edge through
`json_extract` (about 16 ms), and `reconcile_tested_by_with_calls` scans all
calls before joining test nodes (0.10 s; 0.06 s starting from the 3,727 test
nodes). Sharing those reads across the passes is estimated at 0.15-0.2 s of
the 1.1 s. Reaching well below a second needs a different design: store what
each learning pass learned, and re-run a pass over unchanged edges only when
that changes.

In-process consequences: the launcher restores the default SIGINT handler
while Rust runs, since Python would only act on Ctrl-C between bytecodes;
the hook budget watchdog's `process::exit` ends the Python process, which
has printed nothing. On a repository without commits, Python's `status`
also logs a `git diff failed` warning on stderr; Rust's does not.

- Move tool bodies in `dagayn/tools/` (mostly JSON shaping over `GraphStore`)
  into `dagayn-graph` behind a JSON API.
- Replace git subprocesses on the freshness path with `gix`.

### 5.3 Rust MCP server

The server has 21 tools and 5 prompts, no resources or middleware.

First slice (done): a stdio front end in Rust, `crates/dagayn-mcp`, in the
same process as the Python server rather than `rmcp` in a separate binary.
It answers `initialize` (handshake revisions 2024-11-05 to 2025-11-25,
counter-offering 2025-11-25), `ping`, `tools/list` (filtered by the
`--tools` / `CRG_TOOLS` surface), `prompts/list`, the empty resource
listings, and `logging/setLevel` before the Python server runs, from
`dagayn/server/mcp_surface.json`, which `tools/mcp_snapshot.py --regenerate`
records from the fastmcp server and a test keeps current. Every other
message (`tools/call`, `prompts/get`, unknown methods, anything before
`initialize`, the 2026-07-28 envelope, pagination cursors) boots the fastmcp
server of `dagayn.server.main` in a thread on a pipe pair, replays the
client's `initialize` to it under the id `dagayn-proxy-init`, and relays the
session from then on, so validation, serialization, errors, and
notifications are fastmcp's own. As fastmcp's stdio loop does, fd 0 then
reads `/dev/null` and fd 1 writes to stderr. On stdin EOF the front end
closes the pipe, waits for the Python server to finish, and returns, so a
local embedding sidecar around `serve` is stopped normally. Only the
`dagayn` console script uses it; `--http`, `python -m dagayn serve`, and
`DAGAYN_PYTHON_CLI=1` keep fastmcp's own loop. `serverInfo.version` is
dagayn's version instead of fastmcp's.

`tools/mcp_snapshot.py` now also freezes `protocol.json`: the raw replies to
the handshake (three revisions and a request before `initialize`), the full
listings, every prompt rendered, argument errors, and unknown names. Taking
it showed that `prompts/get` had failed for every prompt under fastmcp 4,
which is fixed. CI runs the snapshots against the installed `dagayn serve`.

Measured on this repository through the console script:

| Session | fastmcp loop | Rust front end |
|---|---|---|
| `initialize` round trip | 0.50 s | 0.15 s |
| peak memory, listing only | 150 MB | 66 MB |
| start to first `query_graph_tool` result | 0.66 s | 0.62 s |
| peak memory after that call | 216 MB | 212 MB |

The console script runs `serve` with only its own command module (the other
twelve cost 0.07 s), under the same corrupt-graph and jj guards as the full
CLI. The first call still loads fastmcp and the tools, so a session that
calls a tool ends where it did. The remaining 0.15 s is Python's start and
the `serve` preamble: inferring the embedding provider from the graph loads
`embeddings_store`, which pulls in numpy and, through `embeddings_text` and
`local_embeddings`, the pydantic contracts (about 0.05 s); deferring one of
those imports gained nothing while the others remain.

Second slice (done): native tools. `crates/dagayn-tools` answers
`list_graph_stats_tool` and `get_docs_section_tool`, and the front end tries
it for a `tools/call` of an exposed tool before it delegates. It answers only
what it can answer exactly as Python would: arguments that are the declared
ones with plain JSON types (anything fastmcp would coerce or reject goes to
fastmcp), a `repo_root` given by the client or pinned by `serve --repo`
(auto-detection is Python's), an existing graph in the default location with
no `CRG_DATA_DIR` or legacy `.dagayn.db`, recording this repository, whose
shared lock is free right now (a busy graph is Python's to wait for, so the
reader never blocks), and for the docs a section that exists. The payload
keeps the top-level key order of the Python response; nested maps are
sorted, where Python's order already came from a hash map. `_hints` and
`next_tool_suggestions` follow the session's tool surface as
`tool_surface.filter_suggestions` does. A session whose calls are all
native never loads Python's server.

Porting `list_graph_stats` showed that the Python tool created the
embeddings table as a side effect (it constructed an `EmbeddingStore` to
count), so `dagayn status` reported "empty" instead of "not indexed" after a
stats call; both now count read-only.

Two SQLite libraries share every Python process that loads `_core`:
Python's `sqlite3` and the copy compiled into the extension. POSIX locks are
per process, so neither sees the other's, including the WAL-index locks that
keep a checkpoint off a reader's pages. Reproduced: on a graph without
embeddings tables, `hybrid_search` with no provider opened and cached a
writable `EmbeddingStore` (creating the schema on a read path), the native
store's close then counted itself last and deleted `-wal` and `-shm`, and
every later `sqlite3.connect` in the process failed with "disk I/O error",
so all later answerability read as missing tables. It surfaced only after
`list_graph_stats` stopped creating the table first. Search no longer opens
a store without a provider, `dagayn-tools` opens graphs with
`GraphStore::open_read_only` (a read-only connection never deletes the WAL
files), and the Rust build creates the embeddings schema as Python's does (a
gap `export_db` now catches by recording `sqlite_master`). The hazard
remains wherever Python writes the graph while the native store holds it;
moving `embeddings_store` to Rust (5.4) removes it.

Third slice (done): `get_minimal_context_tool`, the call the server's
instructions put first. `dagayn-tools` answers it for a graph at HEAD
(`commit_synced`, `worktree_behind`, `worktree_ahead`) under git or no VCS,
with the task routing, the answerability summary (`answerability_counts` in
`dagayn-graph` runs the Python queries and failure codes; score and ratio are
rounded as Python's `round` rounds), the top communities and flows, and the
sync block. It delegates whatever makes Python act rather than read: an
`unbuilt` or `commit_drift` graph (a prepare is queued), a local embedding
mode with vectors missing (an embed is queued; `embedding_refresh_skips` in
`dagayn-build` mirrors `embedding_refresh_action`), `changed_files` (the risk
analysis), a freshly seeded worktree (the assessment writes its flag), jj and
SVN, and a task with characters `casefold` and `to_lowercase` treat
differently. The Python assessment writes `seeded_needs_content_verify = 0`
on every verified call; the read-only Rust one does not, which only leaves
that key absent where Python would store `0`.

Tests compare the two answers in both HEAD states for six tasks (English and
Japanese workflows) and on this repository's graph. Measured on a worktree
of this repository, from starting `dagayn serve` to the first
`get_minimal_context_tool` result:

| | fastmcp loop | Rust front end |
|---|---|---|
| start to first result | 0.64-0.72 s | 0.16-0.17 s |
| peak memory | 219 MB | 71 MB |

Fourth slice (done): `query_graph_tool` for `callers_of` and `callees_of`
at depth 1, `standard` and `minimal`, on a target that names a node exactly
(qualified name or repository path). The answerability is the full summary
with the commit-tier freshness of the root the graph records
(`commit_tier_freshness` in `dagayn-build`; the summary is now one module
shared with `get_minimal_context`), and the rows are compacted and merged as
`_compact_row` and `_merge_rows` do. An answer `apply_output_budget` would
trim is delegated: the front end computes the exact length of Python's
`json.dumps` for the budget. Python keeps every other pattern, `depth > 1`,
`full`, targets resolved by name search, builtin and external-package
targets, and `callers_of` with no direct callers (the bare-name fallback).
`DAGAYN_MCP_TRACE=1` now also prints each call answered in Rust.

On a worktree of this repository, 168 calls (42 targets, both patterns,
both detail levels) match fastmcp's answers, 128 of them answered in Rust. A
session of one `get_minimal_context_tool` and twenty `query_graph_tool`
calls never starts Python's server:

| | fastmcp loop | Rust front end |
|---|---|---|
| start to first context | 0.63 s | 0.17 s |
| twenty `query_graph_tool` calls | 1.31-1.40 s | 0.66-0.68 s |
| peak memory | 215 MB | 71 MB |

Fifth slice (done): more of `query_graph_tool`. `source_of` reads the live
span as `read_live_node_source` does (Python's `splitlines` boundaries,
`DocSection` spans to the next heading, the SHA-256 staleness check, the
character cap, `source_coverage`, and `degraded` for a stale file). Targets
are resolved by name as `resolve_query_target` does (`exact_name`, `fuzzy`,
and the early `ambiguous` and `not_found` answers), and `callers_of` with no
direct callers takes the bare-name fallback (`GraphStore::bare_name_callers`
in `dagayn-graph` reuses the build's visibility rules). Python still answers
`depth > 1`, `full`, external-package targets, and the other nine patterns.
On this repository's graph 82 `source_of` calls (functions, classes, doc
sections, files, and a stale file), 82 `callers_of` calls on random nodes,
and 288 calls on names (unique, shared, partial, missing) all match
fastmcp's answers and are all answered in Rust. `crates/dagayn-tools/tests`
covers the tools in the Rust suite, which the pytest comparisons do not
reach.

Sixth slice (done): `semantic_search_nodes_tool` when no embedding
provider takes part: no `provider` or `model` argument, no server default,
no provider in the environment (`get_provider(None)` and
`_infer_remote_embedding_provider_from_env`), and no stored vectors a
persisted provider name could revive. The embedding arm then reports
`provider_unavailable`, and `hybrid_search` is the FTS arm (with its
per-identifier sub-queries, ghost-row filtering, the keyword merge for an
`or` match, and the keyword fallback), the `kind` widening passes, RRF with
Python's tie order, the kind and dotted-name boosts, and the test deboost;
`_intent_boost` is 1.0 outside hybrid mode. 68 calls on this repository's
graph (identifiers, natural language, Japanese, kinds, limits, minimal)
match fastmcp's answers, all answered in Rust. A graph with embeddings, or
a server with a provider, still searches in Python.

Seventh slice (done): the rest of `query_graph_tool` but `tests_for`.
`imports_of`, `importers_of`, `children_of`, `inheritors_of` (with its
bare-name fallback; `GraphStore::bare_name_edges` now takes the edge kind),
`file_summary` (including its own `not_found` form), `docs_for`,
`implementations_of`, and `bridges_from`; the transitive walk of
`callers_of` and `importers_of` (`_expand_transitive`: breadth-first, each
node at its shortest hop, 500 rows, and the reachability and closing
`next_action`); `full` detail (raw rows, edge dicts with the bridge
metadata, the full answerability, and `_hints`); external-package and
builtin targets. Rows are built in Python's raw shape and compacted, merged,
and projected by one shared path, which also fixed the earlier fallback
condition (Python falls back when there are no rows, not when there are no
edges). 280 calls over every pattern, level, and depth on this
repository's graph match fastmcp's answers, all answered in Rust; the
earlier 288 name and 82 `source_of` comparisons still do. `tests_for` (the
heuristic test inference in `coverage.py`) and invalid `depth` values stay
Python's.

Eighth slice (done): one hint session for both sides. `review_tool`,
`flow_tool`, `architecture_analysis_tool`, and `refactor_tool` record each
call in `dagayn.hints`' session, whose history later hints read (called
tools leave `next_steps`, touched files leave `related`). The session now
lives in `dagayn-tools` (`hints::session()`, one per process), and
`dagayn.hints.get_session()` returns a `_core.HintSession` handle on it, so
the Python server and the tools answered in Rust record into one state;
without the extension it stays the Python `SessionState`. `hints.rs` ports
`generate_hints` for the tool names Rust will report, and a test runs the
same calls through both and compares the hints and the session after each.

Ninth slice (done): `review_tool` `mode="affected_flows"`. The change
detection (`get_changed_file_sources` and `get_staged_and_unstaged` for a
git checkout, in `dagayn-build`'s `vcs`), the flows' bridge arrivals and
step counts (`GraphStore::get_affected_flows_annotated`), the dispatcher's
envelope order, and the session: the call records `get_affected_flows` and
then `review`, as Python's does. `_runtime` names the hosting Python process
(`dagayn.runtime_identity`, handed over by `dagayn.server.proxy`). jj, svn,
a ref Python rejects, and every other mode stay Python's. Sessions mixing
Rust and Python answers match fastmcp's call by call (15 calls on this
repository's graph, 8 per worktree state in the tests), and the annotated
flows of 5 files on a graph with 216 bridge steps match Python's.

Tenth slice (done): `review_tool` `mode="impact"` (`get_impact_radius`
the tool): the blast radius from `GraphStore::get_impact_radius`, the
changed files the graph does not hold, the bridge and caveat missingness and
guidance, the minimal form, and `apply_output_budget`, now in Rust
(`Ordered::apply_output_budget`) since on a real graph nearly every standard
answer is trimmed. Two orders that changed from run to run are now fixed:
`get_impact_radius`'s seeds (sorted, so chunked lookups past 450 seeds
return the same order) and the low-confidence bridges of `changes`'
`cross_artifact_proximity` (ties broken by target and line). 44 calls on
this repository's graph and 32 on a larger copy match fastmcp's, 40 and 28
of them answered in Rust.

Eleventh slice (done): `query_graph_tool` `tests_for`, so every pattern is
answered in Rust. `dagayn.coverage`'s inference (`TESTED_BY` edges, then
each test-like candidate scored by module markers, co-location, imports,
and name or source references) is `dagayn-tools`' `coverage`, which
`changes` will share. `str.casefold` is reproduced for the characters whose
folding differs from lower-casing in a simple way; the others (expanded
Greek, Cherokee, some ligatures) leave the call to Python. 82 exact and 96
by-name calls on this repository's graph and on a larger copy match
fastmcp's, all answered in Rust.

Twelfth slice (done): `review_tool` `mode="changes"` (the default) at
`standard` and `minimal` detail. `dagayn-tools`' `changes` is
`analyze_changes` (`git diff --unified=0` ranges, renames, attribution with
stale-hash fallback, the base revision's entities parsed with the same
parser path Python passes, review-priority scores, test gaps through
`coverage`), and `review_summary` is `_change_analysis_summary` with the
architecture metrics it reads (SDP, SAP, bounded ADP cycles, stability
profiles). Hotspots use the persisted rankings and otherwise compute them
as Python does, betweenness included: `pyrandom` reproduces CPython's
`random.Random(0).sample`, and the Brandes passes run in networkx's order,
so sampled scores match to the last bit. `get_edges_by_endpoints` now sorts
its keys (its chunks reordered an endpoint's edges from run to run), and
`compute_change_risk_score` counts a caller once and rounds as Python does.
Both halves were compared with Python's functions directly on five change
sets here and on a larger copy (up to 94 files), then end to end: 40 calls
on this repository's graph match fastmcp's, 35 answered in Rust. A
`changes` call takes 2.0 s and 410 MB instead of 9.8 s and 772 MB here.
`include_source`, `verbose`, and a base `git diff` cannot resolve stay
Python's.

Thirteenth slice (done): `review_tool` `mode="context"`, so every review
mode is answered in Rust. The impact graph with its 300-entry caps, source
snippets read through `resolve_contained_path` (out-of-repo paths are
listed, not read) and numbered as Python numbers them, the relevant-lines
cut for long files, the review guidance text, the 120 KB snippet budget
that drops or clips, and the minimal form. 47 calls on this repository's
graph and 47 on a larger copy match fastmcp's, 42 of each answered in Rust,
including dropped and clipped snippets, graph truncation, and merged
relevant-line ranges.

Fourteenth slice (done): `flow_tool` (`list` and `get`), with the stored
flows' liveness annotation, the kind filter, the minimal listing, flows
found by name, live steps re-read with their bridge arrivals, stale and
truncated flows (`degraded`), per-step source with its 2000-character cap
and output budget, and `not_found`. The dispatcher envelope is shared with
`review_tool` (`seal_dispatch`), and `hints.rs` now carries the whole
`_WORKFLOW` table: it had only two tools, so a `changes` review whose
guidance came out empty got no next steps from Rust. 30 calls on this
repository's graph, 30 on a larger copy (bridge and truncated flows), and
24 on a graph whose stored flow went stale match fastmcp's.

Fifteenth slice (done): `architecture_analysis_tool`'s metric modes,
`adp_violations`, `sdp_metrics`, `sdp_violations`, `sap_metrics`, and
`sap_violations`, at either granularity, any artifact scope and dependency
profile, with their thresholds, `unit_filter`, and `top_n` slicing as
Python slices. The scope graph moved from `review_summary` to
`architecture` and takes a `View` (granularity, artifact scope, profile).
52 calls on this repository's graph and 52 on a larger copy match
fastmcp's. An ADP enumeration Python truncates at 5000 cycles stays
Python's; its truncated list depends on networkx's visiting order, which
follows string hashing, so it differs between Python processes too.

Sixteenth slice (done): `architecture_analysis_tool`'s `hubs`, `bridges`,
`knowledge_gaps`, and `surprising_connections` (`dagayn-tools`'
`analysis`): persisted rankings where they cover the scope, otherwise the
degree ranking and networkx-order betweenness that `review_summary`
already used, now for any artifact scope and test setting; the knowledge
gaps with their p95 hotspot threshold, the source-reading classification
of isolated nodes, community edge shapes, and the gaps' own output budget;
and the surprise scores. 80 calls on this repository's graph and 80 on a
larger copy match fastmcp's, 77 and 78 answered in Rust.

Seventeenth slice (done): `communities`, `community`, and `overview`, so
every `architecture_analysis_tool` mode is answered in Rust. The overview
composes the community coupling (`Counter.most_common` order), the
health summary over every earlier analysis at the bounded `top_n`, and the
stable-component policy summary, then trims to its output budget. 101
calls on this repository's graph and 101 on a larger copy match fastmcp's,
98 and 99 answered in Rust; the only difference is the Python-only ADP
truncation filed as #179.

Eighteenth slice (done): `refactor_tool`'s `dead_code` and `suggest`
(the default). `dead_code` is `find_dead_code` with its node filters,
batched reference lookups, plausible bare-name callers, abstract-base and
base-method reachability, unresolved entrypoint bridges, and public-API
source checks; `suggest` adds the move, split, and document suggestions
with `concerns`' function profiles, the execution plans and work packs,
the stable-component guard, and the guidance. Python's split suggestion
shares one list between `reason_codes` and `evidence.reason_codes`, so the
guard appears in both; Rust mirrors that. 21 calls on this repository's
graph (1102 suggestions) and on a larger copy match fastmcp's, all
answered in Rust. `rename` stays Python's: its preview is kept in
Python's pending store for `apply_refactor_tool`.

Nineteenth slice (done): `refactor_tool` `rename`, so every default tool
is answered in Rust. The pending refactor store moved into `dagayn-tools`
(`pending`, JSON text in insertion order), and
`dagayn.refactor.pending._pending_refactors` is a mapping over it through
`_core.pending_refactor_*` (a plain dict without the extension), so a
preview made in Rust is one the Python `apply_refactor_tool` applies. The
preview's edit sites, import-line checks, identifier check, `not_found`, and
error replies match fastmcp's; 19 calls here, applying each Rust preview
with Python's `apply_refactor_tool` in the same session, differ only in the
random `refactor_id` and timestamp. Non-ASCII names stay Python's, as its
`\w` and Rust's cover different characters.

Twentieth slice (done): `ensure_graph_tool` when the prepare has nothing to
do, the call every session makes first. A git checkout that is not a linked
worktree, whose graph is `commit_synced` or `worktree_ahead` with its content
verified, with no `force`, no hook skip-when-busy contract, and nothing to
embed, is answered in Rust with the noop reply, the sealed `sync` assessment
(`assess_graph_sync` now carries `indexed_files`, `content_verified`,
`unverified_file_count`, the stored SHA, branch, and `last_updated`), and the
full `graph_health`. Building, updating, seeding, and embedding stay
Python's, so a graph that needs any of them still boots the Python server.
Here (an indexed dirty tree with complete embeddings) and on a fresh clean
checkout, synced and ahead, every call matches fastmcp's but for
`elapsed_seconds`.

Twenty-first slice (done): the read-only tools of the full surface
(`--tools all`). `find_large_functions_tool` (the store's size query, paths
made relative), `get_suggested_questions_tool` (the store's questions,
high priority first, with the analysis subtools' envelope),
`get_wiki_page_tool` (slug, then the exact name inside the wiki directory,
read with universal newlines; non-ASCII names and non-UTF-8 pages stay
Python's), `list_repos_tool` (a well-formed `~/.dagayn/registry.json`; a
missing directory, which `Registry()` would create, or a malformed file is
Python's), and `traverse_graph_tool` (BFS by batched layers or lazy DFS from
the keyword arm's top hit, under the same gate as
`semantic_search_nodes_tool`). Every call compared here, on a larger copy, and
on a fresh checkout matches fastmcp's.

Twenty-second slice (done): `apply_refactor_tool`, reading the preview
from the shared pending store. Edits are applied at their recorded line to
whole identifiers outside single-line strings, files are read with
universal newlines and split as `str.splitlines` splits them, and a dry run
returns `difflib.unified_diff` as CPython 3.14 computes it (`difflib` in
`dagayn-tools` ports `SequenceMatcher` with its popular-element
heuristic; 3000 random line sequences, up to 450 lines, match Python's
output). Every file is read before any is written; a file that is not UTF-8
or an edit path that does not exist yet stays Python's. Applying a Rust or a
Python preview here, dry and for real on two copies of a checkout, gives the
same replies and the same trees.

Next: the tools that write (`generate_wiki_tool`,
`build_or_update_graph_tool`, `run_postprocess_tool`, `embed_graph_tool`) and
`cross_repo_search_tool`, then the Python server's remaining role.

### 5.4 Remaining Python surfaces

| Python | Rust |
|---|---|
| `daemon.py`, `incremental_update_pipeline.py` (watchdog) | `notify` |
| `analysis.py`, `architecture.py` (networkx betweenness, `simple_cycles`) | `petgraph` or in-crate code |
| `embeddings_store.py` (numpy) | `ndarray` or plain slices |
| `embeddings_providers.py` | `reqwest` |
| `local_embeddings.py` | unchanged design: it already runs `llama-server` as a sidecar |
| `refactor/` | `dagayn-graph` |

### 5.5 Install and skills

- Embed skill and instruction templates with `include_str!`.
- Port platform config editing (`skills/platforms.py`, `skills/hooks.py`).

### 5.6 Delete Python and switch distribution

- Publish a PyPI wheel that contains only the binary, as ruff and uv do, so
  `uv tool install dagayn` keeps working. Add `cargo-dist` and Homebrew.
  Until then the Rust CLI ships inside `_core` (5.2, fourth slice).
- `eval/`, `wiki.py`, `visualization/`, and the matplotlib export in
  `exports.py` stay as Python dev scripts or are removed.

## Acceptance criteria

1. `tests/test_mcp_snapshots.py` passes with `DAGAYN_CLI_CMD` and
   `DAGAYN_MCP_SERVER_CMD` pointing at the Rust binary.
2. The pytest suite that remains after deleting the Python tests it
   covered passes.
3. Binaries build on macOS arm64/x86_64, Linux x86_64/aarch64, and Windows.
4. Existing installs upgrade without editing hook or MCP config.

## Risks

- **Response-shape drift** while tool bodies move; the 5.1 snapshots catch
  it on the fixtures they cover.
- **MCP SDK parity**: prompt argument handling and error shapes in `rmcp`.
- **Test strategy**: the snapshots cover less internal state than today's
  unit tests, and only nine small fixtures.
- **Scope**: if 5.0 removes the latency users notice, the rest of the
  migration competes with feature work on maintenance value alone.
