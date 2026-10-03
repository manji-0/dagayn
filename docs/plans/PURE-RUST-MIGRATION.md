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

### 5.1 Freeze response shapes as Rust types

- Port `dagayn/contracts/state_types.py` to serde structs in a new crate.
- Turn pytest into a black-box harness: it runs the binary and asserts on
  JSON output. It stays as the parity oracle through the migration.
- Port only unit tests for pure logic to Rust.

### 5.2 Rust CLI

- `clap` binary named `dagayn`, with the current subcommands and flags, so
  `hooks.json`, `.mcp.json`, and installed skills keep working.
- Start with commands that need no MCP: `status`, `update`, `build`.
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
| `parser/manifest_bridges.py` | `dagayn-postproc` |
| `refactor/` | `dagayn-graph` |

### 5.5 Install and skills

- Embed skill and instruction templates with `include_str!`.
- Port platform config editing (`skills/platforms.py`, `skills/hooks.py`).

### 5.6 Delete Python and switch distribution

- Publish a PyPI wheel that contains only the binary, as ruff and uv do, so
  `uv tool install dagayn` keeps working. Add `cargo-dist` and Homebrew.
- `eval/`, `wiki.py`, `visualization/`, and the matplotlib export in
  `exports.py` stay as Python dev scripts or are removed.

## Acceptance criteria

1. The black-box harness passes against the Rust binary on every fixture.
2. MCP tool and prompt responses match the 5.1 serde shapes byte for byte
   on parity fixtures.
3. Binaries build on macOS arm64/x86_64, Linux x86_64/aarch64, and Windows.
4. Existing installs upgrade without editing hook or MCP config.

## Risks

- **Response-shape drift** while tool bodies move; 5.1 exists to catch it.
- **MCP SDK parity**: prompt argument handling and error shapes in `rmcp`.
- **Test strategy**: the black-box harness covers less internal state than
  today's unit tests.
- **Scope**: if 5.0 removes the latency users notice, the rest of the
  migration competes with feature work on maintenance value alone.
