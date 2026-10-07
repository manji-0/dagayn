# What an agent workflow over dagayn's tools is

<!-- constrained-by ./ANALYSIS-TOOL-STRATEGY.md#tool-tiers -->
<!-- constrained-by ./ANALYSIS-TOOL-STRATEGY.md#selection-rules-for-new-analyses -->
<!-- constrained-by ../COMMANDS.md#mcp-tools -->

## Question

The four TARGET notes ([review](./REVIEW-TOOL-TARGET.md#target-contract),
[architecture](./ARCHITECTURE-TOOL-TARGET.md#target-contract),
[flow](./FLOW-TOOL-TARGET.md#target-contract),
[refactor](./REFACTOR-TOOL-TARGET.md#target-contract)) each fixed what one
tool answers. None of them fixed what happens *between* calls: how an agent
gets from the first call to a confirmed answer, and what every response owes
it to take the next step. Does the default surface support one workflow an
agent can follow, and what should it be?

Verdict: **the tools answer their own questions, but the seams between
them cost an agent calls, context, and guesses.** The drill-down modes the
skills send agents to return 300K–480K characters whatever `detail_level`
says; "what to call next" is spelled in seven fields across five shapes;
one reason code is attached to every reply on a fresh graph; and an
ambiguous name costs a round trip with no hint of how to retry. This note
defines the workflow, the response contract that carries it, the evidence,
and the order of work.

Status: accepted ([decisions](#decisions-2026-10-07)); step 1 of the
[order of work](#order-of-work) is done.

## Target contract

> An agent reaches a confirmed answer by **orient → locate → read → trace
> → judge → confirm**, and every Tier 1 response carries what the next
> phase needs: one `next` list of runnable calls, an answer that fits its
> budget at every `detail_level`, and caveats only where they change how
> this answer reads. New behaviour is a field or a mode, never a new tool.

The phases:

| Phase | Question | Tool | Leaves the agent with |
|---|---|---|---|
| Orient | Can I trust the graph, and where do I start? | `get_minimal_context_tool` (`ensure_graph_tool` when it says so) | `sync.state`, and `next` built from the task |
| Locate | Which node is this? | `semantic_search_nodes_tool` | one `qualified_name` |
| Read | What does it do? | `query_graph_tool(source_of)` | the live span |
| Trace | What reaches it, and what does it reach? | `query_graph_tool` (`callers_of`, `callees_of`, `tests_for`, `docs_for`), `flow_tool(entry_points)` | the nodes worth reading next |
| Judge | What must I check that I cannot see? | `review_tool(changes)`, `architecture_analysis_tool(overview)`, `refactor_tool(suggest)` | `findings`, each with a place |
| Confirm | Is the claim true? | `source_of` on each finding's place; a test or a reproduction | a claim at the right [trust tier](../LLM-OPTIMIZED-REFERENCE.md) |

A task enters at the phase it needs: a review starts at Judge, a bug at
Locate, a rename at Trace. After an edit the loop closes at Judge again
(`review_tool(mode="changes")`).

Consequences of that sentence:

- **One `next` field.** Every Tier 1 response has `next`: at most three
  `{tool, args, why}` items whose `args` are complete JSON the agent can
  pass unchanged (a `qualified_name`, not "the chosen node"). It replaces
  `recommended_action`, `next_tool_suggestions`, `next_action`,
  `exactness.next_action`, `guidance[].action`, `next_drill_downs`, and
  `_hints.next_steps`, which move behind `detail_level="verbose"` for one
  release, listed in `deprecated_fields`. An empty `next` means the answer
  is complete.
- **A budget per level, for every mode.** `minimal` fits in 8K characters
  (about 2K tokens) and `standard` in 32K, for any input; what does not fit
  is counted (`omitted`, `truncated`, `total`), never dropped silently.
  The budget covers every field, `missingness` included.
- **Caveats that discriminate.** Graph-wide health lives in
  `get_minimal_context_tool`. Other tools carry a `missingness` item only
  when it limits *this* answer, and per-item caveats of one kind are one
  item with a count. A reason code present on every reply of a healthy
  graph is dropped or recalibrated, by the same rule that removed the
  review risk score.
- **Ambiguity names its retry.** `status="ambiguous"` returns the
  candidates (non-test first) and a `next` with one call per candidate,
  `target` set to its `qualified_name`.
- **Counts agree.** A response states one number for one thing:
  `summary`, `result_count`, and any `counts` match.
- **One source of truth for the workflow.** The phases live in one
  section of `docs/LLM-OPTIMIZED-REFERENCE.md`; the skills, the generated
  instruction files (`dagayn/skills/instructions.py`), and the MCP server
  instructions quote or point at it, and a test fails when they drift.

## Evidence: the current output

Measured on this repository at `c8797b7d` (16,986 nodes, `commit_synced`)
with `dagayn tool`, the same implementation the MCP server runs.

### Size

| Call | `detail_level` | Characters |
|---|---|---|
| `get_minimal_context_tool` | — | 1,073 |
| `semantic_search_nodes_tool`, limit 5 | `standard` / `minimal` | 6,076 / 5,073 |
| `query_graph_tool callers_of` (qualified) | `standard` | 5,637 |
| `flow_tool entry_points` (qualified) | `standard` | 5,633 |
| `review_tool changes`, `base=HEAD~3` | `standard` / `minimal` | 36,327 / 8,320 |
| `review_tool impact`, `base=HEAD~3` | `standard` / `minimal` | 304,048 / 302,203 |
| `review_tool context`, `base=HEAD~3` | `standard` | 479,434 |
| `architecture_analysis_tool overview` | `minimal` | 7,046 |
| `refactor_tool suggest` | `minimal` | 9,642 |

The Judge tools fit their budgets since their redesigns. The drill-downs
they point at do not: `impact` at `minimal` is 302K characters, 245K of it
`missingness`, which holds 587 `low_confidence_cross_artifact_bridge`
items, one per Markdown mention (README translations, `GEMINI.md`) of a
changed symbol. `context` is 371K of `context`; its `_truncation` caps the
node lists at 300 but not the text. The debug and review skills send an
agent to both "when the blast radius is still unclear".

In `semantic_search_nodes_tool`, `minimal` saves 1K of 6K: the
`embedding_health` and `answerability` blocks are the same at every level.

### Next step

| Tool | Fields that say what to call next |
|---|---|
| `get_minimal_context_tool` | `recommended_action` (prose), `next_tool_suggestions` (names) |
| `semantic_search_nodes_tool` | `next_action`, `exactness.next_action`, `guidance[].action`, `_hints.next_steps`: the same step four times |
| `query_graph_tool` | `next_action`, `guidance[].action`; no `_hints` |
| `review_tool` | `_hints.next_steps`, `next_drill_downs` |
| `flow_tool`, `architecture_analysis_tool`, `refactor_tool` | `_hints.next_steps` |

None of them carries arguments: "fetch live source for the chosen
qualified_name" leaves the agent to fill the target in. The orient call
names tools (`["review_tool", "flow_tool", "query_graph_tool"]`), not
calls.

### Caveats

Every reply above carries the full `answerability` block (about 540
characters, including the unlabelled tuple `[373, 553, 11194, 1737,
0.0006]`) and a medium-severity `stale_derived_structures` item ("claims
should be treated as graph-limited"). It fires because
`unassigned_nodes > 0` (`crates/dagayn-tools/src/answerability.rs`): 2,718
non-File nodes have no community, among them 967 functions, 619 tests,
and 832 Markdown nodes. The graph matches HEAD; the item fires on every
call and so tells the agent nothing.

### Ambiguity

`callers_of make_response` returns `status="ambiguous"` with the Rust and
Python candidates and no next step. `flow_tool entry_points` on the same
name returns the same candidates with three generic hints
("see the architecture overview"), none of them the retry.

### Counts

`callers_of dagayn/tools/_common.py::make_response` says "Found 11
result(s)" and `result_count: 11`; its `guidance` claims "returned 13
related node(s)" with `counts.result_count: 13`.

## Decisions (2026-10-07)

- The target contract goes ahead in the order of work below, starting
  with the drill-down budgets.
- The seven old next-step fields stay behind `detail_level="verbose"`,
  listed in `deprecated_fields`, for one release, as the review and flow
  fields did.
- Budgets: `minimal` 8K characters, `standard` 32K, counted on compact
  JSON as the MCP transport sends it; `verbose` is outside the contract.
  A tool may keep a cap of its own at `verbose` (`impact` keeps its 32K:
  without it one call on this repository was 1.5M characters).
- An ambiguous target is not answered per candidate: the reply names
  the retries in `next` and runs nothing.

## Evaluation

`eval/run_workflow_eval.py`, gated in CI by `tests/test_workflow_eval.py`
with floors in `eval/workflow_thresholds.yaml`, on the fixtures the other
evals already build.

- **Contract check**: every Tier 1 tool, every mode, every
  `detail_level`, on each fixture and on a large synthetic diff. Fails on
  a reply over budget, a reply without `next`, a `next` item whose `args`
  the tool's schema rejects, or a `summary` count that differs from
  `result_count`.
- **Follow-the-next traces**: fixed tasks (find the caller that breaks
  after a signature change; find the entry point of a CLI command; review
  a diff with a dangling reference; resolve an ambiguous name), each run
  by following `next[0]` from `get_minimal_context_tool(task=...)` for at
  most 6 calls. Reported per task: whether the expected node or finding
  is reached, calls to reach it, the largest reply, ambiguity retries.
  Each task gates on being reached; the call and size counts gate at the
  baseline the first run records.
- **Discrimination**: on a fresh fixture graph no `missingness` item of
  severity medium or above appears; on a stale or partial one the
  expected items do.

## Order of work

1. **Done:** budget the drill-downs. `impact` folds its low-confidence
   bridges into one `missingness` item with a `count` and three
   `examples` (the full list stays in `low_confidence_bridges`, which the
   budget trims). `context` caps source at 16K bytes below `verbose`
   (120K at `verbose`, as before) and halves its nested lists
   (`context.graph.edges` first) until the reply fits; the output budget
   now reads `a.b.c` paths and merges into an existing `_truncation`.
   On this repository with `base=HEAD~3` (compact characters):

   | Call | Before | After |
   |---|---|---|
   | `impact`, `minimal` | 302,203 | 6,423 |
   | `impact`, `standard` | 304,048 | 23,775 |
   | `context`, `standard` | 479,434 | 32,202 |

   `tests/tools.rs::review_context_fits_its_budget_below_verbose` holds
   `context` to the budget; the contract check over every mode moves to
   step 6. The halving is blunt (`context` keeps 4 of 954 changed nodes
   and one clipped file): which nodes are worth the budget is a question
   for step 4's `next`, which can point at them instead.
2. Recalibrate the caveats: `stale_derived_structures` fires only on
   stale flow memberships or on code nodes left out of a community run
   that should have covered them; `answerability` moves to
   `get_minimal_context_tool` and `verbose`; other tools keep the
   `missingness` items that apply to them.
3. Ambiguity: candidates ordered non-test first, `next` with one call
   per candidate, in `query_graph_tool` and `flow_tool`, sharing one
   resolver.
4. `next`: one builder in `crates/dagayn-tools/src/hints.rs`; every Tier 1
   tool returns it; the old fields go behind `verbose` with
   `deprecated_fields`. `get_minimal_context_tool` fills `args` from the
   task (a search query for a symptom, `base` for a review).
5. Counts: `query_graph_tool` states one number.
6. Eval: follow-the-next traces and the discrimination check, gated.
7. One source of truth: a `workflow` section in
   `docs/LLM-OPTIMIZED-REFERENCE.md`; skills, instruction files, and MCP
   server instructions derive from it, with a drift test.
8. After one release: remove the deprecated next-step fields.

## Touch points

- Code: `crates/dagayn-tools/src/hints.rs`, `answerability.rs`,
  `context.rs`, `review.rs`, `query.rs`, `search.rs`, `flow.rs`,
  `entry_points.rs`, `arch_tool.rs`, `refactor.rs`, `lib.rs`;
  `crates/dagayn-graph/src/maintenance.rs`; `dagayn/tools/_common.py`;
  `dagayn/skills/instructions.py`.
- Docs: `docs/LLM-OPTIMIZED-REFERENCE.md`, `docs/COMMANDS.md`,
  `docs/USAGE.md`, `skills/*/SKILL.md`, the MCP server instructions.
- Earlier plan: [the remediation plan's shared guidance contract](./DAGAYN-FEATURE-INTERFACE-REMEDIATION-PLAN.md#step-1-shared-guidance-contract)
  added `guidance` items and `missingness`; this note keeps their claim
  and evidence and replaces their `action` with `next`.
