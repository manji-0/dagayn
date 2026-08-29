# Changelog

All notable changes to `dagayn` are documented here.

## Unreleased

### Features

- Native FTS indexes Japanese with Lindera IPADIC morphemes (plus base forms)
  and overlapping CJK bigrams, then queries with content morphemes only. Inflected
  queries such as `検索する` AND-match `検索を行う` instead of missing or falling
  through to OR. CJK symbol names also land in `identifier_tokens`. Quality
  gates run on the mixed fixture `tests/fixtures/japanese_search/` (Markdown +
  Python + Terraform), not a 7-node inline corpus.

### Changed

- Parser nodes and edges carry `NodeKind` / `EdgeKind` instead of heap
  strings. The closed set now includes `Type`, `DocBody`, and `IMPLEMENTS`,
  matching the schema. SQLite and the Python store still see the same labels.
- Frozen collections use `Box<[T]>` (flow paths, community members, language
  lists, Brandes adjacency). Flow tracing shares each `GraphNode` via `Arc`
  across the qualified-name and id indexes instead of cloning the record
  twice.
- Parser `file_path` is a shared `FilePath` (`Arc<str>`). Every node and edge
  from one file clones the same handle; SQLite and Python still see a string.

## 4.12.0 — 2026-08-29

### Features

- Bare-name call resolution understands namespaces. A C# `using` or a PHP `use`
  names a namespace rather than a file, so no file-to-file `IMPORTS_FROM` edge
  ever exists, and two files in one namespace need no import statement at all —
  the resolver had no evidence they could see each other. Parsers now record the
  namespaces a file declares (C# `namespace` including the file-scoped form,
  Java/Kotlin/Scala `package`, PHP `namespace`, Elixir `defmodule`, Julia
  `module`) on its `File` node, and a file is visible when it shares a declared
  namespace or declares one the caller imports. Held as per-file maps rather
  than an expanded file-to-file product, since a namespace with N files would
  otherwise cost N² entries. Both backends implement the same rule.
- A symbol is also visible through the file that declares its class. A C++
  method is defined in a `.cpp` nobody includes, while callers include only the
  header declaring the class, so file-level visibility alone could never resolve
  it.
- Include and URI imports resolve to repo-relative files. C/C++/Objective-C
  `#include` searches the including directory and its ancestors plus a parallel
  `include/` tree, resolving only to files that exist — `<vector>` keeps its
  literal name. Dart resolves relative URIs, and `package:X/...` only when the
  nearest `pubspec.yaml` declares `X`. Julia resolves `include("f.jl")` relative
  to the including file.

### Fixes

- C# methods whose return type is a user-defined type were indexed under the
  return type instead of the declared method name, because the name came from
  the first direct `identifier` child and tree-sitter-c-sharp represents such a
  return type as a plain `identifier`. `callers_of` then reported no callers and
  dead-code analysis produced false positives. Declaration names now come from
  the `name` field. See #154.
- C# `Factory.Method(...)` and `new Type(...)` produced no `CALLS` edge at all,
  so those relationships were absent rather than merely mis-targeted.
- C++ class and struct member functions were missing from the graph entirely: a
  member's declarator name is a `field_identifier`, and an out-of-line
  definition's is a `qualified_identifier`, neither of which is an `identifier`.
  Names now follow the `declarator` field chain, and an out-of-line definition
  is attributed to the class its scope names.
- Java `Broker.build(t)` targeted the receiver class rather than `build`, since
  `method_invocation` puts the receiver first.
- Dart attributed every call in a method body to the file, because a Dart body
  is a sibling of its signature rather than a child. Function nodes also spanned
  only the signature line and now cover the body.
- PHP static calls carried a `Class::method` target, which bare-name resolution
  treats as already-qualified and could never bind to a node. The
  `Class::method` form is kept only as cross-artifact bridge evidence.
- Import targets that kept the whole statement, matching nothing: Kotlin
  `import java.util.UUID`, Swift `import Foundation`, Objective-C
  `#import "Logger.h"`, PHP `use Exception;`. PHP group form
  (`use A\B\{One, Two};`) expands per clause and drops an `as` alias.
- C# `abstract` was only detected when it was the first modifier, so
  `public abstract class` was recorded as a plain class.

### Internal

- `NamespaceVisibility` became `SymbolVisibility`, carrying the class-name to
  declaring-file index alongside the namespace maps. The Rust store exposes it
  as `symbol_visibility_by_file` for query-time resolution.

## 4.11.0 — 2026-08-28

### Features

- The native `GraphStore` implements the full public method surface of the
  Python one, so a tool can hold either backend without probing for `_conn` or
  for a method's existence. The 28 methods that were missing are now in
  `dagayn-graph` and bound in `dagayn-py`: node and edge lookups, subgraph
  extraction, FTS5 and LIKE search, impact radius with `CROSS_ARTIFACT` bridge
  classification, community and flow lookups, single-row upserts, and
  derived-table maintenance. Two behaviours differ by design and are documented
  at their definitions: there is no NetworkX arm for `get_impact_radius`
  (`CRG_BFS_ENGINE=networkx` applies to the Python store only, and both engines
  agree), and native `fts_query` segments Japanese with the same Rust bigram
  splitter the Rust index path writes with. See
  `docs/RUST-CORE-MIGRATION-WIP.md`.

### Fixes

- `refactor_tool` failed outright under the default Rust backend, because every
  mode read through Python-only store APIs: `dead_code` on `store._conn`,
  `suggest` on `get_communities_list`, `rename` on `search_nodes`. All three
  work natively now, so `DAGAYN_BACKEND=python` is no longer needed as a
  workaround. See #153.
- `semantic_search_nodes_tool` failed under the Rust backend for the same
  reason, on `fts_index_health(store._conn)`. FTS index health and the
  embeddable-node count are store methods on both backends now.
- The native store's `db_path` returned a `str` where the Python one returns a
  `Path`, so embedding search could not `stat()` the file and semantic ranking
  silently contributed nothing.
- Native `remove_files_data` never deleted `flow_memberships`. Node ids are
  autoincremented, so every re-parse left rows pointing at ids that never come
  back, and `prune_orphaned_graph_structures` kept re-doing the cleanup the
  Python path had already done in place.
- Native `GraphNode` carried no `signature`, so nodes materialized by the Rust
  backend always reported `None` regardless of what post-processing had
  persisted.

### Internal

- `refactor/dead_code.py` no longer issues raw `store._conn` SQL. The queries
  it needed (`get_edges_by_kind`, `count_nodes_by_name`,
  `get_edges_by_sources`/`_targets`/`_target_names`,
  `get_nodes_by_parent_and_name`, `has_edge_to_target`) are store methods
  present on both backends.
- `prune_orphaned_graph_structures` is orchestrated from `dagayn-postproc`
  rather than `dagayn-graph`, because its `communities` step needs the Leiden
  cohesion code that `dagayn-graph` cannot depend on.

### Testing

- `tests/test_rust_graph_store_parity.py` asserts both halves of the contract:
  that no public Python `GraphStore` method is missing natively, and that the
  two backends return equal results for one graph built through each. The three
  `db_path`/`flow_memberships`/`fts_index_health` fixes above were found by it.
- Rust unit tests cover `CROSS_ARTIFACT` bridge classification directly, where
  the rules are subtle: `MEDIUM` is neither a reportable claim nor a caveat on
  its own, but the same edge *is* a caveat when it carries resolved implicit
  Markdown code-span evidence. Each expectation was cross-checked against
  `dagayn.cross_artifact`.

