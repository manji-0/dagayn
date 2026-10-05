# oxc parser evaluation for JavaScript and TypeScript

## Question

Should the JavaScript / TypeScript extractor in `crates/dagayn-parser` parse
with `oxc_parser` (oxc 0.146) instead of tree-sitter, producing identical
graph output?

Verdict: **no, not now, and not behind a flag either.** oxc gives up on any
file with a syntax error, so it cannot reproduce the nodes the graph gets
today for broken, half-edited or Flow files. The speed-up is real but only
matters on very large JS/TS trees, and the port would mean running a second
copy of a ~9,400-line extractor alongside the first.

## What the current extractor is

The port would have to cover all of this:

- Entry: `js_like.rs::parse_javascript_like_interned` runs eight whole-tree
  collection passes (import map, defined names, type names, member and
  namespace paths, local exports, class table, type paths, bound names), then
  one emitting walk and these post-passes: collapse duplicates, merge type
  references, mark external edges, resolve same-file call targets, and add
  `TESTED_BY` edges.
- Vue and Svelte (`js_sfc.rs`): the `<script>` blocks run through the same
  function with the tree-sitter JavaScript or TypeScript parser, so they would
  move too.
- Size: `js_*.rs` plus `js_modules/` come to about 9,400 lines, with 184
  functions that take a `tree_sitter::Node` and 328 distinct grammar kind or
  field strings.
- Node kinds: `File`, `Class`, `Function`, `Test` and `Type`.
- Edge kinds: `CONTAINS`, `CALLS`, `REFERENCES`, `IMPORTS_FROM`, `INHERITS`,
  `IMPLEMENTS`, `TESTED_BY` and `CROSS_ARTIFACT`.
- `extra` keys include `declaration_file`, `export_default`, `overloads`,
  `merged_declarations`, `accessors`, `type_role`, `type_positions`,
  `relationship_role`, `call_kind`, `receiver_type`, `receiver_from`,
  `import_kind`, `alias_form`, `external`, `external_package`,
  `unresolved_module`, `test_modifiers` and the confidence and evidence
  fields.
- Spans: line numbers are tree-sitter `start_position().row + 1`. oxc spans
  are byte offsets, so the port needs a line-start table and a binary search
  for each offset. That mapping is easy and cheap; the oxc walk below already
  does it.
- Parity guards: `core_tests/javascript_*.rs` and `typescript_*.rs` (about
  140 KB of tests), `tests/test_parser.py`, the per-fixture `graph.json` MCP
  snapshots, `test_rust_backend_parity.py` and `test_parity_export.py`.

## Coverage check

| Case | tree-sitter (today) | oxc 0.146 |
| --- | --- | --- |
| JSX in `.js`/`.jsx`, TSX | yes | yes (`SourceType::from_path`) |
| `.mjs` `.cjs` `.mts` `.cts` | yes | yes |
| `.d.ts`, `declare module`, `declare global` | yes | yes; flags `.d.ts` statements as recoverable diagnostics |
| Decorators (class, method, field) | yes | yes (`Decorator` on class, method, property, accessor and parameter) |
| Flow (`// @flow`, `import type`, annotations) | parses most of it: `f`, `g` and the import are extracted | **panics** with "Flow is not supported" and returns an empty program |
| Any syntax error | recovers locally: `ERROR` nodes, surrounding declarations kept | **panics** and returns an empty program; `ParseOptions` has no recovery switch |
| Valid but newer TS syntax | 50 of 6,889 node_modules files contain `ERROR` nodes, for example `readonly onSuccess: import("./Event").Event<T>` (an `import()` type) in `cockatiel` `.d.ts` files, and `let argv` followed by a line that starts with `[` (valid through ASI) in `argparse.js` | 0 panics; 5 recoverable diagnostics (top-level `return`, statements in `.d.ts`) |

