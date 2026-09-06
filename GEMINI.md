# dagayn integration notes for Gemini-family tooling

`dagayn` is the documented product name for this fork. Upstream `code-review-graph` compatibility remains in code, but docs and examples in this repo should speak in terms of `dagayn`.

Recommended workflow:

1. run `dagayn install` in the repository you want to wire up
2. build the graph with `dagayn build`
3. use MCP tools such as `get_minimal_context_tool`, `review_tool`,
   `query_graph_tool`, and `flow_tool`
4. refresh with `dagayn update` or `dagayn watch`

The fork is especially useful in repositories that mix application code, docs, and Terraform.

MCP responses are evidence-ranked leads, not verdicts. Prefer `guidance`,
`answerability`, `missingness`, `zero_result_reason`, and `next_action` before
falling back to legacy raw fields. Documentation bridge results distinguish
`authored`, `extracted`, and `heuristic_reachable` evidence.

<!-- dagayn MCP tools -->
## MCP Tools: dagayn

**IMPORTANT: This project has a knowledge graph. ALWAYS use the
dagayn MCP tools BEFORE using Grep/Glob/Read to explore
the codebase.** The graph is faster, cheaper (fewer tokens), and gives
you structural context (callers, dependents, test coverage) that file
scanning cannot.

### When to use graph tools FIRST

- **Any new task**: `get_minimal_context_tool` for graph freshness, risk, and next-tool hints
- **Exploring code**: `semantic_search_nodes_tool` or `query_graph_tool` instead of Grep
- **Understanding impact**: `review_tool(mode="impact")` instead of manually tracing imports
- **Code review**: `review_tool(mode="changes")` first; use its `analysis_summary` before
  calling drill-down tools
- **Finding relationships**: `query_graph_tool` with
  callers_of/callees_of/imports_of/tests_for/source_of
- **Architecture questions**: `architecture_analysis_tool(mode="overview")`
  first; use `architecture_health` and the Architecture Analysis skill before
  choosing a drill-down mode

Fall back to Grep/Glob/Read **only** when the graph result is missing, stale,
ambiguous, truncated, or `source_of` cannot supply the span. Do not re-read a
whole file just to inspect a function the graph already located.

### Tool surface

`dagayn serve` exposes the compact workflow tool surface by default. Use
`dagayn serve --tools ...` when a deployment needs an exact allow-list; the same
allow-list can be supplied with `CRG_TOOLS`. Use `all`, `full`, or `*` to expose
advanced/maintenance tools.

### Default workflow tools

| Tool | Use when |
| ------ | ---------- |
| `get_minimal_context_tool` | Start here: graph freshness, risk, communities, next tools |
| `ensure_graph_tool` | Empty or missing graph; safe bootstrap without embeddings |
| `review_tool` | Primary change review and review drill-down dispatcher |
| `flow_tool` | Reachable-set flow lists and BFS membership (not call sequences) |
| `architecture_analysis_tool` | Primary architecture review and drill-down dispatcher |
| `refactor_tool` | Planning renames, finding dead code, and evidence-ranked refactor suggestions |
| `query_graph_tool` | Tracing callers, callees, imports, tests, live source spans |
| `semantic_search_nodes_tool` | Finding functions/classes by name or keyword |

### Drill-down tools

| Tool | Use when |
| ------ | ---------- |
| `review_tool(mode="impact")` | Need a wider or deeper blast-radius view |
| `review_tool(mode="affected_flows")` | Need full affected execution-path details |
| `architecture_analysis_tool(mode=...)` | Architecture drill-downs for boundaries and metrics |

### How to judge analysis output

- Treat graph insights as **evidence-ranked leads**, not automatic truth.
- Prefer outputs that expose metrics, thresholds, counts, reason codes, and
  `truncated`/`total` fields; mention those numbers when making recommendations.
- Check test coverage with `query_graph_tool` pattern="tests_for" before claiming a
  code path is untested.
- For refactors, verify public APIs, dynamic dispatch, generated code, test
  artifacts, and framework entry points before editing.
- If an output is truncated or approximate, narrow with `top_n`, `detail_level`,
  `max_depth`, or a targeted follow-up query before drawing conclusions.

### Workflow

1. Start with `get_minimal_context_tool(task=...)`.
2. Use the suggested next tool or a targeted query.
3. For reviews, use `review_tool(mode="changes")` and read `analysis_summary`
   first. Call `review_tool(mode="context")`, `review_tool(mode="affected_flows")`,
   `review_tool(mode="impact")`, or `query_graph_tool` only when the summary points there.
4. For architecture work, use
   `architecture_analysis_tool(mode="overview", detail_level="minimal")`
   and read `architecture_health` first. Use the Architecture Analysis skill to
   choose drill-down modes only when the health summary identifies a concrete risk.
5. For refactors, use `refactor_tool(mode="suggest")` first, then preview
   renames with `refactor_tool(mode="rename")`. Apply with
   `dagayn tool apply_refactor_tool` (advanced MCP surface: `dagayn serve --tools all`).

<!-- dagayn markdown policy -->
## Markdown documentation policy: declare dependencies via directive comments

When authoring or editing a Markdown document in this repository, declare
inter-section and inter-document dependencies as HTML directive comments so
they are captured by the dagayn graph (`DEPENDS_ON` / `IMPORTS_FROM` edges)
and discoverable via `query_graph_tool` / `review_tool(mode="impact")`.

### Required form

```markdown
<!-- <kind> <target> -->
```

`<kind>` MUST be one of: `constrained-by`, `blocked-by`, `supersedes`,
`derived-from`. Choose the kind whose semantics best match the dependency:

| Kind | Use when |
| ---- | -------- |
| `constrained-by` | This section's design is bounded by the referenced document/section |
| `blocked-by` | This item cannot proceed until the referenced item resolves |
| `supersedes` | This document replaces the referenced content |
| `derived-from` | This section is derived from the referenced source |

### Three target shapes

| Dependency type | Target syntax | Example |
| --------------- | ------------- | ------- |
| Within-document section | `#section-slug` | `<!-- derived-from #background -->` |
| Other document (whole file) | `./relative/path.md` | `<!-- blocked-by ./specs/open-issue.md -->` |
| Other document + section | `./path.md#slug` | `<!-- constrained-by ./adr.md#context -->` |

Slugs follow GitHub Markdown rules: lowercase, non-alphanumerics removed,
spaces and hyphens collapsed to `-`. Place the directive immediately under
the heading whose content depends on the target. External URLs
(`http://`, `https://`) are not graph-resolvable — keep them as ordinary
Markdown links, not directive targets.

### When to add a directive

- Section design references an ADR, spec, or research note → `constrained-by` or `derived-from`.
- A document replaces an older one → `supersedes` (place in the new document).
- A spec/task section is blocked on another being resolved → `blocked-by`.
- A later section extends an earlier one non-obviously → `derived-from #earlier-section`.

If no real dependency exists, do not invent one. Directives are signal, not decoration.
