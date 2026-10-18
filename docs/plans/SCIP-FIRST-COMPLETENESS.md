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

### In dagayn

The resolver ships as `dagayn/scip_tsgo.mjs`. It writes a SCIP index, so
the overlay reads it like `scip-typescript`'s. `build --scip` uses it when
TypeScript 7 is installed (see [COMMANDS.md](../COMMANDS.md#scip-call-resolution)).

| Repository | Indexer | Index | Applying it | `build --scip` | Recall | Precision |
| ---------- | ------- | ----- | ----------- | -------------- | ------ | --------- |
| typeorm | tsgo | 9.7 s | 7.0 s | 41 s | 1.000 | 0.987 |
| typeorm | `scip-typescript` | 24.6 s | 9.5 s | 56 s | 1.000 | 0.988 |
| zod (warm) | tsgo | 1.6 s | — | 5.2 s | 0.963 | 0.998 |
| zod (warm) | `scip-typescript` | — | — | 9.4 s | 0.963 | 0.998 |

`build` without `--scip` takes 19 s on typeorm and 2.7 s on zod.

Where the two indexers differ:

- **Ambiguous positions.** tsgo gives one symbol per identifier, where
  `scip-typescript` often gives several at one position (`ambiguous`: 5
  against 265 on zod, 47 against 3,066 on typeorm).
- **More value references.** It therefore confirms more edges and adds
  more value references. On zod these are mostly getters read as
  properties (`schema.isInt`) and `this.f.bind(this)`.
- **Overloads.** Every declaration of an overloaded method defines it;
  taking only the first counted later overloads as references.

**Resident mode.** `dagayn watch` (what the daemon runs per repository)
keeps one `scip_tsgo.mjs serve` process per TypeScript project. It does so
only when the last full build ran the TypeScript overlay
(`scip_overlay_languages` metadata) and TypeScript 7 is installed.

After each incremental update:

1. The process is told which files changed.
2. It indexes the re-parsed files (the changed ones and their dependents),
   plus the definitions they reference.
3. The overlay settles their edges before post-processing.

The overlay skips the edges of a document that holds only definitions.

Measured on typeorm, from the edit to the overlay applied:

| Edited file | Re-parsed | Index | Total |
| ----------- | --------- | ----- | ----- |
| a test file | 1 | 0.12 s | 0.4 s |
| `PostgresQueryRunner.ts` | 26 | 0.73 s | 1.1 s |
| `SelectQueryBuilder.ts` (dependents capped at 500 of 2,075) | 501 | 3.7 s | 5.3 s |

On zod, a call added to a test (`schema.safeParse(..)` on a `z.string()`)
was settled to `ZodType.safeParse` about a second after the save.

`dagayn update` (the hook) does not start the process: a cold start costs
1 to 7 s.

### tsrs: the Rust port behind the same API

[tsrs](https://github.com/oxc-project/tsrs) (oxc-project, Apache-2.0) ports
the TypeScript 7 compiler from Go to Rust. Its npm package `tsrs` (0.16.0,
TypeScript 7.1.0-dev) ships TypeScript 7.1's JavaScript API, so
`scip_tsgo.mjs` runs on it unchanged but for two points of the 7.1 API:

- Snapshots are made with `api.createSnapshot`, not `api.updateSnapshot`.
- An edit makes a new snapshot with `fileNotifications`. `snapshot.update`
  in tsrs 0.16 answered from the old file contents. A new snapshot after
  an edit took 0.15 to 0.18 s on typeorm.

It also ships the default library with its platform package
(`@tsrs/linux-x64-gnu`), which the indexer names `typescript` as before.

| Repository | TypeScript 7.0.2 | tsrs 0.16.0 | Index |
| ---------- | ---------------- | ----------- | ----- |
| zod | 1.4 s | 1.2 s | byte-identical (25,901 occurrences) |
| typeorm | 11.3 s | 11.2 s | byte-identical (336,381 occurrences) |

The resolver takes `tsrs` when no `typescript` 7 is found.

**Embedding tsrs in the Rust core** was considered and is not done:

- **No gain in speed through the API.** Both are native compilers behind
  the same protocol. On typeorm, about 2 s of the index is the exchange
  (19,584 round trips, 149 MB) and about 3 s type checking, per the earlier
  tsgo measurement; in-process the floor is roughly the check.
- **Not published.** Every crate is `publish = false`, a git dependency
  pinned to a TypeScript nightly, at version 0.x.
- **Toolchain.** It needs Rust 1.99.1; dagayn is on 1.98.0.
- **Size.** The checker alone is 109k lines, with 16 tsrs crates and 51
  others under it. The `tsrs` binary is 33 MB.
- **API.** `new_program` returns `&'static Program`, leaked per build,
  which a long-running `dagayn watch` would have to manage.

Revisit when the crates are published with a stable API. Until then the
`--api` process keeps the compiler swappable (TypeScript 7 or tsrs) with
no change to dagayn's build.

## Open questions

- **Default on.** `--scip` is opt-in. Whether `build` should use an
  installed indexer by itself (with its prerequisites present) is a
  product decision. The cost is the indexer's time (axum: 140 s against
  about 10 s without) and a build environment (`node_modules`, a Cargo
  workspace).
- **Hook updates.** Under `dagayn update` (the hook), overlay edges of a
  changed file are replaced by Tree-sitter's until the next `--scip`
  build. Step 3 keeps that window from losing value references. `dagayn
  watch` closes the window for TypeScript with the resident tsgo resolver;
  Rust and the other languages have none yet.
- **Indexer versions.** Positions and symbol formats differ across
  indexer versions. The overlay's tests pin the shapes it reads; new
  indexer releases need a check.