## 4.10.5 — 2026-08-24

### Changed

- MCP `get_minimal_context_tool` no longer waits for `session_prepare`. When
  the graph is empty or HEAD-drifted it enqueues a single background prepare
  and returns the current `sync` plus `repair`/`prepare` queued state. Call
  `ensure_graph_tool` to wait. Hook `session prepare` now skip-when-busy like
  `dagayn update`. Shared-to-exclusive flock upgrades release then wait with
  a timeout instead of blocking forever; native store `close` always unbinds
  the read lock (via a proxy when PyO3 cannot patch `close`).

## 4.10.4 — 2026-08-23

### Changed

- Incremental post-process no longer rebuilds the whole FTS index. Changed
  files call `sync_fts_for_file_paths`; a missing `nodes_fts` table still
  falls back to a full rebuild. Brandes betweenness now uses a dense integer
  graph, generation-stamped BFS, and `min(500, max(64, ceil(5√V)))` sampling
  above 5000 nodes. Centrality community ids are loaded in one batch; dirty
  updates recompute only changed communities plus neighbors. Leiden oversized
  splits partition edges in `O(E)`, incremental Leiden replaces the dirty
  region instead of the whole partition, flow criticality refresh queries only
  dirty flow ids, bare-name resolution uses a name index, and manifest
  extraction is skipped when no manifest files changed. Incremental community,
  centrality, and flow post-process now build those subgraphs in SQLite from
  the dirty set instead of materializing every node and edge first; a region
  larger than 50% of non-file nodes still falls back to a full snapshot.
- Drop unused Python and Rust dependency declarations. The core install no
  longer pins `mcp` (FastMCP already requires `mcp>=1.24,<2`), the empty
  `communities` / `enrichment` / `wiki` extras and `pytest-asyncio` are gone,
  and the unused `anyhow` workspace crate is gone. `dagayn[google-embeddings]`
  now installs `google-genai`, which is the SDK `from google import genai`
  actually imports.
- Local embedding refresh no longer starts the sidecar on every incomplete
  coverage report. Session prepare and MCP first-tool inline the pass only
  when the index is empty or missing coverage is at least 5%
  (`DAGAYN_EMBED_INLINE_MISSING_RATIO`); smaller holes are queued. After a
  structure-only edit-hook update, a file-scoped `embed` task hash-skips just
  the changed and dependent files so comment-only edits (material `text_hash`
  changes with `complete` coverage) catch up without a whole-corpus scan. The
  queued pass infers the stored sidecar preset (BGE-M3 vs Qwen) so a Qwen
  graph does not spawn the BGE-M3 default. Sidecar inference now strips the
  `#text=` partition suffix so stored keys match `dagayn serve`.

### Fixes

- User-level `dagayn serve` no longer freezes the first resolved repository
  into every later MCP tool call. An omitted `--repo` is resolved per call
  from IDE workspace hints; two or more unrelated hints with `cwd` inside
  none of them are an error instead of ranking graphs by mtime. Project-level
  Cursor MCP config now sets `cwd` to `${workspaceFolder}` so that process
  starts in the repo, while the user-level `~/.cursor/mcp.json` copy still
  omits `cwd`/`--repo` (that variable is the folder containing the user
  config, not the open project).

## 4.10.3 — 2026-08-19

### Fixes

