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

The ground truth depends on what the test run can run: a test that skips
leaves the functions only it reaches unexecuted, which lowers untested
recall and tested precision without any change to the graph. Without jj
and matplotlib, five functions (`export_svg`, `_jj_diff_files`, three in
`jj_workspace.py`) went unexecuted and recall fell from 0.691 to 0.672,
so CI's test job installs both, jj pinned to the release used locally.

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

### Rust

<!-- derived-from #evaluation -->

The same harness reads a `cargo llvm-cov --json` export (CI's Rust job runs
`cargo llvm-cov` already, with `--summary-only`). A function's own records
are those whose first region starts first within its span: one per generic
instantiation, an unused function with count 0, a closure separate. llvm-cov
counts what the binaries the tests spawn ran, which the call graph does not
cross, so here the untested precision is if anything low. Report only, no
gate yet.

Baseline (2026-10-08, 3,211 production functions; tests, `mod tests`,
`tests.rs`, `*_tests/` and `dagayn-py` excluded). The hop columns come from
a one-off run with the reported search widened past its usual 8 hops (the
deepest Rust test path is 12):

| Hop limit | Called untested | Untested precision | Untested recall | Tested precision |
|---|---|---|---|---|
| 4 (today) | 697 | 0.122 | 0.381 | 0.945 |
| 8 | 185 | 0.292 | 0.242 | 0.944 |
| none | 137 | 0.387 | 0.238 | 0.945 |
| Python, 4 (today) | 140 | 0.864 | 0.691 | 0.952 |
| Python, none | 135 | 0.874 | 0.674 | 0.949 |

- **Depth is the lever in Rust, not in Python.** Rust's call chains are
  deep (the `answer` → tool → findings → store path of an integration
  test): 612 of the 697 functions called untested ran, 481 of them with a
  test 5–12 hops away. Among the Rust functions a test reaches in 1–4 hops,
  6.6% did not run; 5–8 hops, 6%. Hop count barely separates run from not
  run, while having no path at all does (39% did not run).
- **What no limit still misses**: 138 of the 223 functions that did not run
  are 1–4 hops from a test, a branch the test does not take. The call graph
  cannot see that; the recall ceiling here is branch-level.
- **Calls through a trait are a small part.** Of the functions with no path
  (84 ran, 53 did not), methods of a standard trait (`Drop`, `Deserialize`,
  `visit_map`, `fmt`) are about 16 that ran, called by the standard library
  or serde, which no dispatch rule reaches; methods of the repository's
  traits (`Backend`, `Native`) about 5. In Python, the only overridden
  methods are the 25 of the `EmbeddingProvider` hierarchy, and reaching
  overrides from the base method's callers measured net zero. A dispatch
  rule is not worth its cost in either language here.

Applied: `caller_test_depth` lifts the limit for Rust and keeps 4 hops
elsewhere (Python measured 0.874 / 0.674 / 0.949 with no limit, too close
to its recall floor of 0.67 to trade). `test_reach.hop_limit` is `null` for
Rust. CI's Rust job writes the coverage of its `cargo llvm-cov` run per
function (`cargo llvm-cov report --json`) and gates it with
`eval/test_reach_rust_thresholds.yaml`: untested precision 0.387, recall
0.233, tested precision 0.943 on 3,199 functions. `untested_core` in the
architecture overview still walks 4 hops in every language.
The walk keeps no visit cap (unlike `flow_tool`'s entry points, which stop
at 10,000): it ends at the nearest test, its `seen` set bounds it, and the
deepest Rust path here is 12 hops; asking it of all 3,199 Rust functions
takes 49 s on 8 workers, against 20 s for Python's 1,256 (with the
harness's graph and process start-up in both).

## Python call graph

<!-- derived-from #evaluation -->

The wrong `no_callers` above, closed where the graph can see the call:

- **Functions passed as values** (extractor python 13): an argument
  (`run_guarded(args, dispatch)`, `Thread(target=self._loop)`) or a tuple
  element (`[("dfs", _dfs)]`) is a REFERENCES, also to a function nested
  in the caller or a method of its class, kept only if the file defines it
  (`self.store` is an attribute).
- **A module imported in a function** (`from pkg import helper` in a test
  body, then `helper.run()`): the call records the module's file
  (`module_file`), so resolution finds `run` there.
- **Methods the runtime calls**: `nearest_test` treats a dunder method
  (`__init__`, `__enter__`) and a method decorated `@property`,
  `@cached_property`, `@field_validator`, ... (or its `setter`) as called
  wherever its class is: the walk goes on from the class's callers and
  referrers. A plain method is not reached by constructing its class.

| Metric | Baseline | After |
|---|---|---|
| called untested | 339 | 186 |
| untested precision | 0.422 | 0.656 |
| untested recall | 0.817 | 0.697 |
| tested precision | 0.965 | 0.950 |

| Why called untested | Ran (wrong) | Did not run |
|---|---|---|
| `no_callers` | 28 | 68 |
| `no_test_path` | 33 | 52 |
| `beyond_limit` | 3 | 2 |

132 fewer wrong untested claims cost 21 right ones: code a test reaches
in the graph but the pytest run never executed. The two changes were
measured together.

