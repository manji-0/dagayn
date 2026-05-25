---
name: Review Changes
description: Perform a structured code review using change detection and impact
---

## Review Changes

Perform a thorough, risk-aware code review using the knowledge graph.

### Steps

1. Run `get_minimal_context_tool(task="<review goal>")` to check graph freshness,
   risk, and suggested next tools.
2. Run `review_tool(mode="changes")` to get risk-scored change analysis. Read
   `analysis_summary` first; it includes reason codes, recommended tests,
   affected-flow rankings, documentation update candidates, hotspot proximity,
   and architecture risks in changed scopes.
3. Call `review_tool(mode="context")` when exact source snippets are needed.
4. Call `review_tool(mode="affected_flows")`, `review_tool(mode="impact")`, or
   `query_graph_tool(pattern="tests_for")` only when `analysis_summary` points to a
   concrete flow, blast-radius, or coverage question.
5. Follow documentation bridge edges when they can change the review outcome:
   - For changed code or Terraform nodes, use `query_graph_tool(pattern="docs_for", target="<path::symbol>", detail_level="minimal")` to find linked specs, runbooks, explanations, or issue notes from `dagayn:` documentation directives.
   - For changed Markdown contract sections, use `query_graph_tool(pattern="implementations_of", target="<doc.md>::<section-slug>", detail_level="minimal")` to find code linked by Markdown `implemented-by` or code `implements` directives.
6. For any remaining untested changes, suggest specific test cases.

### Output Format

Provide findings grouped by risk level (high/medium/low) with:
- What changed and why it matters
- Test coverage status
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
  finding, confirm exact source behavior, contract impact, missing tests, or a
  concrete maintainability risk. Otherwise frame it as a follow-up refactor lead.
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
- Read exact source before reporting a behavioral bug; graph structure alone is
  not enough for a correctness finding.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a review
drill-down mode such as `affected_flows` or `impact`,
run the same implementation through the CLI without restarting the agent:

```bash
dagayn tool review_tool --arg mode='"changes"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"context"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"affected_flows"' --arg 'changed_files=["src/app.py"]'
dagayn tool review_tool --arg mode='"impact"' --arg 'changed_files=["src/app.py"]' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/spec.md::contract-section"'
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context_tool(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
