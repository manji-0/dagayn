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

In-process consequences: the launcher restores the default SIGINT handler
while Rust runs, since Python would only act on Ctrl-C between bytecodes;
the hook budget watchdog's `process::exit` ends the Python process, which
has printed nothing. On a repository without commits, Python's `status`
also logs a `git diff failed` warning on stderr; Rust's does not.

- Move tool bodies in `dagayn/tools/` (mostly JSON shaping over `GraphStore`)
  into `dagayn-graph` behind a JSON API.
- Replace git subprocesses on the freshness path with `gix`.

### 5.3 Rust MCP server

- `rmcp` replaces fastmcp. The server uses 21 tools and 5 prompts, no
  resources or middleware.

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
