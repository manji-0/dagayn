# SCIP-backed call resolution

<!-- constrained-by ../CALL-RESOLUTION.md#post-processing-passes -->
<!-- constrained-by ../ARCHITECTURE.md#post-processing -->

## Goal

Take call targets from a compiler-grade indexer where one is available, and
stop growing the type-inference layer that approximates it on top of
Tree-sitter.

Tree-sitter stays: it gives every node, `CONTAINS`, import, Markdown, and
Terraform edge for 30 languages, per file, without a build environment, and
the hook-driven incremental update depends on that. What this plan replaces
is the layer above it that guesses receiver types: receiver tracking in the
extractors, return-type tables, standard-method lists, observed-method
inference, and Rust's cross-file module resolution. Its rules are written by
hand from what a compiler already knows, and each fix tends to open the next
misattribution (`Value::is_null` taken for `std`'s, `map.get(key)` for
`vscode`'s).

## Measured comparison

A proof of concept indexed the v7.0.0 sources (`53c880e`) with
[SCIP](https://github.com/sourcegraph/scip) indexers and compared every
`CALLS` edge of the 7.0.0-plus-unreleased graph with them. The join is by
source position: the column of the called name on the edge's line (or the
next eight lines, for a chain that dagayn records at the expression's first
line) against the SCIP reference occurrence covering it. Joining by name
alone misses import aliases (`file_hash as compute_file_hash`).

| Language | Indexer | Time | Peak memory | Needs |
| -------- | ------- | ---- | ----------- | ----- |
| Rust | `rust-analyzer scip` 1.98.1 | 26 s | 2.1 GB | Cargo workspace; crates outside it (test fixtures) are not indexed |
| Python | `scip-python` (pyright) | 11 s | — | `--environment` JSON from the venv (a uv venv has no `pip`, so discovery falls back to the system Python and drops every third-party package) |
| TypeScript | `scip-typescript` | 3 s | — | `node_modules` installed |

The comparison scripts and indices were kept outside the repository; the
numbers below are not reproducible from it without redoing the proof of
concept as described.

Agreement, by what dagayn concluded (`same` = SCIP binds the same node or
reaches a package too):

| dagayn edge | Rust | Python | TypeScript |
| ----------- | ---- | ------ | ---------- |
| Node, `MEDIUM` | 788 / 789 same | 444 / 2,545 same; 2,049 SCIP cannot type | 73 / 89 same; 13 SCIP shows wrong |
| Package, `MEDIUM` | 24,448 / 24,946 same; 217 SCIP shows wrong | ~11,000 / 13,452 same; 2,026 SCIP cannot type | 336 / 452 same; 116 ambiguous on the line |
| `LOW` that SCIP resolves | 938 / 951 | 424 / 800 | 340 / 473 |

What the numbers say:

- **Rust**: SCIP subsumes the inference layer. Of the 217 disagreements,
  215 are one bug class: the return-type rule "a method on what a package
  call returned is that package's" applied through `RefCell::borrow()` /
  `borrow_mut()`, whose `Ref<T>` derefs to a repository type
  (`bindings.borrow().snapshot()` is `MemberCallBindings.snapshot`, recorded
  as `std` `RefCell::borrow()::snapshot`); the other 2 come from the
  standard-method list and observed-method inference. The 1,010 `EXTRACTED`
  calls SCIP reports as locals are closures dagayn models as nested
  functions, not errors.
- **TypeScript**: SCIP subsumes it too, at 3 s. Of the 13 disagreements, the
  6 examined are all dagayn binding a method to the caller's own class
  (`this.db.close()` is better-sqlite3's, a `Map` field's `clear()` taken
  for the class's `clear`).
- **Python**: of the 2,049 node edges SCIP cannot type, 1,482 are calls into
  the Rust extension (through `#[pyclass]` / `#[pymethods]`), which only
  dagayn sees: SCIP does not cross languages. The other 567, with the 2,026
  package edges, make 2,593 calls dagayn's inference resolves and SCIP
  leaves untyped, because pyright leaves unannotated parameters `Unknown`.
  scip-python also records builtins functions (`isinstance`) as `local`
  symbols.

## Design

<!-- derived-from #measured-comparison -->

### Where SCIP edges enter

Implemented for Rust and TypeScript as `dagayn build --scip`
(`dagayn/scip_overlay.py` runs the indexers; `GraphStore::apply_scip_overlays`
in `crates/dagayn-graph/src/postprocess/scip_overlay.rs` applies a language's
indexes with their definitions pooled).
The TypeScript indexer is dagayn's own since the scip-typescript measurements
above: `crates/dagayn-scip-typescript` indexes a project with the tsrs
checker and writes scip-typescript's symbol grammar, so the overlay reads it
unchanged; `dagayn.scip_typescript` runs it in a child process (a checker
panic ends only the indexer). A reference whose symbol has several
declarations (a function/namespace merge) is emitted once per declaration
symbol, so the overlay counts the call `ambiguous` and leaves it rather than
choosing. The overlay runs on a full `build`, **before** post-processing:

1. For each language with an available indexer, run it on the working tree.
2. For every `CALLS` edge in an indexed file whose content hash still
   matches the graph's, find the SCIP reference at the called name's
   position (the join of the proof of concept).
3. Settle the edge from SCIP's answer:
   - a definition in the repository → the innermost node holding it whose
     name is the symbol's, `HIGH`, `resolved_by: "scip"`; an edge at a node
     holding the definition (a class for its constructor) is confirmed. A
     symbol rust-analyzer gives two definitions (a crate-root function of
     `src/lib.rs` and one of `tests/*.rs`, two nested functions of one
     name) is the one sharing the most scope with the caller; a tie is
     ambiguous;
   - an external symbol → its package (`std`/`core`/`alloc` fold to `std`;
     `@types/x` to `x`; `typescript` to `globalThis`), `external_symbol` from
     the SCIP descriptors, `HIGH`, `resolved_by: "scip"`; an edge already
     external keeps its package name;
   - a local, a parameter, or a type parameter (scip-typescript gives a
     parameter a global symbol defined inside its function) → an unresolved
     edge records `callee_local: true`;
   - no reference or several → leave the edge for the passes.

Running before the passes means `TESTED_BY` sync, flows, and communities see
the overlay's targets, and the passes, which only take edges whose target is
neither a node nor a package, leave them alone. Observed-method inference
does not learn from them: on the v7.0.0 sources it otherwise took
`child_process.exec()` for better-sqlite3's.

On the v7.0.0 sources, the overlay leaves 1,126 calls unresolved (`LOW`),
against 2,597 without it: Rust 1,167 → 14, TypeScript 483 → 194; the
whole `build --scip` takes about 15 s.

When SCIP and dagayn disagree, SCIP wins for Rust, TypeScript, and Python
(Python since dagayn's own indexer, below). Every other indexer (Go,
Java/Kotlin, C/C++, C#, Ruby, Dart, PHP, R) only fills: it keeps what the
extractor resolved and settles the rest before the inference passes. With
Python filling through scip-python, the v7.0.0 sources dropped to 926
unresolved calls (Python 807 → 607), and the 1,482 Python calls into the Rust
extension were kept. A project whose indexer is not installed gets an install
hint and keeps dagayn's own resolution.

### The Python indexer

<!-- derived-from #where-scip-edges-enter -->

`crates/dagayn-scip-python` replaces scip-python. It resolves every name of
a project file with goto-definition from the ty checker (the `ty_*` crates of
the ruff repository) and writes scip-python's symbol grammar, so the overlay
reads it as it read scip-python's. The ty and ruff crates are pinned to one
ruff revision in `[workspace.dependencies]`, the same one the Python
extractor's `ruff_python_parser` comes from, so one copy of ruff is built.
`dagayn.scip_python` runs it as `python -P -m dagayn.scip_python index
--output OUT [--python ENV]` in a child process. The overlay passes the
project's environment explicitly: the `.venv` holding a `pyvenv.cfg` in the
project or an ancestor up to the repository root, else `VIRTUAL_ENV`.
Without either, the project is indexed with a hint; ty then looks for an
environment itself and falls back to the site-packages of the interpreter
that runs dagayn. The shim drops dagayn's own directory from `PYTHONPATH`:
`child_env` adds it so the child can import dagayn, and ty would otherwise
search it before the project.

Where the index differs from scip-python's:

- The project's package is the directory name with version `.`, not the
  `pyproject.toml` name and version. An installed package is its top-level
  module with version `.`, not its distribution name and version. The
  overlay reads only the module of a symbol, so neither changes an edge.
- Standard-library modules are named as typeshed names them (`_io`,
  `_hashlib`) where scip-python names the public module (`io`, `hashlib`).
- A name with several definitions (a `try`/`except ImportError` import, a
  name bound in two branches) gets one occurrence per definition at the
  same range, so the overlay counts the call `ambiguous` and leaves it.
  scip-python picks one.
- Import statements carry the `Import` role; scip-python marks them
  `ReadAccess`.
- A name ty cannot resolve (an import of a module that is not installed)
  gets no occurrence.

Measured with `dagayn build --scip` on this repository (`e3ba931f`) and on
httpx (`b5addb6`), with the Rust and TypeScript indexers disabled
(`DAGAYN_SCIP_RUST=/usr/bin/false`, `DAGAYN_SCIP_TYPESCRIPT=/usr/bin/false`),
on an Apple M5 Max; scip-python is 0.6.8, and time and peak memory are
`/usr/bin/time -l` of the whole build:

| | dagayn: scip-python | dagayn: ty | httpx: scip-python | httpx: ty |
| --- | --- | --- | --- | --- |
| `CALLS` settled (`resolved_by: "scip"`) | 18,642 | 21,021 | 3,238 | 3,843 |
| `HIGH` `CALLS` (4,835 and 1,064 without `--scip`) | 13,638 | 15,974 | 2,749 | 3,341 |
| Build time | 16.2 s | 6.2 s | 3.7 s | 0.8 s |
| Peak memory | 1.17 GB | 646 MB | 430 MB | 228 MB |

The ty indexer alone takes 0.4 to 0.8 s and about 500 MB on this repository,
against about 9.4 s and 1 GB for scip-python. The target differs between
the two graphs on 212 edges here and 658 on httpx (593 of them a class
re-exported with `from ._models import *`). Of 25 of those checked by hand,
ty was right on 22 (the httpx re-exports, pydantic `model_*` calls,
`concurrent.futures.Executor.map` that scip-python took for `builtins.map`).
On the other 3, ty resolved nothing where scip-python did:
`Path.cwd().resolve()` in `task_queue.py`, `executemany` on the result of an
unannotated function, and a method on an `async with ... as` target.

On a project whose pydantic is installed only in its `.venv`, the ty index
resolves 60 of 60 pydantic references (27 of 27 calls); scip-python, run as
the overlay ran it, resolved 35 (12 calls). In `dagayn build --scip` of that
project, scip-python settles no call into pydantic and ty settles 37.

Taking the indexer's answer over the extractor's moves 13 more edges than
filling does (4 here, 9 on httpx, such as `httpcore.Request` taken for
httpx's own, and `time.sleep`); all 13 were checked and are right. The
calls into the Rust extension are not among them.

### Incremental updates

SCIP indexes a whole project, so it does not fit the per-file hook update.
The two layers split by time:

- `build --scip`: Tree-sitter extraction, the overlay, then the inference
  passes for what the overlay left.
- `update` (hook or daemon): a changed file is re-extracted by Tree-sitter
  and its edges come from the inference passes again, as today. Its hash no
  longer matches the index, so the overlay leaves it alone until the next
  `build`.

Edges written by the overlay live in the same `edges` table; re-extracting a
file replaces them like any other edge of that file. No separate store is
needed.

### Availability

The overlay is opt-in (`dagayn build --scip`) and per project: a project
whose indexer is not installed gets a `hint:` with the install command, and a
failed run or a project without its prerequisites (no
`compile_commands.json`) is a warning; both keep today's result. A
TypeScript project without `node_modules` (in the project or up to the
repository root) is still indexed, with a hint: its own symbols resolve and
calls into packages stay unsettled. scip-java runs the project's build with
`clean`, so it also needs `DAGAYN_SCIP_ALLOW_BUILD=1`. TypeScript's and
Python's indexers ship with dagayn; the others are run, not bundled. The per-language table is
in [COMMANDS.md](../COMMANDS.md#scip-call-resolution).

### Freezing the inference layer

The overlay makes parts of the inference layer redundant per language
**at build time**. Between builds, a hook-updated file is resolved by these
passes alone, so they cannot simply be deleted: without them a changed Rust
file's calls stay `LOW` until the next `build`. Frozen means no new table
entries or rules; bugs that SCIP exposes (the `RefCell::borrow()` chains)
are still fixed. Whether to delete them later is an open question below.

| Pass or table | Rust | TypeScript | Python |
| ------------- | ---- | ---------- | ------ |
| Cross-file module resolution (`rust_lang/` modules, `use` tracking) | freeze | — | — |
| Receiver tracking in the extractor (`receiver_from`, bindings) | freeze | freeze | keep |
| Standard-method lists (`RUST_STD_METHODS`, `JAVASCRIPT_BUILTIN_METHODS`) | freeze | freeze | keep |
| Return-type tables (`returned.rs`) | freeze | freeze | keep |
| Observed-method inference | freeze | freeze | keep |
| PyO3 / native-binding bridges | keep | keep | keep |
| Languages without an indexer used here | keep | keep | keep |

Edges a frozen pass produced are recognizable by `confidence_tier` `MEDIUM`
and `inferred_from` (`observed_method`, `return_table`), so the overlay's
effect on them can be measured on any graph before deletion.

## Open questions

<!-- derived-from #design -->

- **Freeze or delete.** Deleting the frozen Rust and TypeScript passes
  removes the code, but a hook update then leaves the changed files' calls
  unresolved until the next `build`: on this repository about 132 `MEDIUM`
  calls per Rust file (27,602 over 209 files). The recommendation is to
  freeze and keep them as the update-time fallback, and to revisit deletion
  only if the overlay becomes cheap enough to run per update (an indexer
  that indexes single files, or a long-running rust-analyzer).

- **Other languages.** Go, Java/Kotlin, C/C++, C#, Ruby, Dart, PHP, and R
  are wired, but only to fill; Go was run on a small module only, the others
  not at all. Each needs the comparison above on a representative
  repository before it can be authoritative, and its package naming mapped
  (only Go import paths and Python modules are) before it names packages.
  Scala support in the current scip-java is unverified.
- **Tier.** `HIGH` for overlay edges matches "evidence written in the code"
  only loosely: it is evidence a compiler derived. A distinct marker
  (`resolved_by: "scip"`) is proposed either way so consumers can tell.
- **Edges Tree-sitter never extracts.** SCIP also sees calls inside macros
  and generated code. The first version only rewrites existing edges; adding
  new ones changes edge counts that tools and tests rely on.
- **Cost.** 26 s and 2.1 GB for Rust on this repository is fine for an
  explicit `build`, not for a hook. Larger workspaces need measuring.
- **Python environment.** The overlay passes the project's `.venv` or the
  active virtualenv to the ty indexer. Without either, ty falls back to the
  site-packages of dagayn's own interpreter, which can resolve a call into a
  package the project does not install.
- **Python names declared `Any`.** `dagayn/server/main.py` binds
  `FastMCP: Any` to fastmcp's class in a `try` and to `_FallbackFastMCP` in
  its `except`. ty resolves methods of `FastMCP(...)` to the fallback
  class's and confirms, at `HIGH`, the edges the extractor drew there. On this
  repository test reach with `--scip` drops from 0.638 recall
  (scip-python) to 0.605, through 5 methods of `_FallbackFastMCP` that
  never run.
