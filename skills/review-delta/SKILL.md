---
name: review-delta
description: Review only changes since last commit using impact analysis. Token-efficient delta review with automatic blast-radius detection.
argument-hint: "[file or function name]"
---

# Review Delta

Perform a focused, token-efficient code review of only the changed code and its blast radius.

**Token optimization:** Before starting, call `get_docs_section_tool(section_name="review-delta")` for the optimized workflow. Use ONLY changed nodes + 2-hop neighbors in context.

## Steps

1. **Orient first** by calling `get_minimal_context_tool(task="<review goal>")`.

2. **Ensure the graph is current** by calling `build_or_update_graph_tool()` (incremental update).

3. **Get risk and review priorities** by calling `review_tool(mode="changes")`.
   Read `analysis_summary` first. It returns:
   - Risk level, risk score, and reason codes
   - Changed/impacted node and file counts
   - Recommended tests
   - Affected-flow rankings
   - Documentation update candidates
   - Hotspot proximity
   - Architecture risks in changed scopes

4. **Fetch source context only when needed** by calling
   `review_tool(mode="context")` for files or functions where source snippets
   are required.

5. **Analyze the blast radius** by reviewing the impact fields in
   `analysis_summary` and, when needed, calling `review_tool(mode="impact")`.
   Focus on:
   - Functions whose callers changed (may need signature/behavior verification)
   - Classes with inheritance changes (Liskov substitution concerns)
   - Files with many dependents (high-risk changes)
   - `CROSS_ARTIFACT` documentation links where changed code points have linked
     specs/runbooks (`query_graph_tool(pattern="docs_for", target=<path::symbol>)`)
     or changed Markdown sections have linked implementations
     (`query_graph_tool(pattern="implementations_of", target=<doc.md>::<section-slug>)`)

6. **Perform the review** using the context. For each changed file:
   - Review the source snippet for correctness, style, and potential bugs
   - Check if impacted callers/dependents need updates
   - Prefer `analysis_summary.recommended_tests` first, then verify uncertain
     coverage using `query_graph_tool(pattern="tests_for", target=<function_name>)`
   - If a `dagayn:` documentation directive links the changed surface to a
     Markdown section or code point, verify whether that linked artifact also
     needs review or an update. Use the stored role (`implemented_by`,
     `implements_contract`, `explained_by`, `has_runbook`,
     `problem_described_by`, `discusses_artifact`, `raises_issue_for`) as the
     reason, check `evidence_type` (`authored`, `extracted`, or
     `heuristic_reachable`), and avoid assuming duplicate inverse edges exist.
   - Flag any untested changed functions

7. **Report findings** in a structured format:
   - **Summary**: One-line overview of the changes
   - **Risk level**: Low / Medium / High (based on blast radius)
   - **Issues found**: Bugs, style issues, missing tests
   - **Blast radius**: List of impacted files/functions
   - **Recommendations**: Actionable suggestions

## Advantages Over Full-Repo Review

- Uses composed change analysis before fetching source snippets
- Automatically identifies blast radius without manual file searching
- Provides structural context (who calls what, inheritance chains)
- Recommends likely tests and flags untested functions automatically

## Efficiency Rules

- Stay on `review_tool(mode="changes")` and `analysis_summary` until there is a
  concrete source, flow, impact, or coverage question.
- Fetch snippets with `review_tool(mode="context")` only for files that can
  change the review outcome.
- Prefer recommended tests first; use `query_graph_tool(pattern="tests_for")`
  only for uncertain coverage.

## Evidence Rules

- Cite the concrete metric behind each risk label:
  `analysis_summary.reason_codes`, blast-radius count, affected flow,
  dependency direction, test gap, or changed public surface.
- Treat missing tests as a lead until `tests_for` and source-level behavior are
  checked.
- Treat zero-result graph queries as graph-limited leads. Read
  `zero_result_reason`, `next_action`, `answerability`, and `missingness` before
  claiming absence.
- If a graph result is truncated, narrow it before making a final review claim.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a review
drill-down tool, run the same implementation through the CLI without restarting
the agent:

```bash
dagayn tool review_tool --arg mode='"changes"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"context"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"impact"' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"tests_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/spec.md::contract-section"'
```
