---
name: Review Changes
description: Perform a structured code review using change detection and impact
---

## Review Changes

Perform a thorough, risk-aware code review using the knowledge graph.

### Steps

1. Run `get_minimal_context(task="<review goal>")` to check graph freshness,
   risk, and suggested next tools.
2. Run `review_tool(mode="changes")` to get risk-scored change analysis. Read
   `analysis_summary` first; it includes reason codes, recommended tests,
   affected-flow rankings, documentation update candidates, hotspot proximity,
   and architecture risks in changed scopes.
3. Call `review_tool(mode="context")` when exact source snippets are needed.
4. Call `review_tool(mode="affected_flows")`, `review_tool(mode="impact")`, or
   `query_graph(pattern="tests_for")` only when `analysis_summary` points to a
   concrete flow, blast-radius, or coverage question.
5. For any remaining untested changes, suggest specific test cases.

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
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