- `dagayn build --force-full-build` no longer deletes the graph it cannot
  replace. The delete ran before anything took the write lock, so when the lock
  turned out to be held the rebuild failed to acquire it and the old graph was
  already gone — one repository lost a 21 GB graph this way to a holder that had
  owned the lock for 26 hours. The delete now happens under the write lock and is
  skipped, naming the holding pid, when the lock cannot be taken. The lock is
  released again before the build, because holding it across the whole command
  would keep it held through the embedding pass too (the acquisition is
  reentrant, so the pass's own release would not free it) and lock out every
  reader for the duration.
- `dagayn session prepare` can no longer run without limit. Its
  `--budget-seconds` was only consulted between phases, to decide whether to
  start the next one, and could not interrupt a phase already running: one run
  took ~5 minutes against a 45 s budget, and another held the graph's exclusive
  lock for 26 hours (21.5 h of CPU) — the stall every MCP tool call on that graph
  was queued behind. The CLI now arms the same hard watchdog `dagayn update`
  uses. The hard stop is 4× the advisory budget rather than the budget itself,
  because killing at the budget would kill a phase that needs slightly longer on
  every session start and the graph would never converge. `--budget-seconds 0`
  stays unbounded, and the MCP path is never killed this way.
- A `dagayn build` that failed now exits non-zero. Reporting `0 files, 0 nodes`
  and exiting 0 made a build that never ran indistinguishable from an empty
  repository.
- Git worktrees checked out inside a repository (`.worktrees/`,
  `.claude/worktrees/`) are no longer indexed. A worktree is another checkout of
  the same history, so indexing them multiplies the graph by the worktree count:
  one real graph held 1,651,932 nodes across 122,121 files, of which 98.2 %
  (1,621,706 nodes / 39 worktrees) were duplicates of the 45,432 nodes the
  repository itself contributes. `git ls-files --others` stops at the
  nested-repo boundary and reports the directory, which the `is_file()` checks
  already drop — but the directory-walk fallback used when git returns nothing
  has no such boundary, and nothing prunes what an earlier version indexed that
  way.

### Performance

- A sliced embedding pass now scans the corpus once instead of once per slice.
  Deciding what still needs embedding requires the graph store and costs ~8 s on
  a 42k-node graph, while a slice embeds for 4 s — so two thirds of every pass
  was spent re-deriving work it had already derived. The scan now produces a
  `(qualified_name, text, text_hash)` work list and the write half runs with no
  graph store open at all. Measured on that graph, one 420 s queue pass went from
  6 slices / ~390 nodes to **85 slices / 4,801 nodes**, with per-slice wall time
  down from 12.2 s to 4.1 s against a 4 s budget. The work list is held in memory
  for the pass (~20 MB for 42k nodes), which is the cost of not re-scanning.

### Internal

- The embedding pass logs each slice's wall time and each scan's candidate count
  at INFO. On a large graph the per-slice rescan, not the embedding, is what
  dominates a sliced pass, and that was not visible from the outside.

## 4.10.2 — 2026-08-19

### Fixes

- MCP tool calls no longer go silent for two minutes when a build owns the
  graph. The tool entry point kept the writer's 120 s budget while waiting for
  the shared lock, so a call that collided with a build or an embedding pass
  hung and then failed anyway. Readers opened for a tool call now wait
  `DAGAYN_READ_LOCK_TIMEOUT` (default 10 s) and report which pid is writing and
  how to wait longer. Batch readers (`detect-changes`, enrichment) keep the long
  budget, because for them waiting is the useful behaviour. A reader still holds
  the shared lock for as long as its connection is open — that invariant is what
  keeps a writer's WAL checkpoint from tearing `sqlite_master`.
- Two read-only tool calls on one graph now overlap inside a single process.
  The per-path thread lock was an `RLock`, so a long-lived `dagayn serve`
  serialized every call on the same graph even though the flock they take is
  shared and two separate processes read in parallel; with the interactive read
  timeout above, a slow call made its neighbour fail rather than queue. The lock
  is now a reader/writer lock that serves waiters in arrival order — mode
  preference in either direction starves the other side, so FIFO keeps the queue
  worker's update from being shut out by a busy MCP server. Readers and a writer
  inside one process still never overlap, which is what keeps the single flock
  handle from being held in two modes at once.
- The embedding pass no longer holds the graph's exclusive lock while the local
  embedding sidecar starts and loads its model (up to `--local-embedding-timeout`
  seconds, 300 by default) — that time touches no sqlite, but every reader was
  locked out for it. The structural build now hands the lock back before the
  embedding pass, and the pass takes it again around its database work; the
  orphaned-embedding prune takes its own.
- The embedding pass now runs as a series of time-bounded slices, taking the
  graph lock once per slice instead of once for the whole corpus. Measured on a
  710-node repository the pass held the lock for 22 s straight, and 2 of 6
  concurrent MCP-style reads failed on the 10 s reader timeout; a slice is
  capped at 4 s (`DAGAYN_EMBED_SLICE_SECONDS`, 0 restores one-shot behaviour),
  so a reader or a queued update waits for one slice. Each provider batch was
  already persisted, so slicing adds no new resumption risk; the whole-corpus
  orphan and retired-partition sweeps still run once per pass. Re-measured on
  the same repository: 167 of 167 reads succeeded, the longest waiting one slice
  (4.6 s), and the pass itself took 20.7 s against 21.1 s unsliced — the slice
  overhead was inside the noise. Waiters poll for the file lock rather than
  queuing on it, so the pass also stays out of the lock briefly between slices
  and the poll interval is capped at 0.1 s; without that, releasing and
  instantly re-taking the lock handed over to nobody and reads still failed.
- An embedding run too large for one queue budget now finishes instead of dying.
  At the measured ~32 nodes/s the 600 s `embed` budget covers roughly 19,000
  nodes, and anything beyond that was killed by the watchdog on every attempt
  until the task was parked `dead` with embeddings permanently incomplete. A
  queued `embed` now stops at a slice boundary once it has spent 70 % of the
  budget and queues a follow-up for the rest, carrying the sidecar settings over
  so the model is not reloaded. A pass that reports leftovers without embedding
  anything does not re-queue, because it would otherwise spin forever on input
  the provider keeps rejecting.
- A hook-triggered `dagayn update` no longer waits out a concurrent writer's
  whole budget while resolving its diff base, then skips anyway. The CLI peeked
  `git_head_sha` under the shared lock's full 120 s timeout before reaching the
  non-blocking write lock, so an update fired while a build or an embedding pass
  owned the graph hung the editor for up to two minutes. The base peek is now
  non-blocking for hook runs (it prints that it is skipping and returns);
  manual updates still wait. The queue worker's stored-`base` peek is bounded by
  `DAGAYN_READ_LOCK_TIMEOUT` instead of the writer's budget.

## 4.10.1 — 2026-08-18

### Fixes

- The queue worker no longer imports dagayn from the hook's working directory.
  `python -m` prepends the cwd to `sys.path`, so a hook firing inside a
  checkout that ships a `dagayn/` package (dagayn's own repository, most
  obviously) made the worker load that source tree along with any stale
  compiled `_core` beside it, and the task failed with a confusing
  "Rust post-processing requires dagayn._core support" error. The worker is
  now spawned with `-P` and inherits the caller's own import location via
  `PYTHONPATH`, so an uninstalled source checkout keeps working too.

## 4.10.0 — 2026-08-18

### Features

- Edit-triggered hooks enqueue a structure-only update on a per-repository
  SQLite task queue (`dagayn queue add` / `run` / `status` / `clear`) instead of
  spawning their own `dagayn update`. A burst of edits coalesces into one task
  drained by a single detached worker, so the graph no longer re-diffs per
  keystroke batch, and the last edit of a burst is no longer lost to a skipped
  overlapping run. See `docs/COMMANDS.md`.

### Performance

- Tool latency cut by sharing one `GraphSnapshot` across sub-analyses,
  persisting code-scope hub/bridge scores so `artifact_scope=code` queries skip
  the betweenness recompute, and batching coverage scans instead of issuing
  N+1 queries per changed function: `architecture_overview` 3.4s → 0.77s,
  `review_changes` 2.8s → 1.5s, `get_minimal_context` 65s → 0.87s on large
  diffs.

### Fixes

- Hook-triggered updates can no longer run away. On a 153k-file monorepo one
  could burn CPU for over an hour and grow the WAL to 9.1GB against a 514MB
  graph, because neither Claude Code's hook timeout nor Cursor's detached
  `afterFileEdit` kills dagayn itself. Hook updates now self-terminate after a
  budget (120s by default, `--budget-seconds` to override), `journal_size_limit`
  is set on every writing connection, the `PostToolUse` matcher no longer fires
  on `Bash`, and `.dagayn/hook-skip` opts out of hook-triggered updates.
- Config files (`~/.cursor/hooks.json`, `settings.json`, generated hook
  scripts) are written atomically, so a concurrent editor write can no longer
  leave them as invalid JSON. Externally managed read-only files and symlinks
  (nix/home-manager) are refused instead of replaced.
- The per-edit hook path no longer generates embeddings; the session-start hook
  refreshes them instead.
- The hook task queue no longer loses the tail of an edit burst: `enqueue`
  now looks up its pending twin and writes inside one `BEGIN IMMEDIATE`
  transaction, so it serializes against `claim`. Previously the twin could be
  claimed between the two statements, the new work was folded into a task the
  worker had already read, and it vanished when that task completed.
- The queue worker recovers tasks left `running` by a worker that died
  mid-execution (budget watchdog `os._exit`, crash, `SIGKILL`). Such rows were
  invisible to `claim` and stayed in the queue forever, permanently skewing
  `dagayn queue status`. Tasks whose attempts are already spent are parked
  `dead` instead of requeued.
- The queue worker's idle poll no longer takes the queue write lock: it asks a
  plain read whether anything is pending first, instead of opening
  `BEGIN IMMEDIATE` ~120 times per idle minute and making `queue add` wait
  behind an empty poll.
- Task kinds carry a default priority (`update` 10, `embed`/`postprocess` 0),
  so an edit-triggered update is claimed ahead of an `embed` that was queued
  before it. `--priority` still overrides it. A running task is still not
  preempted.
- Failed tasks now back off before the retry (1s per attempt spent, capped at
  10s) instead of re-running three times back to back.
- `task_log` is trimmed to its last 200 rows as it is written, so a long
  session cannot grow the queue database without bound.
- Queue timestamps carry their UTC offset, which offset-naive local time did
  not — it was ambiguous across a DST fold and not comparable between
  machines.
- Removed `TaskQueue.pending_kinds()`, which no production code called.
- pyrefly reports no errors for the project again; the type-check job was red.

### Internal

- Container payloads that were `dict[str, Any]` now carry real types (#150),
  and post-processing, build, and change-analysis results are typed models.
- Oversized modules are split and the `tools`/`refactor` import cycle is
  broken (#148).
- Agent instructions document the dagayn MCP tools.

## 4.9.0 — 2026-08-16

### Features

- Rust file discovery now indexes compound Terraform extensions
  (`.tftest.hcl`, `.tfcomponent.hcl`, `.tfdeploy.hcl`, `.tfquery.hcl`) so the
  default backend stops dropping them from full builds and stops deleting their
  nodes on incremental updates. See: #135
- Terraform JSON syntax (`.tf.json` / `.tfvars.json`) is now detected and
  parsed on both backends: resource/data/module/variable/output/check blocks
  are extracted directly from the JSON document, and tfvars files keep a File
  node. See: #138
- Notebooks with non-python/r kernels (Julia, Scala, SQL, Databricks) keep a
  File node instead of vanishing from the graph. See: #137

### Fixes

- MCP tools recover from `SQLITE_CORRUPT` (`database disk image is malformed`)
  by closing leaked graph connections and retrying once. Idle `GraphStore.close()`
  now issues `PRAGMA wal_checkpoint(PASSIVE)` so a long-lived `dagayn serve`
  does not keep leftover WAL generations. See `docs/TROUBLESHOOTING.md`.
- Production Terraform `check` blocks are no longer classified as tests
  (`is_test`), so plan/apply health checks stay in review and gap analysis.
  See: #136
- FTS keyword search folds case in Python for non-ASCII identifiers
  (Greek/Cyrillic uppercase no longer fails the LIKE fallback) and Hangul text
  is bigram-segmented like CJK; qualified_name/file_path are bigrammed too.
  See: #139
- `dagayn-vscode` watch mode no longer dies after 60 s: the watcher is spawned
  without a timeout, tracked, and killed on extension deactivate. See: #141
- `dagayn-vscode` can now be packaged as a VSIX under pnpm: a staging script
  installs production deps with npm (vsce requires `npm list`) and bundles the
  better-sqlite3 native module pinned to VS Code's Node 22 ABI. CI validates
  the package. See: #142
- `dagayn-vscode` settings are wired: `dagayn.graphTheme` and
  `dagayn.graph.defaultEdges` apply to the symbol graph webview, and
  `dagayn.treeView.show*` filters the tree view. See: #143
- `dagayn-vscode` registers an editor hover provider so symbol docstrings are
  shown on hover as the README advertises. See: #144
- `dagayn-vscode` backend onboarding now runs on activate and installs the
  fork from GitHub (not PyPI); the embed-failure hint no longer names the
  nonexistent `dagayn[embeddings]` extra. See: #145
- `dagayn-vscode` `buildGraph({fullRebuild})` passes `--force-full-build`
  instead of the nonexistent `--full`. See: #146
- `dagayn-vscode` webview (`src/webview/graph.ts`) is now covered by TypeScript
  checking via a dedicated `tsconfig.webview.json`. See: #147
- Dispatcher-wrapped MCP tools (`review_tool`, `flow_tool`,
  `architecture_analysis_tool`) no longer crash with a pydantic
  `DispatcherOkResponse` validation error when their subtool returns a
  structured error envelope: `review_tool(mode="changes")` in a single-commit
  repository (no `HEAD~1` diff base) now reports `status: "error"` with the
  `diff_base_unreachable` reason code and actionable guidance instead of
  failing. `refactor_tool(mode="rename")` apply guidance now states that
  `refactor_id` is session-scoped (apply within the same `dagayn serve` MCP
  session).

### Documentation

- `docs/PERFORMANCE-IMPROVEMENTS-WIP.md` status markers updated: shipped items
  (§4.6 target-name index, §4.7 mtime skip, §4.8 caches, §3.1 mcp_latency)
  are no longer marked "not yet implemented", and the nonexistent
  `dagayn update --post` flag reference was corrected. See: #140

## 4.8.4 — 2026-08-15

### Changed

- Pre-push pytest runs tests related to the files being pushed instead of the
  full suite, so `git push` stays a local gate without waiting on CI's job.

### Fixes

- Default Rust-backend `--force-full-build` no longer corrupts `sqlite_master`
  during postprocess (`malformed database schema (<community-name>)`). CLI
  `build` / `update` / `postprocess` no longer hold a second GraphStore while
  the backend writes. Postprocess and query stay on the open store's connection
  (Rust methods instead of a sidecar Python GraphStore). Local embeddings and
  orphan-vector prune run only after that store is closed. WAL connections
  disable `mmap_size` so a checkpoint cannot tear schema pages. `run_postprocess`
  takes the same exclusive `graph_write_lock` as build/update. MCP queries and
  CLI reads (`status`, `wiki`, `detect-changes`), hook enrich, and
  cross-repo search take a shared `graph_read_lock` on the same lock file, so a
  reader waits for a writer and a writer waits for in-flight reads. Nested
  schema migration during a read upgrades the shared flock in one blocking
  `LOCK_EX` (non-blocking upgrade deadlocked against the process's own lock).
  Idle MCP no longer keeps a leftover GraphStore connection open between tool
  calls.
- Graph bootstrap refuses roots that are not a git/svn repository. MCP
  auto-prepare can resolve the root to a non-repo directory (e.g. `$HOME`
  when Cursor spawns the global server outside the project), where an empty
  leftover `.dagayn/graph.db` passed project-root validation and triggered a
  full build of the entire non-repo tree. `assess_graph_sync` now reports
  `vcs`; `needs_mcp_auto_prepare` and the `get_minimal_context` embedding
  trigger refuse `vcs == "none"`. `session_prepare` / `ensure_graph` return
  `reason="not_vcs_repo"` without touching `.dagayn/`, and sync payloads carry
  `vcs` so agents can tell a non-repo root apart from an unbuilt graph.

## 4.8.3 — 2026-08-14

### Fixes

- Local embedding probes refuse a server whose `/v1/models` catalog (or
  embeddings `model` field) does not match the requested preset, instead of
  accepting any 1024-dim listener on the port. `bge-m3` defaults to port 18080
  and `low` to 18081. After a model switch, search stays on the new partition
  even when it has fewer rows (`degraded` / `partial_coverage`) and a completed
  embed run deletes retired provider partitions. See: #76, #71
- Stored flows are disclosed as CALLS reachable sets (`kind: reachable_set`),
  not ordered execution paths. `path` / `steps` / `members` are BFS visit
  order. Tracing caps at depth 15 and 512 members and records `truncated` /
  `truncation_reason` (`max_depth` or `max_nodes`). Schema v16 adds those
  columns. `flow_tool` marks truncated flows `degraded` and adds
  `truncated_flow` missingness. See: #113
- Full build, incremental update, and watch share git's indexable file set:
  tracked plus untracked, excluding gitignored. `.dagaynignore` remains an extra
  restriction. Untracked source is no longer dropped on the next `build`,
  newly-ignored files are pruned on `update`, and watch no longer indexes
  gitignored generated code. See: #83
- Path lookup no longer creates `.dagayn` as a side effect. `db_path_for` /
  `data_dir_for` are read-only; stale registry entries are reported as
  `stale_registry_entry` instead of resurrecting a deleted checkout. Project-root
  validation requires `.git`/`.svn` or a `.dagayn/graph.db`, so an empty
  `.dagayn` leftover cannot grant `repo_root` forever. See: #90, #127
- `repo_slug` identifies a checkout by inode (falling back to a case-folded
  path) so one repository maps to one graph on case-insensitive filesystems.
  Existing `CRG_DATA_DIR` subdirectories from the old path-hash slug are
  adopted. See: #87
- Default MCP prompts, tool docstrings, and next-step hints no longer name
  tools that are missing from the compact surface (`apply_refactor_tool`,
  `embed_graph_tool`, `list_graph_stats_tool`, `find_large_functions_tool`).
  Rename apply is `dagayn tool apply_refactor_tool`; suggestions are filtered
  through the active `--tools` / `CRG_TOOLS` allow-list. See: #107
- Incremental flow tracing recomputes stored criticality after each pass, so
  adding TESTED_BY coverage (a test file that is not on the flow path) lowers
  review ranking instead of leaving a stale score. See: #114

## 4.8.2 — 2026-08-10

### Fixes

- Structure-ready now means HEAD-aligned (`synced` or `dirty_worktree`); MCP
  `auto_prepare` no longer re-runs on every dirty worktree tool call, and
  `ensure_graph` guidance is limited to `empty` / `git_drift`.
- Session/worktree freshness guarantee tests require `status == "ok"` plus
  `is_structure_ready`, cover OpenCode wiring, `worktree sync`, `--from-hook`,
  seed-skip re-enter, serve-seed vs catch-up, and partial→retry.
- Cursor MCP install no longer pins `--repo` / `cwd` to a hardcoded path or an
  unexpanded `${workspaceFolder}` template. Shared `~/.cursor/mcp.json` is
  synced on install, and `dagayn serve` resolves the open workspace from
  Cursor's `WORKSPACE_FOLDER_PATHS` (preferring a folder that already has a
  `.dagayn` graph). Unexpanded IDE placeholders in `--repo` are ignored.

### Documentation

- Document session / worktree / Subagent graph-freshness use cases and the
  structure-ready contract in `docs/SESSION-GRAPH-FRESHNESS.md`, with
  guarantee tests in `tests/test_session_graph_freshness.py`.

## 4.8.1 — 2026-08-10

### Fixes

- Rust-default GraphStore can run manifest bridge postprocess: resolve
  `repo_root` without requiring Python-only helpers, and fall back to a
  short-lived Python SQL store when `_conn` is unavailable.

## 4.8.0 — 2026-08-10

### Features

- Phase 4 CROSS_ARTIFACT analysis integration: impact radius traverses reportable
  bridges with explainable `bridge_transitions`; low-confidence bridges appear as
  missingness/caveats; flow steps mark bridge arrivals distinctly; review and
  architecture guidance recommend `docs_for` / `implementations_of` / bridge
  follow-ups; communities weight `CROSS_ARTIFACT` at 0.6.
- Phase 3 cross-artifact Layer-2 bridges: parse maturin/PyO3 `pyproject.toml`
  and OpenAPI Generator manifests to emit `CROSS_ARTIFACT` edges
  (`builds_artifact`, `generates_code`, `binds_generated_client`) with
  confidence/evidence metadata. Edges appear in normal `edges_by_kind` stats.
- High-confidence Terraform → application-code `CROSS_ARTIFACT` bridges
  (`local-exec`, Lambda/function source paths, `handler` / `entry_point`) with
  postprocess resolution and `query_graph` pattern `bridges_from`.

### Fixes

- Manifest bridge refresh is transactional (discover-then-swap under
  `BEGIN IMMEDIATE`) so a failed rescan leaves prior bridges intact.
- Manifest File upserts no longer overwrite existing parser `file_hash` /
  `mtime_ns` metadata used by incremental skip.
- Manifest-controlled paths reject `..` / out-of-root traversal before any
  filesystem join or read.
- Markdown artifact resolution is scoped to documentation bridges so Terraform
  `handler` / `entry_point` edges are not claimed by the Markdown resolver.

### Improvements

- Restore an optional numpy BLAS matmul path for embedding hybrid search when
  the Python fallback runs (`DAGAYN_EMBEDDING_SEARCH_BACKEND=python` / `auto`).
  Vectors are cached as a process-level `(N, D)` matrix keyed on a WAL-aware DB
  stamp; ranking stays within float tolerance of the pure-Python cosine loop.
  Install with `pip install "dagayn[numpy]"` — numpy remains optional, not a
  hard dependency. `EmbeddingStore` pinning in `hybrid_search` and batched
  `embed_nodes` SELECT/INSERT are documented as shipped.

### Testing

- Add ranking-parity, matrix-cache reuse, EmbeddingStore pin-reuse, and
  numpy-vs-Python microbenchmark coverage for embedding search.
- Extend `tools/embedding_search_benchmark.py` with `--compare-numpy`.

### Performance

- Finish write-side batch upserts (#15): `upsert_edge` uses `UPDATE`/`INSERT`
  `RETURNING id` (no `SELECT id`); community member assignment uses a temp-table
  `UPDATE … FROM` join; cache `repo_root` during path normalization; avoid
  double cache invalidation inside `store_file_batch`; document §4.4 as shipped
  with statement-count regression tests.

### Documentation

- Add `docs/RECIPES.md` with copy-paste flows for watch/`session prepare`/
  `serve`, multi-repo registry → search, and optional embedding providers;
  cross-link from README and related docs.

## 4.7.0 — 2026-08-10

### Features

- Add `dagayn session prepare` to guarantee a usable+synced graph at session
  start and after mid-session checkout / worktree moves. Hooks (Claude /
  Codex / Cursor / OpenCode / Pi / Hermes) call it with a 45s self-budget;
  structure sync is Phase 1, optional local embeddings are Phase 2 when budget
  remains. Cursor re-prepares via `afterShellExecution` (and OpenCode via
  `tool.execute.after`) so HEAD-moving git commands have already landed.
- `ensure_graph_tool` / `get_minimal_context_tool` auto-refresh on empty or
  HEAD/worktree drift and inherit `dagayn serve --local-embedding` instead of
  forcing `none`. Responses expose a compact `sync` status
  (`synced` / `git_drift` / `dirty_worktree` / `empty`).

### Fixes

- Session prepare incremental updates now diff from the graph's stored
  `git_head_sha` (same order as `dagayn worktree sync`), not only `HEAD~1`.
- Hook lock contention that skips structure refresh reports `partial` when the
  graph is still unsynced, instead of `ok`.
- Cursor / OpenCode relocate hooks run after the git command, not before.

### Documentation

- Document session prepare, sync observability, and serve-mode embedding
  inheritance in COMMANDS / USAGE / LLM-OPTIMIZED-REFERENCE.

## 4.6.1 — 2026-08-05

### Fixes

- Worktree graph inheritance now replaces empty schema-only `graph.db` stubs
  (0 nodes) created by `dagayn status` / `GraphStore`, instead of treating
  file existence as a populated graph. Claude Code `SessionStart` also runs
  `dagayn worktree sync --seed-only` before `status`, matching Cursor.
- Clamp stale node line ranges in embedding materialization so
  `_comment_sentences_for_node` cannot `IndexError` when `line_start` points
  past EOF — previously `dagayn update --local-embedding` aborted mid-embed
  and never committed the update that would heal those nodes.

## 4.6.0 — 2026-08-05

### Features

- Add `ensure_graph_tool` to the default MCP surface for safe empty-graph
  bootstrap (`postprocess="minimal"`, `local_embedding="none"`). Ready graphs
  are a no-op unless `force=True`. `get_minimal_context_tool` now recommends
  it when `graph_health` is empty. Full `build_or_update_graph_tool` stays on
  the advanced/maintenance surface.

### Documentation

- Align packaged skills with the default MCP surface: everyday workflows gate
  on `ensure_graph_tool` when `graph_health` is empty; semantic-search and
  cross-repo freshness prefer ensure over advanced-only stats/build tools;
  clarify that traverse/apply/find_large helpers need `--tools all` or
  `dagayn tool`.
- Tighten skill efficiency: explore follows Decision Model instead of always
  opening architecture overview; review-delta/PR refresh only when empty/stale;
  PR deep-dives prefer snippets; reading-markdown defers impact unless needed;
  CLI fallbacks describe default vs advanced tools accurately; update
  `docs/LLM-OPTIMIZED-REFERENCE.md` usage/review sections for ensure +
  `analysis_summary`.
- Add `worktree-sync` and `implement-feature` skills; document worktree
  bootstrap in `install-dagayn`; add a docs-update-after-code-change flow to
  `review-changes` (with pointers from review-delta/PR).

## 4.5.0 — 2026-08-04

### Features

- Support Claude Code and Cursor worktree sessions end to end. New
  `dagayn worktree sync` / `dagayn worktree info` inherit the main checkout's
  `graph.db` into a linked worktree through the SQLite backup API (WAL content
  and embeddings included) and re-parse only the branch diff; inheritance also
  runs automatically at `dagayn serve` startup and before `dagayn update` /
  `dagayn status`. Disable with `DAGAYN_WORKTREE_SEED=0`.
- Add `dagayn hook-repo`, which resolves the repository an agent hook payload
  refers to from `workspace_roots`, `file_path`, or an `EnterWorktree` tool
  response, so hook scripts no longer depend on their working directory.
- `dagayn install` now maintains a managed `.worktreeinclude` block listing the
  gitignored MCP config it wrote, so Claude Code copies it into every worktree
  it creates, and registers `dagayn worktree sync` in `.cursor/worktrees.json`
  so Cursor runs it when creating a worktree for a parallel agent. Installing
  from inside a worktree also configures the main checkout.
- `dagayn worktree sync` copies the main checkout's gitignored MCP config and
  skill files into the worktree (`--no-copy-config` to opt out), which covers
  hosts that create worktrees with plain `git worktree add`.
- Claude Code hooks gained a `PostToolUse` entry matching
  `EnterWorktree|ExitWorktree` that runs `dagayn worktree sync`, and their repo
  resolution now falls back to `CLAUDE_PROJECT_DIR` when the working directory
  is outside the repository. Codex hooks omit the worktree entry.
- OpenCode's user-level plugin now resolves the repository with
  `git rev-parse --show-toplevel`, passes `--repo` on every command, and runs
  `dagayn worktree sync --seed-only` at session start so linked worktrees
  inherit the main checkout's graph.

### Fixes

- Install git hooks into the repository's shared hooks directory resolved via
  `git rev-parse --git-common-dir`, honoring `core.hooksPath`. Previously
  `dagayn install` skipped git hooks entirely inside a worktree, where `.git`
  is a file rather than a directory.
- Cursor hook scripts now resolve the repository from the hook payload and pass
  `--repo` explicitly. User-level Cursor hooks run with their working directory
  set to `~/.cursor`, so the previous cwd-dependent scripts updated the wrong
  repository (or none).
- Cursor hooks now return the JSON each event expects: `sessionStart` reports
  graph status as `additional_context`, and `beforeShellExecution` returns
  `{"permission": "allow"}` with the change analysis attached. `afterFileEdit`
  runs the update detached so the editor never blocks on it, and the hook
  timeouts were raised from 5/5/10 seconds to values a real refresh fits in.
- Cursor hook updates now reuse the local embedding arguments chosen at install
  time, matching the Claude and Codex hooks.
- `dagayn status` no longer warns about branch drift when the graph was built at
  the current commit, and points worktrees at `dagayn worktree sync` instead of
  a full rebuild.

## 4.4.1 — 2026-07-20

### Fixes

- Re-export `_git_branch_info` from the incremental facade so `dagayn status`
  no longer raises `ImportError` when printing branch metadata.
- Warn from `dagayn status` when the working copy git commit or SVN
  path/revision has drifted from the graph build metadata.
- Drop removed in-process `local:` embedding specs from the material-model
  benchmark and require OpenAI-compatible sidecar URLs instead.
- Widen Cursor `beforeShellExecution` matching so path-qualified `git`
  binaries (for example nix-profile absolute paths) still trigger the
  pre-commit graph refresh, and replace existing `crg-*` hook entries on
  reinstall so matcher updates take effect.

### Documentation

- Document Cursor install hooks and VCS drift warnings for `dagayn status`.

## 4.4.0 — 2026-07-18

### Improvements

- Update Terraform parsing to `tree-sitter-terraform` v0.2.0 and recognize 17
  additional block types.
- Expand the VS Code extension with multi-root workspace support, module
  dependency visualization, saved custom queries, node documentation panels,
  blast-radius snapshot save/compare, editor context-menu commands, and
  auto-update failure notifications.
- Align skill examples with current tool signatures and remove the obsolete
  `wiki-research` skill.

### Fixes

- Pass `--flash-attn on` to managed `llama-server` sidecars so current
  llama.cpp CLI parsers no longer treat the next flag as the Flash Attention
  mode and exit before MCP/`serve` becomes ready. Startup failures now include
  a stderr tail for diagnosis.
- Satisfy ty 0.0.61 sort-key and redundant-cast diagnostics in analysis,
  local-embedding, and state-type helpers.
- Include the VS Code test stub package in git so extension CI can run
  reliably.

### Documentation

- Update vector-search backend documentation and remove the outdated system
  BLAS requirement from the README.
- Move the architecture analysis report from the repository root into `docs/`.

## 4.3.0 — 2026-06-21

### Improvements

- Add a Rust native embedding-search backend with process-level vector caching,
  prewarm support, macOS Accelerate matrix-vector search, and SIMD dot-product
  fallbacks; remove the Python-side numpy search path while keeping a
  pure-Python fallback mode through `DAGAYN_EMBEDDING_SEARCH_BACKEND` for A/B
  testing.
- Parallelize embedding search for large matrices.
- Add typed Rust DTOs and enum-backed parser contracts for stricter
  cross-language interfaces.
- Add Pydantic boundary DTOs for state transitions, dispatchers, architecture
  analysis, refactor outputs, and answerability guidance.
- Split the embeddings, incremental build, and review tool modules into
  focused submodules.

### Fixes

- Drop the system BLAS dependency in the native embedding search backend.
- Keep the Rust embedding backend clippy-clean.
- Align typed contracts with CI checks.

### Testing

- Add an embedding search micro-benchmark.
- Expand test coverage for impact analysis scoring.
- Update markdown resolution contract assertions.

## 4.2.8 — 2026-06-15

### Improvements

- Recognize public API coverage for bare helper contracts across Rust, Python,
  JavaScript, TypeScript, Go, Java, C#, C++, PHP, Ruby, and Kotlin test styles,
  reducing false quality-check gaps when helpers are intentionally private.
- Share edge-record normalization between graph storage paths so rich edge
  metadata such as line ranges, confidence, scope, and evidence is preserved
  consistently.

### Fixes

- Keep the Rust graph backend clippy-clean by deriving the default confidence
  tier directly on the enum variant.

## 4.2.6 — 2026-06-09

### Fixes

- Keep `get_minimal_context_tool` lightweight by avoiding automatic change
  impact analysis unless callers explicitly provide `changed_files`, preventing
  CLI and MCP entry-point hangs in repositories with costly coverage inference.

## 4.2.5 — 2026-06-08

### Fixes

- Include local embedding sidecar arguments in generated AI-tool update hooks
  when installing with `--mode local-embedding` or
  `--mode local-embedding-llama`, so hook-triggered graph refreshes keep local
  vectors current.
- Preserve sidecar port, binary, and startup-timeout options consistently
  between the MCP `serve` command and generated update hooks.

## 4.2.4 — 2026-06-08

### Improvements

- Run bare BGE-M3 local embeddings through a managed `llama-server` GGUF Q8
  sidecar instead of in-process sentence-transformers, keeping Apple Metal
  execution outside the dagayn Python process.
- Document the BGE-M3 Q8 sidecar benchmark, including search-quality parity
  with sentence-transformers BGE-M3 on the smaller quality set and much lower
  observed resident memory than the PyTorch MPS path.
- Update GitHub workflow actions for the Node 24 runtime.

### Fixes

- Keep generated hook updates graph-only so MCP `serve --local-embedding`
  settings are not reused by edit-time hook refreshes.
- Default explicit in-process local sentence-transformers loading to CPU unless
  `CRG_LOCAL_EMBEDDING_DEVICE` is set.

## 4.2.3 — 2026-06-08

### Improvements

- Clarify installed skill guidance so agents consistently use semantic search
  for start-node discovery, relationship queries for specific graph evidence,
  workflow dispatchers for review/architecture/refactor decisions, and raw
  graph traversal only for bounded neighborhood follow-ups.

## 4.2.2 — 2026-05-31

### Improvements

- Include unstaged, staged, and untracked files in change review and
  incremental graph update detection.
- Add `change_file_sources` buckets so base-ref changes remain distinguishable
  from local worktree, staged, unstaged, and untracked files.
- Annotate changed nodes and relevant edges with `change_status` and summarize
  existing vs added graph entities in `change_entity_summary`.

## 4.2.1 — 2026-05-27

### Fixes

- Update the `wiki-research` skill for the simplified static `dagayn visualize`
  export surface and explicitly avoid the removed interactive HTML/webserver
  workflow.

## 4.2.0 — 2026-05-27

### Changes

- Remove the interactive HTML graph visualization from `dagayn visualize`.
  Static exports now require an explicit `--format` value: `graphml`,
  `mermaid-c4`, `svg`, `cypher`, or `obsidian`.
- Remove `dagayn visualize --serve` and the local webserver path.

### Improvements

- Reduce architecture-analysis noise by separating code/docs/test scopes,
  classifying low-signal knowledge-gap findings, and adding edge-shape metrics
  for single-file communities.
- Add compact `_runtime` metadata to tool responses so agents can distinguish
  long-lived MCP processes from direct CLI runs.
- Add Rust parser type-reference edges for local type identifiers to reduce
  false isolated-node leads.

### Fixes

- Keep the Rust parser clippy-clean after the new type-reference traversal.

## 4.1.4 — 2026-05-25

### Fixes

- Calibrate `refactor_tool(mode="dead_code")` confidence so public APIs,
  ambiguous symbol names, and candidates without readable source are reported
  as lower-confidence review leads instead of ordinary deletion candidates.

## 4.1.3 — 2026-05-25

### Fixes

- Classify Rust `#[test]` functions as `Test` nodes even when their names do
  not start with `test`, reducing false-positive graph quality test gaps.

## 4.1.2 — 2026-05-25

### Fixes

- Apply repository-wide ruff formatting so main CI passes after the 4.1.x
  remediation releases.

## 4.1.1 — 2026-05-25

### Fixes

- Restore strict CI compatibility for ruff, ty, and Rust clippy after the
  interface remediation release.

## 4.1.0 — 2026-05-25

### Improvements

- Align the MCP and CLI fallback tool surface around public `*_tool` names,
  bounded `top_n` output, and shared `detail_level` arguments.
- Make fallback responses easier to act on by adding result counts, confidence,
  zero-result reasons, and next-action hints where they were missing.
- Keep installed Codex skills and MCP interface guidance in sync with the
  remediated tool contracts.

### Fixes

- Preserve calibrated documentation evidence types in minimal outputs.
- Normalize zero-result and dispatcher error contracts across graph query,
  flow, review, architecture, semantic search, and refactor tools.
- Keep `dagayn.__version__` aligned with the released package version.

## 4.0.6 — 2026-05-24

### Improvements

- Ship numpy as a standard dependency so embedding search uses the vectorized
  cosine-similarity path in default installs.
- Prefer persisted hub and bridge centrality scores even when callers pass a
  graph snapshot, avoiding runtime betweenness centrality during suggested
  question generation.
- Sample large-graph Rust centrality sources by stable hash instead of sorted
  prefix so approximate bridge scoring does not miss connected regions.
- Generate suggested architecture questions as a native Rust analysis unit when
  using the Rust graph store, avoiding Python snapshot materialization.

### Fixes

- Keep `dagayn serve` importable on Python 3.14.0b4 by applying compatibility
  shims before importing FastMCP dependencies that still reference older
  CPython typing and `collections.abc` surfaces.

## 4.0.5 — 2026-05-24

### Fixes

- Revert the managed local embedding runtime to a single `llama-server`
  implementation and remove the slow `mlx-openai-server` path.
- Update local embedding documentation and tests so the `low` preset consistently
  uses the llama.cpp GGUF model on every platform.

## 3.2.0 — 2026-05-17

### Features

- Add `dagayn build --force` to force a full graph rebuild even when an existing
  graph database is present.
- Teach installed skills how to follow explicit Markdown ↔ code documentation
  traceability through `dagayn:` directives, `docs_for`, and
  `implementations_of`.

### Improvements

- Clarify Markdown `CROSS_ARTIFACT` resolution semantics for unresolved or
  ambiguous code-span references.
- Extend review, exploration, debug, refactor, and architecture skills so
  documentation bridge edges are considered when they affect the task.

### Fixes

- Pass install-time skill rendering context explicitly so `ty` can type-check
  the `dagayn install` command path.

## 3.1.0 — 2026-05-17

### Features

- Add install-time skill rendering so search and graph-build skills adapt to
  the selected embedding mode (`fts`, `local`, or `remote`).
- Add operational skills for dagayn installation, semantic search, wiki
  research, and cross-repository workflows.
- Make `build_or_update_graph_tool()` inherit the local embedding preset from
  `dagayn serve --local-embedding low|high`; explicit
  `local_embedding="none"` remains available for deliberate FTS-only refreshes.
- Prefer the globally installed `dagayn serve` command in generated MCP config
  when `dagayn` is available on `PATH`, matching `uv tool install` workflows.

### Improvements

- Align packaged skills with the 3.x MCP dispatcher surface and remove stale
  split-tool names.
- Refresh graph exploration, debugging, review, refactor, and Markdown authoring
  skills with token-efficient graph-first workflows.
- Make Markdown symbol checks robust under hybrid semantic search by requiring
  exact symbol matches for `CROSS_ARTIFACT` resolution.

### Fixes

- Avoid duplicate AGENTS/CLAUDE instruction injection when an existing dagayn
  heading is present without a marker.
- Ensure `dagayn install` rewrites managed skill content while preserving
  unrelated user-created skills.

## 2.3.5 — 2026-04-30

### Performance

- `get_impact_radius_sql`: replace `get_edges_among` with temp table JOIN (B5)
- `find_dependents`: batch frontier lookups, reducing N+1 to 3 queries/hop
- `store_file_batch`: batch processing in `full_build` and `incremental_update` (A2)
- Add mtime-based incremental skip (migration v11, `mtime_ns` column)
- `find_dead_code`: batch-preload edges, reducing O(N) SQL to O(1) lookups
- Batch postprocessing `CROSS_ARTIFACT` ref resolution (N+1 → 3 queries)
- `get_local_subgraph`: batch DFS traversal via recursive CTE
- Vectorize embedding search with numpy BLAS and process-level matrix cache
- Batch flow INSERT/DELETE with `executemany` and `IN (?,…)`
- Batch community INSERT/UPDATE in `_clear_and_store_communities`
- migration v10: add `idx_nodes_parent_name(parent_name, name)` index
- Cache `parse_diff_ranges` and `embed_query` results with `lru_cache`
- Pin `EmbeddingStore` per-process; batch `embed_nodes` hash lookups
- Bulk-insert nodes/edges with `executemany`, use `RETURNING id` in upsert
- SQLite PRAGMA tuning, parser worker singleton, token-estimate fix

## 2.3.4 — 2026-04-30

### Features

- Add `writing/reading-markdown-document` skills with global install support (#1)

### Performance

- Pin sqlite connection, batch node lookups, add profiler scaffold (#2)

### Refactoring

- Split megafiles into focused packages (Phase 1–2)
- Split `parser/core.py` (3476 → 1269 lines) into focused modules
- Split `extension.ts::registerCommands` into feature modules (Phase 3-1)
- Extract `graphWebview` HTML/CSS to static assets (Phase 3-2)
- Add Protocol classes to lift SAP abstractness (Phase 4)

### Fixes

- Resolve lint and type-check errors
- Update notebook parity snapshot
- Apply ruff-format to `parser/_protocol.py`

## 2.3.3 — 2026-04-27

### Features

- Expand bridge detection to 13 languages (file_io, subprocess, FFI)
- Add cohesion filter and file-I/O bridge detection
- Add unified multi-language CROSS_LANGUAGE edge extractor
- Add Markdown → code `CROSS_ARTIFACT` edges (doc-to-symbol bridge)
- Add SAP metrics and unify SDP/SAP edge set
- Add ADP/SDP analysis tools; fix Python relative import resolution
- Add Mermaid C4 export
- Add markdown documentation policy to `dagayn install`
- Add markdown parser support

### Refactoring

- Extract tree-sitter language extractors and walkers (A.3+A.4)
- Extract bespoke-parser languages into `dagayn/parser/` (A.2)
- Split parser shared infra into dispatch/grammars/bridges modules
- Split `cli.py` into `cli/` package with command modules
- Split `graph.py` into `graph/` package (types/helpers/core)
- Convert `dagayn/parser.py` monolith to `parser/` package
- Rename `CROSS_LANGUAGE` → `CROSS_ARTIFACT` edge kind
- Migrate vendored grammar sources to dynamic loading system
- Rename VS Code extension and rebrand to `dagayn`

### Fixes

- Exclude dev-environment artifacts from vendor grammar sources
- Filter short plain-word identifiers from markdown code-span candidates
- Resolve grammar archive structure and binding injection
- Enforce HTTPS-only URL before `urlopen` in `vendor_grammars`
- Skip grammar vendor during editable install
- Remove dead code modules; fix test expectation
- Resolve architecture lint and typing

### Tests

- Add cross-package Java integration tests for SAP/SDP
- Add comprehensive test coverage for analysis and exports
- Add multi-file language tests

## 0.1.0 — Initial dagayn fork

- Forked from `code-review-graph` by Tirth Kanani and established `dagayn` as an independent project
- First-class Terraform parsing support
- Markdown structure parsing with directive-based dependency extraction
- Graph registration paths unified to repository-root-relative
- Package name, CLI, and storage directory unified under `dagayn`
- See [NOTICE](NOTICE) for upstream attribution

## 2.9.0 — 2026-05-13

### Features

- Embedding progress bar: `dagayn update --local-embedding` now shows a live
  progress bar (nodes/s, ETA) on stderr when running interactively
- `CRG_OPENAI_TIMEOUT` env var: controls the per-call HTTP timeout for the
  OpenAI-compatible embedding provider (default 120 s); local embedding runs
  automatically inherit `--local-embedding-timeout` so large/slow models no
  longer hit the 120 s ceiling
- `dagayn serve --local-embedding {low,high}`: managed llama-server sidecar
  for semantic search during MCP sessions
- `dagayn install --local-embedding {low,high}`: bakes the llama-server flags
  into the generated MCP server entry so the model is available after restart
- FTS search-quality benchmark (`dagayn/eval/`) with 12 labelled queries;
  results published in `docs/LOCAL-EMBEDDINGS.md` and `README.md`

### Fixes

- Local embedding runs now set `CRG_OPENAI_TIMEOUT` to the startup-timeout
  value, preventing spurious HTTP timeouts on slow/large GGUF models

## 2.9.1 — 2026-05-14

### Improvements

- MCP semantic search now inherits the embedding mode selected for `dagayn serve`:
  `--local-embedding {low,high}` defaults search to the managed local
  OpenAI-compatible sidecar, and `--remote-embedding {openai,google,minimax}`
  defaults search to the selected remote provider.
- `hybrid_search` test deboost: nodes detected as test code (`is_test=True`)
  are multiplied by 0.6× during boosting so source ranks above the tests
  that exercise it. Tests remain visible (deboost, not filter). Each
  result dict now exposes `is_test: bool`.
- RRF default `k` changed from 60 to 10 so the `score` field spreads over
  ~0.05–0.2 instead of being compressed into 0.015–0.016 with L2-normalised
  embeddings. Item order is preserved.
- Natural-language identifier extraction: `hybrid_search` now extracts
  snake_case / camelCase / PascalCase tokens from the query and fires an
  extra FTS arm per identifier, so phrases like
  "tests for embed_graph" still match the `embed_graph` symbol directly.

## 2.10.0 — 2026-05-14

### Features

- `dagayn install --mode {fts,local,remote}` makes the three install
  patterns first-class CLI choices.  Each mode has paired sub-options:
  `--preset {low,high}` for local, `--provider {openai,google,minimax}`
  for remote.  When `--mode` is omitted on a TTY, the user is prompted
  interactively; under `-y` or a non-TTY stdin the install fails fast
  with a helpful error.  Legacy `--local-embedding low|high` continues
  to work as a shortcut for `--mode local --preset $X`.
- `--mode remote` prints the provider-specific environment variables the
  user needs to set in the shell that launches their AI coding tool
  (e.g. `CRG_OPENAI_API_KEY`, `CRG_OPENAI_BASE_URL`, `CRG_OPENAI_MODEL`
  for `openai`).  The MCP server inherits those at launch time.

## 2.10.1 — 2026-05-15

### Fixes

- Re-running `dagayn install` with local or remote embedding enabled now updates
  existing MCP config entries and dagayn-generated hooks instead of leaving the
  old `dagayn serve` / `dagayn update` arguments in place.
- Re-running `dagayn install` now refreshes dagayn-managed skill Markdown
  placements from the packaged source, replacing stale skill content while
  preserving unrelated user-created skills.
- Align `TESTED_BY` scoring semantics across Python and Rust analysis paths:
  production symbols now consistently point to the test symbols that cover
  them, matching parser output and schema documentation.
- Make sampled bridge-centrality scoring deterministic.
- Avoid reporting SAP violations for scopes with no eligible concrete or
  abstract types.

## 2.11.0 — 2026-05-16

### Features

- Add explicit documentation bridge directives for Markdown, Python comments,
  and Terraform comments. Directives emit `CROSS_ARTIFACT` edges for
  `implemented_by`, `implements_contract`, `explained_by`, `has_runbook`,
  `problem_described_by`, `discussed_by`, `discusses_artifact`, and
  `raises_issue_for` relationships.
- Add `query_graph` patterns `docs_for` and `implementations_of` so agents can
  traverse documentation relationships in either direction without storing
  duplicate inverse edges.

### Documentation

- Document the documentation bridge role model and authoring-site policy in the
  cross-artifact edge specification.
