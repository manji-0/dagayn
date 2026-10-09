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

Status: accepted ([decisions](#decisions-2026-10-07)); steps 1–7 are
done; step 8 waits one release (see the [order of work](#order-of-work)).

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
  `exactness.next_action`, `next_drill_downs`, and `_hints`, which move
  behind `detail_level="verbose"` for one release, listed in
  `deprecated_fields`. `guidance[].action` stays, as each guidance item's
  own description ([decisions](#decisions-2026-10-08)). An empty `next` means the answer
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

## Decisions (2026-10-08)

- `_hints` goes behind `verbose` whole: its `warnings` repeat
  `missingness` codes and its `related` is nearly always empty.
- `guidance[].action` stays, as each guidance item's own description;
  `next` is the list to run.
- Later the same day: below `verbose` a reply drops what is diagnosis or
  restatement. `_runtime`, `called_subtool`, the whole of `guidance` (its
  claim is `summary`, its counts the reply's own, its caveats
  `missingness`, its action `next`), and search's `embedding_health` go
  to `verbose`; `_repo` keeps only `repo_root`. This replaces the
  decision above to keep `guidance` in the default reply.

## Evaluation

Three gates, all in `crates/dagayn-tools/tests/tools.rs`, run in CI on
fixtures the tests build.

- **Contract check**: `every_reply_names_calls_that_answer_and_fits_its_budget`.
  26 calls across the six tools, their modes, and their levels, on a
  fixture with a 120-file change. Fails on a reply over its level's
  budget, a reply without `next` or with more than three calls, a call
  without a `why`, or a call (but a shell command) that answers with an
  error, an ambiguity, or a missing node: making each call is what shows
  its `args` complete and its targets real. Counts that disagree are
  held by `query_graph_counts_one_row_per_node_everywhere`.
- **Follow-the-next traces**: `following_next_reaches_the_answer`. From
  the first call of a task, following `next[0]` (but a shell command)
  must reach the answer within six calls: a symptom ("debug why main
  fails") reaches `callers_of app.py::main`; an ambiguous name answers
  on its first retry; a review of an untested addition reaches the
  `untested_change` finding. Call counts and sizes are not gated beyond
  the six-call limit and the contract check's budgets.
- **Discrimination**: `a_fresh_graph_raises_no_caveats`. On a graph that
  matches its commit, no reply of the six tools carries a medium or high
  caveat about the graph (staleness, derived structures, another commit,
  unindexed edits, an older extractor, unresolved bridges).

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
2. Recalibrate the caveats.
   - **Done:** the 2,718 unassigned nodes are real: all of them sit in
     files changed since the last full post-process, which an
     incremental update does not re-cluster. The code is right; its
     audience was wrong. `missing_flows`, `missing_communities`, their
     `_table` variants, and `stale_derived_structures` are `missingness`
     only on answers read from communities or stored flows (the legacy
     community and analysis modes of `architecture_analysis_tool`, and
     `flow_tool` `list` / `get`); `graph_health.reason_codes` still
     reports them. In the MCP snapshots 69 items went, nothing else
     changed.
   - **Done:** `answerability` leaves the six Tier 1 analysis tools
     below `detail_level="verbose"` (`"full"` for `query_graph_tool`) and
     every error; `get_minimal_context_tool` keeps it as `graph_health`.
     One filter in `dagayn_tools::call` and its Python twin
     `summary_at_verbose_only` (the MCP wrappers and the Python search)
     do it, so no tool body changed. The overview's unit map no longer
     copies the community report's derived-structure gaps. In the
     snapshots 75 files lost only `answerability` and derived-structure
     items. On this repository (compact characters, `minimal`): search
     5,073 → 3,630, `callers_of` 4,368 (it had a compact block already),
     architecture overview 7,046 → 5,143.
   - Found while measuring: `review_tool(mode="changes")` at `minimal`
     is 20,888 characters with `base=HEAD~3` once the diff spans 75
     snapshot files; its budget is 32K at every level. The contract check
     in step 6 holds it to 8K.
3. **Done:** ambiguity. `query_graph_tool` and `flow_tool`
   (`entry_points`) order candidates production code first and answer
   with `next`, the first three as calls (`crates/dagayn-tools/src/next.rs`
   builds them, the first piece of step 4's builder). A retry keeps the
   arguments that shape the answer (`pattern`, `depth`, `mode`, `limit`,
   `detail_level`) only where they differ from the default, so it reads
   the same whichever server filled the defaults in. `flow_tool`'s
   `_hints` now name the retries instead of the architecture overview.
   On this repository `callers_of make_response` returns the two calls
   to make, one per `make_response`. The two resolvers stay separate:
   `query_graph_tool` falls back to fuzzy hits when no name matches,
   which `flow_tool` should not.
4. `next`.
   - **Done:** every Tier 1 reply has `next`
     (`crates/dagayn-tools/src/next.rs`; `[]` where nothing follows, added
     in `dagayn_tools::call` and `summary_at_verbose_only`).
     `get_minimal_context_tool` starts the workflow (`ensure_graph_tool`
     first on an empty or drifted graph; a review, the search for the
     task's text, the overview, or refactor findings); search reads its
     first hits (a file's `file_summary`); `query_graph_tool` reads what
     it found, or who calls a span it read; `flow_tool` reads the entry
     points; review, architecture, and refactor make their first findings
     runnable (a `tests_to_run` command as `{"tool": "shell"}`). The
     Python search builds the same list. Arguments at their defaults are
     left out.
   - **Done:** the earlier fields go behind `verbose`
     ([decisions](#decisions-2026-10-08)). `dagayn_tools::seal_reply`
     (and its Python twin, now idempotent so a reply Rust sealed passes
     through Python unchanged) drops `next_action`,
     `exactness.next_action`, `next_drill_downs`, `next_tool_suggestions`,
     and `_hints` below `verbose` and names the ones present in
     `deprecated_fields` at `verbose`. `get_minimal_context_tool` has no
     `verbose`, so its `recommended_action` and `next_tool_suggestions`
     go now, as its `top_flows` did; what only they said moved into
     `why` (a queued repair) or `next`. Two things only the old fields
     said moved into `next` first: a transitive query stopped at its
     `depth` puts the same call at `depth=6` first, and a rename preview
     names `apply_refactor_tool` with `dry_run`. In the snapshots 82
     files changed, and nothing but these fields. On this repository
     `get_minimal_context_tool` went from 976 to 804 characters and
     search at `minimal` from 4,005 to 3,569.
5. **Done:** counts. Below `full`, `query_graph_tool` folds a node's
   edges into one row, and `guidance` counted the edges before the fold:
   `callers_of make_response` said 11 rows and 13 nodes. `guidance` now
   counts the rows; `full`, which lists one row per edge, still counts
   edges, consistently with its own `result_count`.
6. Eval.
   - **Done:** the contract check,
     `tests/tools.rs::every_reply_names_calls_that_answer_and_fits_its_budget`:
     26 calls across the six tools and their levels on a fixture with a
     120-file change, each reply held to its budget and to at most three
     `next` calls, and every call it names (but a shell command) made and
     required to answer without an error, an ambiguity, or a missing
     node. It found five replies over budget, all from lists of paths a
     long diff repeats (`changed_files`, `unmapped_changed_files`,
     `change_file_sources`, `source_snippets_omitted`), and two levels
     with no budget at all (`context` and `affected_flows` below
     `verbose`). The paths are now trimmed first; `minimal` and
     `standard` share `MINIMAL_BUDGET` / `STANDARD_BUDGET`, 250 tokens
     under 8K and 32K characters for what a reply adds after trimming;
     `query_graph_tool` at `minimal` went from 16K to 8K. On this
     repository with `base=HEAD~3` every reply fits (`changes` at
     `minimal` 18,291 → 7,209, `context` 47,770 → 30,906), and all 21
     calls the replies name answered `ok`.
   - **Done:** the follow-the-next traces and the discrimination check
     (see [Evaluation](#evaluation)), as tests beside the contract check
     rather than a separate harness: they need no thresholds file, since
     each task either reaches its answer or fails. The first run of the
     traces failed: `get_minimal_context_tool` searched for the whole
     task, and search requires a query's most selective word, so "debug
     why main fails" searched for "fails" and found nothing. The search
     call now drops the words that route the task and the words that say
     how to work (`why`, `fails`, `please`), searching for "main". The
     discrimination check passed on its first run, which step 2 had
     made true.
   - Not covered: a Python-side dispatcher error reached through
     `dagayn tool` (not MCP) still carries `answerability`; the contract
     is the MCP surface's.
7. **Done:** one source of truth. `dagayn/skills/workflow.py` holds the
   phases and the `next` rule, as `trust.py` holds the trust tiers; the
   `workflow` section of `docs/LLM-OPTIMIZED-REFERENCE.md` (with the reply
   contract: arguments, budgets, `missingness`, the deprecated fields),
   the installed instructions (`_CLAUDE_MD_SECTION`, refreshed in
   `AGENTS.md` and `GEMINI.md`) carry it verbatim, and
   `tests/test_skills.py::test_workflow_is_written_once` fails when a copy
   drifts. Skills carry no copy (2026-10-10): the instructions load it in
   every session, so a copy in a skill was read twice. The instructions
   dropped the tool table, which restated the tool descriptions, and the
   trust tiers, which skills carry where a judgment is made and
   `get_docs_section_tool(section_name="trust")` has in full. The MCP server instructions name `next` and
   the `workflow` section in two sentences, inside the default
   descriptions' 8,500-character budget.
8. After one release: remove the next-step fields `verbose` still
   carries, and the hint engine that builds `_hints`.
9. **Done:** the envelope. `dagayn_tools::trim_envelope` (and its
   Python twin) applies the decision of 2026-10-08 below `verbose` and
   to every `get_minimal_context_tool` reply; search accepts
   `detail_level="verbose"` natively. The MCP text was already minified
   JSON on every path, so the size went down only by fields. On this
   repository (compact characters): search at `minimal` 3,569 → 2,075,
   `flow_tool entry_points` 4,175 → 3,236, `source_of` 3,076 → 2,421,
   `get_minimal_context_tool` 804 → 734. The parity tests compared the
   reply text only when `_runtime` was absent; they now compare it as
   JSON, since serde_json sorts nested keys and Python does not. A
   review at `minimal` also stops computing the score-first summary it
   no longer shows (1.93 s → 1.72 s on this repository).

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
