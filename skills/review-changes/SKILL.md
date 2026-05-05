---
name: Review Changes
description: Perform a structured code review using change detection and impact
---

## Review Changes

Perform a thorough, risk-aware code review using the knowledge graph.

### Steps

1. Run `get_minimal_context(task="<review goal>")` to check graph freshness,
   risk, and suggested next tools.
2. Run `detect_changes` to get risk-scored change analysis.
3. Run `get_affected_flows` to find impacted execution paths.
4. For each high-risk function, run `query_graph` with pattern="tests_for" to check test coverage.
5. Run `get_impact_radius` when the blast radius is unclear or needs a wider view.
6. For any untested changes, suggest specific test cases.

### Output Format

Provide findings grouped by risk level (high/medium/low) with:
- What changed and why it matters
- Test coverage status
- Suggested improvements
- Overall merge recommendation

### Evidence Rules

- Base risk claims on changed-node count, blast radius, affected flows, test
  coverage, and public API/dependency direction changes.
- Report `truncated`, `total`, or approximation metadata when a tool response is
  incomplete.
- Read exact source before reporting a behavioral bug; graph structure alone is
  not enough for a correctness finding.

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
