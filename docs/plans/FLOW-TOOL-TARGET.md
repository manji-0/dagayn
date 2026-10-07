# What flow_tool is for

## Question

<!-- constrained-by ../COMMANDS.md#mcp-tools -->

`flow_tool` is a Tier 1 tool: the debug, explore, implement-feature, and
review skills and the onboarding and architecture prompts send agents to
it. Does it answer a question an agent has, and what should it be?

Verdict: **today it ranks stored reachable sets that no longer answer
anything.** The ranking does not separate one flow from another, the names
do not identify them, one call can return half a million characters, and
both tools that consumed flows (`review_tool` and
`architecture_analysis_tool`) stopped trusting them in their redesigns.
The question agents bring to it, "where is this code entered from?", is
one it cannot ask. This note defines the target contract, the evidence
behind it, and the order of work.

Status: accepted ([decisions](#decisions-2026-10-07)); steps 1–4 of the
[order of work](#order-of-work) are done.

## Target contract

> `flow_tool` answers **which entry points reach this code, and through
> which calls**. Given a target, it returns the repository's entry points
> (CLI commands, MCP tool dispatch, FFI exports, framework handlers) whose
> calls reach the target, each with one shortest call chain. Without a
> target, it lists the entry points, grouped by declared unit and kind.

Consequences of that sentence:

- **The question starts from the code the agent is looking at.** Debugging
  ("how does a request get here?"), implementing ("which command already
  runs this?"), and reviewing ("who is exposed to this change?") all start
  from a symbol, not from a list of flows.
- **A chain, not a set.** Each entry point comes with one call path, which
  an agent can read in order; a 512-member reachable set is not.
- **No ranking score.** Entry points are listed, not scored; criticality
  goes.
- **Computed at query time.** A reverse search over `CALLS` and the
  high-confidence cross-artifact edges the flow trace already follows, so
  the answer exists right after `ensure_graph_tool`
  (`postprocess="minimal"`) and is never stale.
- **Tests and fixtures are not entry points.** The same path
  classification as `review_tool`'s `untested_change`.
- **Bounded output.** `minimal` fits in about 2K tokens: 10 entry points
  with their chains, the rest counted.

## Evidence: the current output

Measured on 2026-10-07 against this repository at `60864bcb` (875 files,
16,776 nodes, full postprocess: 373 flows), with `dagayn tool flow_tool`.
Sizes are the response as compact JSON, as an MCP client receives it.

| Call | Output (chars) |
|---|---|
| `list` (default `limit=50`, `standard`) | 292,909 |
| `list`, `minimal` | 7,318 |
| `get flow_name=main` | 177,111 |
| `get flow_name=main`, `minimal` | 177,111 (identical) |

| Over all 373 stored flows | Value |
|---|---|
| median members | 4 |
| cut at the 512-node cap (`max_nodes`) | 23 |
| criticality p10 / p50 / p90 / max | 0.16 / 0.37 / 0.70 / 0.75 |
| entry point under `tests/fixtures/` | 83 (22%) |
| entry point in eval, benchmark, or `tools/` scripts | 33 |
| share their name with another flow | 151 (`main` ×28, `run` ×15, `handle` ×14) |

What the numbers mean:

- **The output is unbounded.** `list` returns every row's `members` and
  `path` id arrays in `standard`; `get` returns 512 steps and ignores
  `detail_level`, because `FlowGetRequest` has no such field
  (`dagayn/contracts/state_types.py`) and `get_flow`
  (`crates/dagayn-tools/src/flow.rs`) takes none.
- **Criticality ranks size.** The score is
  `0.30·file_spread + 0.20·external + 0.25·security + 0.15·test_gap +
  0.10·depth` (`crates/dagayn-graph/src/flow_trace.rs`). File spread
  saturates at 5 files and depth at 10, so every large flow lands at
  0.69–0.75; it correlates 0.51 with file count. The security term uses
  the same helper (`is_security_sensitive_identifier`) whose prefix match
  `REVIEW-TOOL-TARGET.md` found
  ranking `python_call_signature` as security-sensitive.
- **The top of the list is not the product.** Of the 20 highest-ranked
  flows, 8 start in eval benchmarks or `tools/` scripts and one in
  `tests/conftest.py`. The test filter excludes test files but not
  fixtures, so a fifth of all flows start in a fixture sample.
- **Names do not identify a flow.** `list` in `minimal` returns names
  without file paths, and 151 flows share their name with another.
- **Reachable sets saturate.** The large flows all reach the same core
  (parser, store), which is why every changed node was "in a critical
  flow" in the old `review_tool` (`REVIEW-TOOL-TARGET.md`), and why
  `ARCHITECTURE-TOOL-TARGET.md` made flows no input of the overview.
- **The data is often missing.** Flows exist only after a full
  postprocess. `ensure_graph_tool` and the edit hooks run `minimal`, so
  on a bootstrap graph `flow_tool` has nothing, and after edits its sets
  go stale (`docs/SESSION-GRAPH-FRESHNESS.md`).

Consumers of the stored flows today:

| Consumer | Use | Depends on it? |
|---|---|---|
| `review_tool(mode="changes")` | `affected_flow_count`, the flow term of the deprecated risk score, `affected_flows` in `standard` | no: `findings` do not read flows; the score is behind `verbose` |
| `review_tool(mode="affected_flows")` | the annotated flows touching the change | yes, it is this tool's question asked from a diff |
| `get_minimal_context_tool` | `top_flows` (3 names), `flows_affected` (5 names) | no: names without a file, from the ranking above |
| answerability | `missing_flows` costs 0.15 of the score | only as a penalty |
| wiki, visualization export | top 200 / 100 flows by criticality | yes, as page content |
| `flow_snapshots` table | written by `summary_flows.rs`, never read | no |

## The question, measured

<!-- derived-from #target-contract -->

A prototype reverse search over the same graph: from the target, follow
`CALLS` edges backwards breadth-first, stop at nodes the flow trace already
treats as entry points, drop test and fixture entries, and keep one
shortest chain per entry point.

| Target | Callers reached | Entry points (non-test) | Shortest chains |
|---|---|---|---|
| `assess_graph_sync` | 48 | 2 | `RustTools.call_tool → call → get_minimal_context → assess_graph_sync`; `main → run → run → status_lines → assess_graph_sync` |
| `is_production_code` | 55 | 1 | `RustTools.call_tool → call → architecture → with_unit_map → unit_map → is_production_code` |
| `get_flows` | 42 | 1 | `RustTools.call_tool → call → flow → list_flows → get_flows` |
| `node_text` | 1,324 | 18 | `main → RustOwnedParser.parse_file → … → zig_c_import_namespaces → node_text` (6 hops) |

Each answer fits in a few hundred characters and says something the
caller list does not: `assess_graph_sync` is reached by the MCP
`get_minimal_context` tool and by `dagayn status`, and by nothing else.

The prototype also shows methods with no static caller. For
`node_text`, 3 of the 18 are trait methods called through dynamic
dispatch (`CStdlibScope.visit`, `CStdlibScope.std_type`,
`PhpStdlibScope.collect_use`). They cannot be dropped: the entry of the
first three targets, `serve_mcp.RustTools.call_tool`, is the same shape (a
trait method the MCP library calls). They are labelled
`dispatched_method` instead, which tells the agent that something outside
the graph calls them.

## Result

<!-- derived-from #the-question-measured -->

`flow_tool(mode="entry_points")` on this repository at `398b306f`, in
`minimal`. The search stops at the nearest entry point on each path: with
every entry reported, `assess_graph_sync` returned 15, ten of them CLI
handlers that reach it through the Python-to-Rust `call_tool` bridge,
which is itself an entry point.

| Target | Callers reached | Entry points | Output (chars) | Entry points found |
|---|---|---|---|---|
| `assess_graph_sync` | 10 | 4 | 3,836 | `call_tool` (FFI), `RustTools.call_tool` (dispatched), `main` (`dagayn status`), `run_cli` (FFI) |
| `is_production_code` | 22 | 2 | 3,404 | `call_tool` (FFI), `RustTools.call_tool` (dispatched) |
| `GraphStore.get_flow_edge_data` | 42 | 7 | 5,841 | the two Python bindings, `call_tool`, `RustTools.call_tool`, `main`, `run_cli` |
| `node_text` | 833 | 11 (10 shown) | 7,994 | 3 dispatched visitor methods, 6 FFI parse entry points, `main` |

Each search takes 0.15–0.2 s and reads no stored flow. On the eval set
(13 cases, 3 negative) every gated kind has precision and recall 1.00,
every chain hop is a graph edge, and the output is 3.2–3.8K characters.

## Modes

| Mode | Returns |
|---|---|
| `entry_points` with `target` | the nearest entry points that reach the target, 10 by default: `entry_point`, `kind`, `chain` (qualified names in call order), `hops`, and in `standard` `file` and `line`; `entry_points_omitted`, `reached_callers`, `truncated` |
| `list` without a target (step 4) | entry points grouped by unit and kind, with counts; no members, no score |
| `get` | deprecated: one stored flow, `detail_level` honoured (`minimal` drops `steps` and `path`), for one release |

Entry kinds, from the first rule that holds:

| Kind | The node |
|---|---|
| `main` | is named `main` or `__main__` |
| `framework_handler` | has a framework decorator the flow trace knows (`@app.get`, `@click.command`, `@mcp.tool`, …) |
| `ffi_export` | is exported across a language boundary (`extra.ffi_export`) |
| `named_entry` | has a conventional entry name (`handle_*`, `on_*`, `lambda_handler`, …) |
| `dispatched_method` | is a method nothing in the graph calls: a trait, interface, or framework calls it |
| `uncalled` | is a function nothing in the graph calls |
| `module_level` | is a file whose top-level code makes the call |

A node with a caller anywhere in the graph, tests included, is
`uncalled` only if one of the first four rules names it; so a helper that
only tests call has no entry point, and its test callers are never walked.

`review_tool(mode="affected_flows")` becomes the same search run from each
changed function: which entry points the change reaches.

## Fate of the stored flows

- `criticality` and its ranking go. `get_minimal_context_tool` drops
  `top_flows` and `flows_affected`, and answerability drops the
  `missing_flows` penalty, because the new modes need no stored data.
- `flow_snapshots` goes; nothing reads it.
- Wiki and visualization switch to entry points per unit. After that
  nothing reads the `flows` table, and the full postprocess drops the flow
  trace.

## Decisions (2026-10-07)

- The target contract above goes ahead: `flow_tool` answers which entry
  points reach a symbol; the eval gates it before the stored flows go.
- Wiki and visualization switch to entry points per unit in step 5. No
  known consumer depends on the per-community "Execution Flows" section;
  the section keeps its heading for one release so links to it resolve.

## Evaluation

A harness beside `eval/run_architecture_eval.py`, run in CI on fixed
fixtures:

`eval/run_flow_eval.py`, gated in CI by `tests/test_flow_eval.py` with the
floors in `eval/flow_thresholds.yaml`; cases live in
`tests/fixtures/flow_eval`. Graphs are built with `--skip-flows`, so the
eval also shows the mode needs no stored flows.

- **Negative cases**: a function called only from tests; one whose only
  caller is called only from tests; one called by a `main` under
  `tests/fixtures/`. Each expects no entry points.
- **Seeded positives**: a Python `main`, a `@click.command`, a FastAPI
  route, an `@mcp.prompt` function whose target is named like an entry
  (`render`, which must not be listed itself), a `handle_*` function called by `main` (only the handler is
  expected), a module-level script, a Rust `main`, a Rust trait method
  (expected as `dispatched_method`), an uncalled TypeScript export, and two
  entry points reaching one target.
- **Reported per run:** precision and recall per entry kind; any kind or
  expected chain that differs; every chain hop without a `CALLS` or
  `CROSS_ARTIFACT` edge in the built graph; output size (p50/max). Each
  kind gates at 0.8, and a mismatch or a bad hop fails the gate.
  `ffi_export` is reported but not gated until a fixture with a native
  bridge exists.

## Order of work

1. **Done:** bound today's output. `list` drops each flow's `path` and
   `members` id arrays and its `files` list (`file_count` stays), and
   `minimal` names the `entry_point`; `get` honours `detail_level`, and
   `minimal` returns the first 50 steps as qualified name and line with
   `steps_omitted`. On this repository `list` went from 292,909 to 21,115
   characters and `get flow_name=main` in `minimal` from 177,111 to 7,698.
2. **Done:** eval harness with the cases above.
3. **Done:** `entry_points` mode on the query-time reverse search
   (`crates/dagayn-tools/src/entry_points.rs`), reusing the flow trace's
   entry-point rules and edge set; methods without a static caller are
   labelled `dispatched_method`, not dropped.
4. **Done:** `review_tool(mode="affected_flows")` runs the search from
   every changed function (the functions whose lines the diff touches, or
   every function of a file without a diff range) and returns
   `entry_points`; the stored flows that contain them move behind
   `detail_level="verbose"`, listed in `deprecated_fields`.
   `get_minimal_context_tool` no longer names `top_flows` or
   `flows_affected`: they were names without a file, ranked by size, and
   the tool has no verbose level to keep them behind. The debug, explore,
   implement-feature, and review skills, the MCP prompts, the agent
   instructions, and the next-step hints point at `entry_points`.
5. `list` without a target lists entry points per unit and kind; wiki and
   visualization switch to it. Then `get`, criticality, `flow_snapshots`,
   the `missing_flows` answerability penalty, and the flow trace in the
   full post-process go, after one release with `list` and `get` kept.

## Touch points

- Code: `crates/dagayn-tools/src/flow.rs`, `context.rs`, `review.rs`,
  `answerability.rs`; `crates/dagayn-graph/src/flow_trace.rs`, `flows.rs`,
  `summary_flows.rs`; `dagayn/tools/flow_dispatcher.py`,
  `dagayn/contracts/state_types.py`, `dagayn/wiki.py`,
  `dagayn/visualization/data.py`.
- Docs: `docs/COMMANDS.md`, `docs/USAGE.md`,
  `docs/LLM-OPTIMIZED-REFERENCE.md`, `docs/SESSION-GRAPH-FRESHNESS.md`,
  the MCP tool description, `dagayn/prompts.py`,
  `dagayn/skills/instructions.py`, and the debug-issue, explore-codebase,
  implement-feature, review-pr, and build-graph skills.
- Tests: flow cases in `crates/dagayn-tools/tests/tools.rs`,
  `tests/test_flows.py`, `tests/test_review_flow_dispatchers.py`, and the
  MCP parity snapshots.
