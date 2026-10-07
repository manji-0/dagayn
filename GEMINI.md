# dagayn integration notes for Gemini-family tooling

`dagayn` is the documented product name for this fork. Upstream `code-review-graph` compatibility remains in code, but docs and examples in this repo should speak in terms of `dagayn`.

Recommended workflow:

1. run `dagayn install` in the repository you want to wire up
2. build the graph with `dagayn build`
3. use MCP tools such as `get_minimal_context_tool`, `review_tool`,
   `query_graph_tool`, and `flow_tool`
4. refresh with `dagayn update` or `dagayn watch`

The fork is especially useful in repositories that mix application code, docs, and Terraform.

MCP responses are evidence-ranked leads, not verdicts. Prefer `next`,
`missingness`, and `zero_result_reason` before
falling back to legacy raw fields. Documentation bridge results distinguish
`authored`, `extracted`, and `heuristic_reachable` evidence.

<!-- dagayn MCP tools -->
## MCP Tools: dagayn

**This project has a dagayn knowledge graph. Use the dagayn MCP tools before
grep, glob, or whole-file reads to explore the codebase.** The graph is
cheaper and gives structural context (callers, dependents, tests, linked
docs) that file scanning cannot. Fall back to file search only when a graph
result is missing, stale, ambiguous, or truncated, or `source_of` cannot
supply the span.

### Workflow

<!-- dagayn workflow -->
Orient → locate → read → trace → judge → confirm. Enter at the phase the task
needs, and close the loop after an edit with `review_tool`.

1. **Orient**: `get_minimal_context_tool(task=...)` reports `sync.state` and
   the first calls for the task.
2. **Locate**: `semantic_search_nodes_tool` turns a description into a
   `qualified_name`.
3. **Read**: `query_graph_tool(pattern="source_of")` returns the live span.
4. **Trace**: `query_graph_tool` (`callers_of`, `callees_of`, `tests_for`,
   `docs_for`) and `flow_tool(mode="entry_points")`.
5. **Judge**: `review_tool`, `architecture_analysis_tool`, or
   `refactor_tool` answer with `findings`; an empty list means nothing to act
   on.
6. **Confirm**: `source_of` on each finding's place, a test, or a
   reproduction.

Every reply ends with `next`: at most three calls with complete arguments and
a `why`. Follow it unless the task points elsewhere; `[]` means the answer is
complete. `status="ambiguous"` puts one retry per candidate in `next`, and
`missingness` lists only the gaps that limit that reply. Full contract:
`get_docs_section_tool(section_name="workflow")`.
<!-- /dagayn workflow -->

Per task: a review starts at `review_tool(mode="changes")` (read `findings`
before any drill-down); a bug or a feature at `semantic_search_nodes_tool`;
structure at `architecture_analysis_tool` (`units`, `unit_edges`,
`findings`); a rename at `refactor_tool(mode="rename")`, applied with
`apply_refactor_tool` in the same `dagayn serve` session. Pass `depth` to
callers_of/importers_of for a transitive chain.

### Default tools

| Tool | Use when |
| ------ | ---------- |
| `get_minimal_context_tool` | Start here: freshness and the first calls |
| `ensure_graph_tool` | Graph empty or behind HEAD; bootstrap without embeddings |
| `review_tool` | Change review: what to check that the diff does not show |
| `query_graph_tool` | Callers, callees, imports, tests, linked docs, live source spans |
| `semantic_search_nodes_tool` | Find code or doc sections by name, keyword, or meaning |
| `flow_tool` | Entry points that reach a symbol, with the call chain |
| `architecture_analysis_tool` | Map of declared units and structural findings |
| `refactor_tool` | Refactor suggestions, dead code, rename previews |
| `get_docs_section_tool` | dagayn reference sections, e.g. `trust` |

Drill-down tools: `review_tool(mode="impact" | "affected_flows" |
"context")` and `architecture_analysis_tool(mode=...)`. `dagayn serve --tools
all` (or `CRG_TOOLS`) exposes the advanced and maintenance tools.

### How to judge analysis output

<!-- dagayn trust tiers -->
Reach comes from the graph, correctness from `source_of`, and user-visible
effect from a reproduction; keep them apart in a claim. Full rules:
`get_docs_section_tool(section_name="trust")`.

- **Highest** — on a current graph (`sync.state` is `commit_synced` or
  `worktree_ahead`): `HIGH` and `EXTRACTED` edges, whether the parser or a
  SCIP index (`resolved_by: "scip"`) settled them; `source_of` spans;
  authored doc contracts (`implemented_by` / `implements_contract` links whose
  target exists — `evidence_type=authored` alone is on every Markdown result).
- **Medium** — structure, not correctness: `MEDIUM` (inferred) edges,
  `reason_codes`, blast radius, flows, communities, metrics, suggestions,
  explanatory doc links, and search hits.
- **Low** — a hypothesis until `source_of` or a reproduction confirms it:
  `LOW` edges, `heuristic_reachable` doc links whatever their edge confidence,
  a file-level `tests_for` of 0, `truncated` or `ambiguous` results, and
  answers about files changed since the graph was built (`sync.state`
  `commit_drift` or `worktree_behind`).
<!-- /dagayn trust tiers -->

- Cite the counts, thresholds, reason codes, and `truncated`/`total` fields
  behind a recommendation; narrow a truncated result with `top_n`,
  `detail_level`, or a targeted query before concluding.
- Check `query_graph_tool(pattern="tests_for")` before calling code
  untested; a file-level zero is Low trust, not proof.
- Before a refactor, check public APIs, dynamic dispatch, generated code,
  and framework entry points.

<!-- dagayn markdown policy -->
## Markdown documentation policy: declare dependencies via directive comments

In Markdown that dagayn indexes, declare a real dependency on another section
or document with an HTML comment directly under the dependent heading, so the
graph records it (`DEPENDS_ON` / `IMPORTS_FROM`) and impact analysis sees it:

```markdown
<!-- <kind> <target> -->
```

- `<kind>`: `constrained-by` (bounded by the target), `blocked-by` (cannot
  proceed until it resolves), `supersedes` (replaces it; place it in the new
  document), or `derived-from` (built from it).
- `<target>`: `#section-slug` (same document), `./path.md`, or
  `./path.md#slug`. Slugs follow GitHub rules: lowercase, punctuation
  removed, spaces collapsed to `-`. External URLs stay ordinary links.
- No real dependency, no directive. The `writing-markdown-document` skill
  has the full rules.