Of the 53 functions now called tested that did not run, 9 come through
the class step and the rest through ordinary calls and the new argument
references (`initializer=_init_worker`, `signal(.., _handle_sigterm)`,
`Timer(.., _expire)`): the graph path is real, but the test mocks the
call or takes another branch. Not a parsing error, and not one a reach
rule can tell apart. Taking only CALLS into the class step (no
REFERENCES) was measured and rejected: tested precision stayed 0.950 and
4 more functions were wrongly called untested (untested precision 0.642). `tests/tools.rs::a_test_that_uses_a_class_reaches_the_methods_the_runtime_calls`
holds the method reach.

## Code that runs without a caller

<!-- derived-from #python-call-graph -->

The functions that ran but had no caller in the graph, closed one
mechanism at a time and measured after each:

| Step | Untested precision | Recall | Tested precision |
|---|---|---|---|
| Before (python 13) | 0.656 | 0.697 | 0.950 |
| A nested function in a list or dict, assigned to an attribute, or returned (python 14) | 0.753 | 0.697 | 0.952 |
| Imports that run a module-level instance's methods or a package's `__getattr__` | 0.797 | 0.697 | 0.952 |
| `from m import f as g`, and both branches of `x = f if c else g` (python 15) | 0.819 | 0.697 | 0.952 |
| A module-level call of a function that builds the instance (`STORE = make_store()`) | 0.846 | 0.691 | 0.951 |
| A call at a module's top level, outside any branch, loop, `except` handler, or lambda (`TABLE = _registry()`, python 16) | 0.864 | 0.691 | 0.952 |

- **What a module's import runs.** `nearest_test` follows IMPORTS_FROM
  into a Python module only when the walk reached it through an implicit
  method: the module builds the class's instance at its top level, calls a
  function that builds it, or defines the `__getattr__` / `__dir__` being
  walked. Following imports into every module that runs code at its top
  level was measured and rejected: untested precision 0.897, but recall
  fell to 0.549 (26 right untested answers lost with 29 wrong ones).
- **The cost of the last step** is one right answer:
  `_SharedPendingRefactors.__iter__` runs on iteration, which no test does,
  but the class step counts every dunder method of a class in use.
  `WorkerLock.__exit__` is the same limit.

Left after these (3 functions that ran with no caller):

- **Calls through a base class**: `provider.embed()` on a receiver typed
  only as `EmbeddingProvider` runs `_TokenHashEmbeddingProvider.embed`.
  Reaching overrides from a call of the base method needs the receiver's
  declared type and an override edge; an eval case is a base class with
  two subclasses and a test that builds one.
- **Nested pydantic models**: `GuidanceEvidence.normalize_type` is a
  validator of a model used only as a field type of another model, which
  the class step does not follow (a field annotation is a type reference).

`tests/tools.rs::importing_a_module_reaches_what_its_import_runs` holds
the import steps.

## Callers but no test path

<!-- derived-from #code-that-runs-without-a-caller -->

The functions that ran and have callers, but no test within the limit (17
after the steps above), almost all had a module's top level as their only
caller. Among every production function whose only callers are module
top levels and that no test reached (35), the two edge kinds split
differently:

| Module-level edge | Ran | Did not run | What tells them apart |
|---|---|---|---|
| CALLS | 3 | 5 | Whether the call runs on import |
| REFERENCES (a value in a table) | 11 | 16 | Nothing static |

- **Calls.** The 3 that ran are statements at the top level
  (`METRIC_SPECS = _registry()`, `_pending_refactors = _make_store()`, and
  `_retrieval_specs` under `_registry`). Of the 5 that did not run, three
  are `main()` under `if __name__ == "__main__":` or in `__main__.py`
  (which no module imports), and two are `@mcp.tool()` on
  `_FallbackFastMCP`, a class bound only in an `except ImportError:`
  handler that the parser matched by method name on a receiver of unknown
  type. The parser (python 16) marks a top-level call outside any branch,
  loop, `except` handler, or lambda `import_time`; `nearest_test` follows
  IMPORTS_FROM into the module of such a call when it was resolved by name,
  not guessed on an unknown receiver (`receiver_unknown`). All three
  right answers were kept.
- **Table entries are left untested, on purpose.** `INDEXERS` in
  `scip_overlay.py` holds eleven `Indexer(arguments=_x_arguments)`
  entries; ten ran and `_python_arguments`, the same shape, did not. The
  CLI handler table, `_scenario_*` tables, and `queue_worker`'s
  `_execute_*` entries did not run either. Which entry a table's reader
  calls depends on data, so following the module's imports from a table
  entry is the rejected wide rule again.

Left (14): the ten `_x_arguments` entries; `_zed_settings_path`, called in
a lambda of the platform table; `with_dispatch_metadata`, called through a
module-level `partial(with_dispatch_metadata, ..)` alias (a call of the
alias could resolve to the function it wraps); `_vectorize`, under the
base-class gap above; and `_ReadLockBoundStore.close`, a proxy
`wrap_store_close_to_unbind` returns as the `_CloseableStore` protocol, so
`store.close()` is a call through the protocol (the same gap).

`tests/tools.rs::importing_a_module_reaches_its_top_level_calls_only`
holds the import-time step.

The import walk follows every IMPORTS_FROM into a module, including an
import under `if TYPE_CHECKING:` (`import_scope: type_checking`, python
11), which never runs; excluding those is a separate step.

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
  `member_calls.rs` (`CallOrigin.element`), `python/mod.rs` (functions
  as values, `module_file`, aliased imports), `extractor_version.rs`
  (markdown 2, python 15, rust 22, csharp 8, terraform 2).
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
