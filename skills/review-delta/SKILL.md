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

2. **Refresh only when needed**:
   - If `graph_health.status` is `empty`, call `ensure_graph_tool()`.
   - If the working tree looks newer than the graph (hooks skipped, untracked
     files needed, or results look stale), call `ensure_graph_tool(force=True)`.
   - Otherwise skip ensure and go straight to review — hooks/`dagayn update`
     usually keep a healthy graph current.

3. **Get risk and review priorities** by calling `review_tool(mode="changes")`.
   Read `analysis_summary` first. It returns:
   - Risk level, risk score, and reason codes
   - Changed/impacted node and file counts
   - Recommended tests
   - Affected-flow rankings
   - Documentation update candidates
   - Hotspot proximity
   - Architecture risks in changed scopes

4. **Fetch source context only when needed**:
   - Change-set snippets: `review_tool(mode="context")`
   - One named function or class: `query_graph_tool(pattern="source_of")`
   Read the file only when that span is truncated, stale, or neighbors are
   required.

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
   - Review the `context` snippet or `source_of` span for correctness, style,
     and potential bugs
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
  change the review outcome. For one `qualified_name`, use `source_of` instead
  of opening the file.
- Prefer recommended tests first; use `query_graph_tool(pattern="tests_for")`
  only for uncertain coverage.
- Do not call `ensure_graph_tool(force=True)` on every review when
  `graph_health` is already healthy.
- When documentation candidates appear, either update them (see review-changes
  **Docs update after code change**) or list them as explicit deferrals — do not
  silently ignore authored contract links.

## Evidence Rules

- Cite the concrete metric behind each risk label:
  `analysis_summary.reason_codes`, blast-radius count, affected flow,
  dependency direction, test gap, or changed public surface.
- Treat missing tests as a lead until `tests_for` and `source_of` (or a file
  read if that span is truncated/stale) are checked.
- Treat zero-result graph queries as graph-limited leads. Read
  `zero_result_reason`, `next_action`, `answerability`, and `missingness` before
  claiming absence.
- If a graph result is truncated, narrow it before making a final review claim.

## CLI Fallback

Default MCP already exposes `review_tool` and `query_graph_tool`. Use
`dagayn tool` when the server allow-list omitted them:

```bash
dagayn tool review_tool --arg mode='"changes"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"context"' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"src/app.py::handler"'
dagayn tool review_tool --arg mode='"impact"' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"tests_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/spec.md::contract-section"'
```
