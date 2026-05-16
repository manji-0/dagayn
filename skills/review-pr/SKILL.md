---
name: review-pr
description: Review a PR or branch diff using the knowledge graph for full structural context. Outputs a structured review with blast-radius analysis.
argument-hint: "[PR number or branch name]"
---

# Review PR

Perform a comprehensive code review of a pull request or branch diff using the knowledge graph.

**Token optimization:** Before starting, call `get_docs_section_tool(section_name="review-pr")` for the optimized workflow. Never include full files unless explicitly asked.

## Steps

1. **Orient first** by calling `get_minimal_context_tool(task="<PR review>")`.

2. **Identify the changes** for the PR:
   - If a PR number or branch is provided, use `git diff main...<branch>` to get changed files
   - Otherwise auto-detect from the current branch vs main/master

3. **Update the graph** by calling `build_or_update_graph_tool(base="main")` to ensure the graph reflects the current state.

4. **Get risk and review priorities** by calling `detect_changes_tool(base="main")`:
   - This uses `main` (or the specified base branch) as the diff base
   - Returns all changed files across all commits in the PR
   - Read `analysis_summary` for risk reasons, recommended tests, affected-flow
     rankings, documentation update candidates, hotspot proximity, and
     architecture risks in changed scopes

5. **Fetch focused source context** by calling `get_review_context_tool(base="main")`
   for the files or functions that need exact snippets.

6. **Analyze impact** by using `analysis_summary` first, then calling
   `get_impact_radius_tool(base="main")` only when a wider view is needed:
   - Review the blast radius across the entire PR
   - Identify high-risk areas (widely depended-upon code)

7. **Deep-dive each changed file**:
   - Read the full source of files with significant changes
   - Use `query_graph_tool(pattern="callers_of", target=<func>)` for high-risk functions
   - Start with `analysis_summary.recommended_tests`; use
     `query_graph_tool(pattern="tests_for", target=<func>)` to verify uncertain coverage
   - Check for breaking changes in public APIs

8. **Generate structured review output**:

   ```
   ## PR Review: <title>

   ### Summary
   <1-3 sentence overview>

   ### Risk Assessment
   - **Overall risk**: Low / Medium / High
   - **Blast radius**: X files, Y functions impacted
   - **Test coverage**: N changed functions covered / M total

   ### File-by-File Review
   #### <file_path>
   - Changes: <description>
   - Impact: <who depends on this>
   - Issues: <bugs, style, concerns>

   ### Missing Tests
   - <function_name> in <file> - no test coverage found

   ### Recommendations
   1. <actionable suggestion>
   2. <actionable suggestion>
   ```

## Tips

- For large PRs, focus on the highest-impact files first (most dependents)
- Use `semantic_search_nodes_tool` to find related code the PR might have missed
- Check if renamed/moved functions have updated all callers
- Prefer `detect_changes_tool.analysis_summary` before calling drill-down
  review tools.
- Use graph risk labels as prioritization, not proof. Confirm behavioral issues
  in source or tests before reporting them as findings.
- Include `truncated`, `total`, approximation, or threshold metadata in the
  review when a tool's output is bounded.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a PR
review drill-down tool, run the same implementation through the CLI without
restarting the agent:

```bash
dagayn tool detect_changes_tool --arg base='"main"' --arg detail_level='"minimal"'
dagayn tool get_review_context_tool --arg base='"main"' --arg detail_level='"minimal"'
dagayn tool get_impact_radius_tool --arg base='"main"' --arg detail_level='"minimal"'
dagayn tool list_flows_tool --arg detail_level='"minimal"'
```
