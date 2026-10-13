# SCIP first: structural completeness instead of more inference

<!-- derived-from ./SCIP-CALL-RESOLUTION.md -->
<!-- constrained-by ../CALL-RESOLUTION.md#scip-overlay -->

## Why

Two rounds of caller-recall fixes, each measured on held-out repositories
that took no part in finding them, generalised poorly:

| Round | What was added | Dev recall | Held-out recall |
| ----- | -------------- | ---------- | --------------- |
| N (Go) | dot imports, package-scoped types, embedding, function values | 0.537 → 0.863 | 0.783 → 0.843 |
| N (Rust) | same-file function values, calls through glob imports | 0.594 → 0.702 | 0.818 → 0.820 |
| N2 (TypeScript) | inherited methods, factory aliases | 0.754 → 0.802 | 0.830 → 0.830 |

Each fix was a rule written for a shape that one or two development
repositories had. These are the rules
[SCIP-CALL-RESOLUTION.md](./SCIP-CALL-RESOLUTION.md#freezing-the-inference-layer)
already proposed to freeze for Rust and TypeScript.

The protocol and per-repository numbers are in the evaluation repository
(`heldout/graph/PROTOCOL*.md`, `RESULTS*.md`). The classifier below is
`heldout/misses.py` there.

## What the misses are

Every missed (caller file, callee) over 14 repositories (Rust: axum,
tracing, ripgrep, alacritty; Go: caddy, lazygit, gin, fzf; TypeScript:
rxjs, typeorm, zod, mobx, xstate, query), by what the graph has at the
call:

| Group | Rust | Go | TypeScript |
| ----- | ---- | -- | ---------- |
| A. An edge with an unresolved target, receiver type unknown | 54 % | 58 % | 48 % |
| B. A function used as a value, no edge | — | 3 % | 35 % |
| C. A bare name, unresolved | 13 % | 2 % | 7 % |
| D. No edge at all (macros, chains recorded on another line) | 28 % | 37 % | 7 % |
| E. Bound to another declaration | 4 % | 1 % | 2 % |

- Group A appears in all 14 repositories.
- For a plain variable receiver, the variable's own type annotation types
  only 5–9 % of the misses. The type has to come from somewhere else:
  - a call's return type (27–57 %),
  - a callback parameter typed by context (TypeScript 39 %),
  - a pattern (Rust 17 %),
  - a loop element (Go 14 %).
- `returned.rs` drops generic arguments (`drop(arguments)` in
  `returned_by`), so none of these compose.
- Closing group A on top of Tree-sitter means writing a type checker for
  each language. That is the layer the SCIP plan decided to stop growing.

## The SCIP overlay already closes group A

`dagayn build --scip` on three development repositories, N2 sources,
default indexer settings:

| Repository | Indexer | Build | Recall without → with | Precision with |
| ---------- | ------- | ----- | --------------------- | -------------- |
| zod (TypeScript) | scip-typescript 0.4.0 | 22 s | 0.731 → 0.910 | 0.998 |
| caddy (Go, fills only) | scip-go 0.1.26 | 25 s | 0.923 → 0.993 | 0.985 |
| axum (Rust) | rust-analyzer 1.97.0 | 140 s | 0.786 → 0.845 | 0.980 |

What is left is structural, not typing:

- **zod:**
  - 38 of the 44 misses are function values with no edge (group B).
  - With an edge for every reference whose definition has a node, recall
    would be at most 0.965.
  - The other 17 targets are functions held in `const`s inside test
    callbacks, which have no node.
- **axum:**
  - 155 of the 222 misses are calls of functions a macro defines
    (`top_level_handler_fn!(get, GET)` defines `get`, `post`, `delete`,
    …). dagayn has no node for them, so the overlay's
    `node_at(definition)` finds none and leaves the edge (`left: 898`).
  - The other 67 are calls with no edge (inside macros, a chain on
    another line).
  - With an edge for every one of those, recall would be at most 0.892.
- **caddy:** 17 misses, all with a node; at most 1.000.

**A caveat on these numbers.** The truth of this benchmark comes from the
same compilers the indexers wrap:

- rust-analyzer's own SCIP for Rust,
- the TypeScript compiler API for TypeScript,
- `go/types` for Go.

So with `--scip` the recall numbers diagnose what dagayn's structure cannot
hold. They are not evidence that the overlay is right. That evidence has to
come from agent experiments on sealed tasks (below).

## Plan

<!-- derived-from #the-scip-overlay-already-closes-group-a -->

### 1. Complete the overlay: edges for references it has none for

- **What it adds.** Today the overlay only rewrites `CALLS` edges that
  Tree-sitter extracted. Extend it to every non-definition, non-import
  SCIP reference in an indexed, unchanged file whose definition the
  overlay can name (`node_at` today: the innermost `Function`, `Test`,
  `Class`, or `Type` node of the symbol's name) and that no edge at that
  line already names. For each, add:
  - a `CALLS` edge when the reference is called: the next non-space
    character after it, past a generic argument list or a Rust `!`, is
    `(`;
  - otherwise a `REFERENCES` edge with `value_reference: true`.
- **Its source** is the innermost `Function` / `Test` node holding the
  line, else the file (as the extractors do).
- **Its metadata:** `resolved_by: "scip"` and `scip_added: true`, tier
  `HIGH`.
- **Value references.** Settle the existing `value_reference`
  `REFERENCES` edges the same way as calls.
- **Counts.** This answers the open question "Edges Tree-sitter never
  extracts" of the SCIP plan. It changes edge counts only in a
  `--scip` build, and `scip_added` lets tools and tests tell those edges
  apart.

This closes groups B and D wherever an indexer runs, for every language
the overlay reads, without a rule per syntax position.

### 2. Nodes for definitions only a macro or generator writes

The axum case is general: macro-generated functions, and generated code in
other languages. With an index, a SCIP definition of a function that no node
holds can get a node:

- `kind: Function`, named by its symbol, at the definition's range
  (usually the macro call);
- `extra.generated: true`, `resolved_by: "scip"`.

This is an open design point rather than step 1's mechanical one:

- Nodes feed communities, flows, and the File/`CONTAINS` structure, and a
  hook update of the file removes them until the next `--scip` build.
- The first version should restrict itself to Rust definitions inside a
  `macro_invocation` range, and measure what else would qualify.

### 3. Value references without an index: invert the rule

- **Now:** the extractors list the syntax positions that count as a
  value reference (`arguments`, `pair`, `return_statement`,
  `variable_declarator`, …), and each round added positions.
- **Proposed:** every identifier (and `this.x` / `Class.x` member) in
  expression position that names a function of the file or an import
  counts. Excluded:
  - the callee of a call,
  - a declaration's name,
  - a property key,
  - an import or export specifier,
  - a type position,
  - the left side of an assignment.
- **Where it matters:** this is what a hook update and a build without
  an indexer see. TypeScript first (35 % of its misses), then Go and
  Rust.

### 4. Keep the inference layer frozen, and decide about rounds N / N2

- **No new receiver-typing rules.** Fix bugs only, as the SCIP plan
  says.
- **The rules rounds N and N2 added are unreleased** (the `Unreleased`
  section of the changelog):
  - Go's (dot imports, package-scoped types, embedding, function values)
    generalised (+0.06 held out), and Go's indexer only fills, so they
    stay.
  - The Rust glob-prefix calls and the TypeScript inheritance and
    factory-alias rules did not generalise. They are frozen-layer
    additions and candidates to revert before the next release.
  - That call is the maintainer's.

### 5. Evaluate by agent experiments, not graph recall

- **Graph recall** with `--scip` diagnoses structure (it is near-circular
  with the truth), so it gates steps 1–3 only as a regression and gap
  check.
- **Effect claims** need a new sealed agent experiment: the same task
  generator, fresh repositories per language, with these conditions:
  - none,
  - dagayn without an index,
  - dagayn with `--scip`.
- **What it measures:** the pre-registered tokens, time, F1, and exact
  matches, plus build cost (time, peak memory) per repository.

## Status

<!-- derived-from #plan -->

**Step 1 is done** (`9002f86c`). Recall with `--scip`:

| Repository | Overlay before | Completed | Precision |
| ---------- | -------------- | --------- | --------- |
| zod | 0.910 | 0.961 | 0.998 → 0.998 |
| axum | 0.845 | 0.867 | 0.980 → 0.979 |
| caddy | 0.993 | 0.994 | 0.985 → 0.985 |

The first version gave the same recall at a lower precision (axum 0.968).
Four filters fixed that:

- `.await` is a reference to `poll`, but the name written there is not
  the definition's;
- Rust attributes (`#[crate::test]`);
- `use` lines and the lines of a multi-line `use` / `export { … }`;
- calls through a generic argument list holding parentheses
  (`f::<(A,)>()`).

**Step 3 is done for JavaScript / TypeScript** (`db3c3144`, javascript
14). Without an index, on the six TypeScript repositories:

- recall rose by 0.000 to 0.012 (rxjs 0.754 → 0.758, mobx 0.679 →
  0.687, query 0.707 → 0.719);
- precision was unchanged.

The rule is now one rule instead of a list of positions. But what is left
of group B in TypeScript is not names:

- 1,081 of the 1,490 value misses are members (`observer.next`,
  `this._next = onNext`), which need the receiver's type, so the index's.
- Most of the other 409 are functions held in a `const` inside another
  function, which have no node of their own.

So for TypeScript, step 3 is about a hook update keeping what a build
found, not about closing the gap.

## TypeScript without scip-typescript: a tsgo resolver

<!-- derived-from #open-questions -->

TypeScript 7.0.2 (the Go compiler) ships an unstable API,
`typescript/unstable/sync`, that answers `getSymbolAtLocation` for many
nodes per request over IPC. A resolver on top of it asks for every
identifier's symbol and takes each declaration's line from the source
files it has already fetched. Measured in the evaluation repository
(`tsgo/RESULTS.md`, 4 cores):

| typeorm, with `node_modules` | Time |
| ---------------------------- | ---- |
| `scip-typescript` 0.4.0 | 29.3 s |
| TypeScript 5.9 compiler API, symbols only | 15.1 s |
| tsgo, every identifier (339 k) | 9.5 s |
| tsgo, identifiers named like a repository function or class (147 k) | 6.5 s |

- **Agreement.** tsgo agrees with TypeScript 5.9 on 97–99.8 % of
  references. The differences examined (zod) are which of several
  declarations a union property returns first.
- **Resident mode.** Kept running, the resolver re-resolves an edited
  file and the files importing it in 0.13–0.30 s per edit (typeorm,
  `updateSnapshot({fileChanges})`). That is fast enough for the hook
  update, which would close the hook-update gap below for TypeScript.
- **Costs:**
  - TypeScript 7 rejects removed options (`downlevelIteration`,
    `moduleResolution: node`), so the resolver needs a derived tsconfig
    that overrides them.
  - The API is marked unstable.

## Open questions

- **Default on.** `--scip` is opt-in. Whether `build` should use an
  installed indexer by itself (with its prerequisites present) is a
  product decision. The cost is the indexer's time (axum: 140 s against
  about 10 s without) and a build environment (`node_modules`, a Cargo
  workspace).
- **Hook updates.** Overlay edges of a changed file are replaced by
  Tree-sitter's until the next `--scip` build. Step 3 is what keeps that
  window from losing value references.
- **Indexer versions.** Positions and symbol formats differ across
  indexer versions. The overlay's tests pin the shapes it reads; new
  indexer releases need a check.