Examples you can reproduce: write each snippet to a file and parse it with
oxc 0.146.0 (the benchmark harness's `oxc-dump` mode, see Artifacts) and
with the `parse_dump` example. With oxc each yields only the `File` node; with tree-sitter
you get:

- `unclosed_call.ts` (`class A { method() { foo( } }` between two exported
  functions): `ok`, `A`, `A.method`, `after` and a call from `after` to `bar`.
- `half_fn.ts` (an unterminated `function half(`): `ok`, `Later`,
  `Later.run`, an `IMPORTS_FROM` edge to `./x`, and calls.
- `garbage.js` (`let = = ;`): `tail`.
- `bad.tsx` (unterminated JSX expression): `File` only for both parsers.

Since files in a working tree are often mid-edit, an oxc-only extractor would
make functions vanish from the graph while someone is typing.

## Measurements

Benchmark: a `js_parse_bench` example run as
`cargo run --release -p dagayn-parser --example js_parse_bench -- MODE LIST`
(see Artifacts) on an Apple Silicon machine with the `release` profile (fat LTO). Each mode
runs in its own process; peak RSS comes from `/usr/bin/time -l` and includes
the whole file list, which is held in memory.

| Corpus | Mode | Time | Throughput | Peak RSS |
| --- | --- | --- | --- | --- |
| repo: `dagayn-vscode/` plus `tests/fixtures`, 108 files, 0.65 MB | tree-sitter parse only | 45 ms | 14 MB/s | 17 MB |
| | oxc parse only | 3.3 ms | 190 MB/s | 7 MB |
| | oxc parse + visitor with line mapping | 4.0 ms | 165 MB/s | 7 MB |
| | full tree-sitter extractor (shipped) | 147 ms | 4.4 MB/s | 23 MB |
| `dagayn-vscode/node_modules`, 6,889 files, 44.4 MB | tree-sitter parse only | 1,605 ms | 28 MB/s | 77 MB |
| | oxc parse only | 102 ms | 435 MB/s | 66 MB |
| | oxc parse + visitor with line mapping | 134 ms | 330 MB/s | 67 MB |
| | full tree-sitter extractor (shipped) | 5,575 ms | 8 MB/s | 130 MB |

The scratchpad corpus had only one `.ts` file, so it was not used.

Profile of the full extractor (macOS `sample`, node_modules corpus, about
7,700 samples):

| Where the time goes | Share |
| --- | --- |
| tree-sitter parsing | 35% |
| tree-sitter cursor and node navigation, including `kind()` strlen/strncmp | 34% |
| allocation | 14% |
| `str::from_utf8` of node text | 8% |
| dagayn's own logic | 4% |
| filesystem (module resolution) | 2.5% |

What this means in practice: oxc could remove most of the first two rows, so
the ceiling is roughly 3.5–4 s saved per 7,000 JS/TS files (5.6 s down to
about 1.5–2 s). On this repository's own JS/TS that is about 0.1 s.

Cost of the dependency:

- Binary: a stripped fat-LTO binary that links the oxc parser and visitor is
  **+0.56 MB**: 55,697,216 versus 55,134,560 bytes for the same crate without
  oxc.
- Dropping tree-sitter would not shrink the binary much either: the
  JavaScript, TypeScript and TSX grammars come to about 3.3 MB of compiled
  code in a `_core` of about 105 MB. They would also have to stay for broken
  files, Flow, and Vue/Svelte fallbacks.
- Build: release per-crate compile times are oxc_ast 4.0 s, oxc_parser 4.6 s,
  and about 3 s for oxc_regular_expression, oxc_diagnostics, oxc_ast_visit
  and smaller crates, largely in parallel. That adds roughly 9 s to the
  critical path in `release`; `dev-fast` was not measured separately.
- MSRV: no crate in the workspace declares `rust-version`; the floor is the
  pinned `rust-toolchain.toml` (1.95). 0.146.0 is the last oxc release that
  builds on it.
  0.147 to 0.152 need 1.96, and 0.153 needs 1.97. oxc raises its MSRV every
  one to two months, so each oxc upgrade would also force a toolchain bump.

## Options considered

1. **Replace fully.** Rejected. Broken-file and Flow output cannot be
   identical, because the nodes the snapshots contain for those files come
   from tree-sitter's error recovery and oxc has none.
2. **oxc behind an opt-in flag, falling back to tree-sitter when oxc
   panics.** Feasible, and output on clean files can be held identical by the
   existing guards. But it means a second implementation of all 9,400 lines
   above, which every later JS/TS extractor change would have to edit twice.
   It would also produce different output on the ~0.7% of valid files that
   tree-sitter misparses, unless it deliberately reproduced tree-sitter's
   mistakes. On top of that come the MSRV coupling and about 0.5 MB of binary
   size. The gain is seconds only on monorepos with thousands of JS/TS files.
   Not worth it now.
3. **Keep tree-sitter (chosen)** and go after the measured overhead in the
   current extractor, with output unchanged. 34% of the time is navigation
   and 8% is UTF-8 validation, coming from eight extra whole-tree passes,
   string `kind()` comparisons and repeated `from_utf8` on node text. Folding
   the collection passes, comparing `kind_id()` and validating the source
   once are cheaper and keep parity by construction.

Revisit if oxc gains error recovery (a partial AST plus diagnostics instead
of a panic), or if JS/TS parsing becomes the measured bottleneck of `dagayn
build` on real user repositories.

## Artifacts

The benchmark harness (`crates/dagayn-parser/examples/js_parse_bench.rs`,
modes `ts-parse`, `full`, `oxc-parse`, `oxc-walk`, `oxc-dump`, and
`BENCH_LIST_ERRORS=1` to list the files tree-sitter gets wrong) and oxc
0.146.0 as a pinned dev-dependency of `dagayn-parser` were written for this
evaluation but not kept in the tree: with the verdict "keep tree-sitter",
they would only add about 9 s of oxc compilation to every
`cargo test -p dagayn-parser` and `cargo clippy --all-targets`, and about
450 lines to `Cargo.lock`. To reproduce the numbers, add
`oxc_allocator`, `oxc_ast`, `oxc_parser`, and `oxc_span` `=0.146.0` as
dev-dependencies (0.146.0 is the newest release that builds on the pinned
Rust 1.95; 0.147 to 0.152 need 1.96 and 0.153 needs 1.97) and an example
that parses each listed file with both parsers.
