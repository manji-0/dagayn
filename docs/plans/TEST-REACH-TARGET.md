# Tests the call graph cannot see

<!-- constrained-by ./REVIEW-TOOL-TARGET.md#finding-kinds -->

## Question

`review_tool`'s `untested_change` reports changed production functions that
no test reaches, directly or through callers up to 4 hops. On this
repository it reported `crates/dagayn-tools/src/stability.rs` and two
functions that call into it as untested, though
`tests/tools.rs::unstable_dependencies_are_found_where_they_are_introduced`
runs them. Should the judgement change, and how?

Status: the declaration and the call-graph fixes are shipped
([decisions](#decisions-2026-10-08)); dispatch-aware reach is the
follow-up.

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

<!-- derived-from #result -->

The functions still reported after the declaration had no caller in the
graph at all: three gaps of the Rust call graph, closed by the extractor's
version 22 and resolution.

- **A method on a closure parameter over a call's elements**
  (`unstable_dependencies(..).iter().filter(|d| d.introduced_by(..))`):
  the parameter is bound to the elements of what the call returned
  (`receiver_from.element`, through `iter`, `filter`, and the other
  methods that keep the elements), and resolution takes the `T` out of a
  `Vec<T>`, `HashSet<T>`, `Option<T>`, ... in the declared return type. A
  call of a package is left alone: its element types are not in the tables.
- **A function passed as a value**: `.map(Type::method)` (also
  `crate::m::Type::method` and `Self::method`) is a REFERENCES to the
  method, resolved as a typed call is (`receiver_type`, in the type's
  module file); a `let`-bound closure passed by name (`.is_some_and(word)`)
  is one to its node. `untested_change` follows these references as calls;
  type references are not.
- **A method on a field inside a macro**
  (`json!({.. directive.directive_kind.relationship_role() ..})`): the
  field's declared type types the receiver, as outside a macro.

On this repository: 119 fewer unresolved method calls, 45 more resolved
value references, and 6 of 975 element-bound calls resolved (the rest
come from calls of the standard library, left as they were). Every
function the review had still called untested now has a caller.
`tests/tools.rs::closures_over_returned_elements_and_method_values_reach_their_targets`
holds the resolution across files.

## Evaluation

<!-- derived-from #target-contract -->

Depth is not the lever: against the coverage of a test run, functions with
a test five to eight hops away barely exist (one of 1,256 Python production
functions), while the ones with no path to a test at all ran in more than
half the cases. `eval/run_test_reach_eval.py` measures that and gates it in
CI: the pytest run writes `coverage.json`, the job builds this repository's
graph, and the harness asks `query_graph_tool(pattern="tests_for")`'s
`test_reach` (the code path `untested_change` uses) of every Python
production function. Floors in `eval/test_reach_thresholds.yaml`.

Baseline (2026-10-08, 1,256 functions, 339 called untested):

| Metric | Value |
|---|---|
| untested precision (called untested and did not run) | 0.422 |
| untested recall (did not run and called untested) | 0.817 |
| tested precision (called tested and ran) | 0.965 |

| Why called untested | Ran (wrong) | Did not run |
|---|---|---|
| `no_callers`: nothing in the graph calls or references it | 107 | 88 |
| `no_test_path`: callers, but no test within 4 hops | 88 | 55 |
| `beyond_limit`: a test 5–8 hops away | 1 | 0 |

The wrong `no_callers` are calls the Python graph misses: a constructor
call that does not reach `__init__`, properties, framework callbacks
(pydantic validators, `__getattr__`, `with`'s `__enter__`), and functions
called from a table (`{"dfs": _scenario_dfs}`, a command registry). Coverage
counts only the pytest process, so code a test runs through the `dagayn`
command counts as not run: the untested precision is if anything high.

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
  comment directives, closure elements, method values, macro fields),
  `member_calls.rs` (`CallOrigin.element`), `extractor_version.rs`
  (markdown 2, python 12, rust 22, csharp 8, terraform 2).
- Resolution: `crates/dagayn-graph/src/postprocess/returned.rs` (element
  types), `bare_names.rs` (typed value references).
- Graph: `crates/dagayn-graph/src/communities.rs`
  (`get_test_targets_for_source`).
- Tools: `crates/dagayn-tools/src/coverage.rs` (`tests_for`),
  `findings.rs` (`nearest_test`, shared by `untested_change` and
  `tests_for`'s `test_reach`), `query.rs`.
- Eval: `eval/run_test_reach_eval.py`, `eval/test_reach_thresholds.yaml`,
  `tests/test_test_reach_eval.py`, and the CI step.
- Tests: `core_tests/rust_lang.rs`, `dagayn-graph` `tests/analysis.rs`,
  and the declarations in `crates/dagayn-tools/tests/tools.rs`.
- Docs: the writing-markdown-document skill's directive table.
