---
name: review-changes
description: Perform a structured code review using change detection and impact
---

## Review Changes

Perform a thorough, risk-aware code review using the knowledge graph.

### Steps

1. Run `get_minimal_context_tool(task="<review goal>")` to check graph freshness,
   risk, and suggested next tools. If `graph_health.status` is `empty` (or
   `ensure_graph_tool` is the first next-tool hint), call `ensure_graph_tool()`
   and re-orient before any review call.
2. Run `review_tool(mode="changes")` to get risk-scored change analysis. Read
   `analysis_summary` first; it includes reason codes, recommended tests,
   affected-flow rankings, documentation update candidates, hotspot proximity,
   and architecture risks in changed scopes.
3. Call `review_tool(mode="context")` when change-set snippets are needed.
   For one named symbol, use `query_graph_tool(pattern="source_of")` instead of
   opening the file.
4. Call `review_tool(mode="affected_flows")`, `review_tool(mode="impact")`, or
   `query_graph_tool(pattern="tests_for")` only when `analysis_summary` points to a
   concrete flow, blast-radius, or coverage question.
5. Follow documentation bridge edges when they can change the review outcome:
   - For changed code or Terraform nodes, use `query_graph_tool(pattern="docs_for", target="<path::symbol>", detail_level="minimal")` to find linked specs, runbooks, explanations, or issue notes from `dagayn:` documentation directives.
   - For changed Markdown contract sections, use `query_graph_tool(pattern="implementations_of", target="<doc.md>::<section-slug>", detail_level="minimal")` to find code linked by Markdown `implemented-by` or code `implements` directives.
6. For any remaining untested changes, suggest specific test cases.
7. **Docs update after code change** — when `analysis_summary` lists documentation
   update candidates, or `docs_for` returns authored contract/runbook links for
   changed symbols, do not stop at "docs may be stale":
   1. Rank candidates: `implemented_by` / `implements_contract` first, then
      `explained_by` / `has_runbook` / `problem_described_by`, then weaker
      `extracted` / `heuristic_reachable` hits.
   2. Fetch only the docs that affect the merge decision. Prefer
      `query_graph_tool(pattern="source_of")` on a DocSection; open the file
      only if the span is truncated, stale, or neighbors are required.
   3. Edit them with the `writing-markdown-document` skill (keep `dagayn:`
      directives and heading slugs accurate).
   4. `ensure_graph_tool(force=True)`, then re-check
      `query_graph_tool(pattern="docs_for" | "implementations_of")` or
      `review_tool(mode="impact")` on the touched doc paths.
   If docs work is deferred, say so explicitly in the review output with the
   doc path and role that still needs an update.

### Output Format

Provide findings grouped by risk level (high/medium/low) with:
- What changed and why it matters
- Test coverage status
- Documentation updates required or deferred (path + role)
- Suggested improvements
- Overall merge recommendation

### Evidence Rules

- Base risk claims on changed-node count, blast radius, affected flows, test
  coverage, `analysis_summary.reason_codes`, and public API/dependency
  direction changes.
- Treat `concern_separation` and `function_concern_pressure` as refactoring
  prioritization evidence, not correctness evidence. Read role, threshold,
  reason codes, purity-likelihood evidence, missingness, and suggested action
  before deciding whether to mention it. Boundary/coordinator functions may
  legitimately have side effects.
- Do not report a function concern profile as a bug by itself. For a review
  finding, confirm source behavior with `source_of`, contract impact, missing
  tests, or a concrete maintainability risk. Otherwise frame it as a follow-up
  refactor lead.
- Treat `CROSS_ARTIFACT` documentation roles as typed evidence, not duplicate
  inverse facts. Check each result's `evidence_type`: `implemented_by` and
  `implements_contract` are authored contract evidence; explanatory roles such
  as `describes_symbol` are usually `extracted`; unresolved or low-confidence
  candidates are `heuristic_reachable` and should stay tentative. Cite the
  stored role and the query pattern (`docs_for` or `implementations_of`) used.
- For zero-result or not-found graph queries, read `zero_result_reason`,
  `next_action`, `answerability`, and `missingness` before claiming absence.
- Report `truncated`, `total`, or approximation metadata when a tool response is
  incomplete.
- Confirm behavior with `query_graph_tool(pattern="source_of")` before reporting
  a behavioral bug; graph structure alone is not enough. Read the file only when
  that span is truncated, stale, unreadable, or neighbors are required.

## CLI Fallback

Default MCP already exposes `review_tool` and `query_graph_tool`. Use
`dagayn tool` when the server allow-list omitted them:

```bash
dagayn tool review_tool --arg mode='"changes"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"context"' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"src/app.py::handler"'
dagayn tool review_tool --arg mode='"affected_flows"' --arg 'changed_files=["src/app.py"]'
dagayn tool review_tool --arg mode='"impact"' --arg 'changed_files=["src/app.py"]' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/spec.md::contract-section"'
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context_tool(task="<your task>")` before any other graph tool.
- If the graph was empty, count tool calls **after** `ensure_graph_tool` returns.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens
  after ensure.
