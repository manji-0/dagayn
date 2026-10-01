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
(`dagayn/scip_overlay.py` runs the indexers; `GraphStore::apply_scip_overlay`
in `crates/dagayn-graph/src/postprocess/scip_overlay.rs` applies an index).
The overlay runs on a full `build`, **before** post-processing:

1. For each language with an available indexer, run it on the working tree.
2. For every `CALLS` edge in an indexed file whose content hash still
   matches the graph's, find the SCIP reference at the called name's
   position (the join of the proof of concept).
3. Settle the edge from SCIP's answer:
   - a definition in the repository → the innermost node holding it whose
     name is the symbol's, `HIGH`, `resolved_by: "scip"`; an edge at a node
     holding the definition (a class for its constructor) is confirmed;
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

When SCIP and dagayn disagree, SCIP wins for Rust and TypeScript. For
Python (not yet wired), SCIP would only fill edges dagayn left `LOW` and
confirm edges where both agree; it would never override a dagayn binding,
because pyright's `Unknown` is common and the Rust-extension bridge is
invisible to it.

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

The overlay is opt-in at first (`dagayn build --scip`, or a config key),
and per language: a missing indexer, a failed run, or a project without the
prerequisites (no `node_modules`, no venv) skips that language with a
warning and keeps today's result. Indexers are run, not bundled.

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

- **Other languages.** Indexers exist for Java/Kotlin/Scala (`scip-java`),
  Go (`scip-go`), C/C++ (`scip-clang`), C# (`scip-dotnet`), Ruby
  (`scip-ruby`). Each needs the comparison above on a representative
  repository before it joins the overlay.
- **Tier.** `HIGH` for overlay edges matches "evidence written in the code"
  only loosely: it is evidence a compiler derived. A distinct marker
  (`resolved_by: "scip"`) is proposed either way so consumers can tell.
- **Edges Tree-sitter never extracts.** SCIP also sees calls inside macros
  and generated code. The first version only rewrites existing edges; adding
  new ones changes edge counts that tools and tests rely on.
- **Cost.** 26 s and 2.1 GB for Rust on this repository is fine for an
  explicit `build`, not for a hook. Larger workspaces need measuring.
- **Python environment.** Discovery needs the venv's distributions passed
  explicitly; the overlay should generate the `--environment` file from the
  interpreter dagayn runs under.
