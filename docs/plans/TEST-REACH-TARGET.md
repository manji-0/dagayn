# Tests the call graph cannot see

<!-- constrained-by ./REVIEW-TOOL-TARGET.md#finding-kinds -->

## Question

`review_tool`'s `untested_change` reports changed production functions that
no test reaches, directly or through callers up to 4 hops. On this
repository it reported `crates/dagayn-tools/src/stability.rs` and two
functions that call into it as untested, though
`tests/tools.rs::unstable_dependencies_are_found_where_they_are_introduced`
runs them. Should the judgement change, and how?

Status: the declaration is shipped ([decisions](#decisions-2026-10-08));
dispatch-aware reach is the follow-up.

## Evidence

The path from the test to `stability::unstable_dependencies` exists and is
static: `stability::unstable_dependencies ← architecture_findings ←
with_unit_map ← arch_tool::architecture ← lib::answer ← lib::call ←
tests/tools.rs::answer` (a test helper, kind `Test`). The test is six hops
up, past the limit of four.

Raising the limit is the wrong fix. `lib::call` has 46 callers within two
hops, and every tool test reaches it; `lib::answer` picks the tool by its
name (`match name { "review_tool" => review::review(...), ... }`). At six
hops every function under any arm counts as tested by every tool test:
`stability.rs` would be "tested" by the graph-statistics test. The review
eval's `untested_change_far_from_tests` case (nearest test five callers
away) holds the limit at four for that reason.

What the call graph misses is which arm a test selects: the test passes the
tool name as a string literal, and `CALLS` edges carry no argument text.

## Target contract

> A test can declare the code it exercises, where the call graph cannot
> follow it: `// dagayn: tests <path>::<symbol>` (any language whose code
> comments dagayn reads). `tests_for`, `untested_change`, and
> `tests_to_run` count the declaring test as a direct test of that symbol.

- **Declared, not inferred.** The author states what the test selects; the
  graph does not guess which arm a string reaches.
- **A direct test.** The declaration is a test edge into the symbol, so the
  four-hop walk starts from there: declare the arm a test selects
  (`review::review`), not every function below it.
- **A directive opens its comment.** A `dagayn:` mention inside prose, and
  in Rust anything inside a string literal, is no edge.

## Decisions (2026-10-08)

- Declare first; understand dispatch later.
- The declaration is the `tests` directive: a `CROSS_ARTIFACT` edge from
  the test to the symbol, role `tests`, bridge kind `test`. The graph's
  test lookup (`get_test_targets_for_source`) and `tests_for`'s coverage
  scan read it next to `TESTED_BY`; no `TESTED_BY` edge is synthesised.
- Rust code comments carry `dagayn:` directives, read from comment nodes.

## Result

- `crates/dagayn-tools/tests/tools.rs`: 37 tests declare the 17 dispatch
  arms they call by name (64 declarations, generated from the tool names in
  each test body and `lib::answer`'s arms).
- `tests_for crates/dagayn-tools/src/arch_tool.rs::architecture`: 0 → 4.
- Of the ten functions the review called untested before, six are now
  reached: `Snapshot.dependency_edges`, `findings::unstable_dependencies`,
  and four of `stability.rs`.
- The review and architecture evals are unchanged.

## Remaining gaps

The functions still reported have no caller in the graph at all; these are
gaps of the Rust call graph, not of test reach:

- A method called through a closure parameter
  (`.filter(|dependency| dependency.introduced_by(...))`): the receiver's
  type is not inferred, and the call stays `introduced_by`, unresolved.
- A function passed as a value (`.map(UnstableDependency::finding)`,
  `.is_some_and(word)`): no edge.
- A method called on a field (`self.directive_kind.relationship_role()`).

## Follow-up: dispatch-aware reach

Record string-literal arguments on `CALLS` edges and the string patterns of
`match` / `if` arms that call; a test then reaches the arm its literal
selects, through any number of forwarding calls that pass the parameter on.
It needs parser work per language and its own eval (a dispatcher with two
arms, a test selecting one: the other stays untested). Until then, the
declaration is the way to state it.

## Touch points

- Parser: `crates/dagayn-parser/src/documentation_directives.rs` (the
  `tests` kind; a directive opens its comment), `rust_lang/mod.rs` (Rust
  comment directives), `extractor_version.rs` (markdown 2, python 12,
  rust 21, csharp 8, terraform 2).
- Graph: `crates/dagayn-graph/src/communities.rs`
  (`get_test_targets_for_source`).
- Tools: `crates/dagayn-tools/src/coverage.rs` (`tests_for`).
- Tests: `core_tests/rust_lang.rs`, `dagayn-graph` `tests/analysis.rs`,
  and the declarations in `crates/dagayn-tools/tests/tools.rs`.
- Docs: the writing-markdown-document skill's directive table.
