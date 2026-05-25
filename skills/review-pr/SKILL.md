---
name: review-pr
description: Review a PR or branch diff using the knowledge graph for full structural context. Outputs a structured review with blast-radius analysis.
argument-hint: "[PR number or branch name]"
---

# Review PR

Perform a comprehensive code review of a pull request or branch diff using the knowledge graph.

**Token optimization:** Before starting, call `get_docs_section_tool(section_name="review-pr")` for the optimized workflow. Never include full files unless explicitly asked.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so related-code search matches the installed
retrieval setup.
<!-- /dagayn skill embedding context -->

## Steps

1. **Orient first** by calling `get_minimal_context_tool(task="<PR review>")`.

2. **Identify the changes** for the PR:
   - If a PR number or branch is provided, use `git diff main...<branch>` to get changed files
   - Otherwise auto-detect from the current branch vs main/master

3. **Update the graph** by calling `build_or_update_graph_tool(base="main")` to ensure the graph reflects the current state.

4. **Get risk and review priorities** by calling `review_tool(mode="changes", base="main")`:
   - This uses `main` (or the specified base branch) as the diff base
   - Returns all changed files across all commits in the PR
   - Read `analysis_summary` for risk reasons, recommended tests, affected-flow
     rankings, documentation update candidates, hotspot proximity, and
     architecture risks in changed scopes

5. **Fetch focused source context** by calling `review_tool(mode="context", base="main")`
   for the files or functions that need exact snippets.

6. **Analyze impact** by using `analysis_summary` first, then calling
   `review_tool(mode="impact", base="main")` only when a wider view is needed:
   - Review the blast radius across the entire PR
   - Identify high-risk areas (widely depended-upon code)
   - Follow documentation bridges only when relevant to the changed surface:
     `query_graph_tool(pattern="docs_for", target=<path::symbol>)` for changed
     code/Terraform nodes, and
     `query_graph_tool(pattern="implementations_of", target=<doc.md>::<section-slug>)`
     for changed Markdown contract sections

7. **Deep-dive each changed file**:
   - Read the full source of files with significant changes
   - Use `query_graph_tool(pattern="callers_of", target=<func>)` for high-risk functions
   - Start with `analysis_summary.recommended_tests`; use
     `query_graph_tool(pattern="tests_for", target=<func>)` to verify uncertain coverage
   - Check for breaking changes in public APIs
   - When `dagayn:` documentation directives are present, interpret direction
     by authoring site: Markdown `implemented-by` means the doc owns the
     contract; code `implements` means the implementation declares conformance;
     `explained-by`, `has-runbook`, and `problem-described-by` mean linked docs
     may be stale after code changes. Check each result's `evidence_type`
     (`authored`, `extracted`, or `heuristic_reachable`) before treating it as
     contract evidence. Do not expect duplicate inverse edges.

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
- Use `semantic_search_nodes_tool` for conceptual or fuzzy related-code search.
  For exact renamed or moved symbols, prefer `query_graph_tool` relationships or
  a targeted `rg` literal check after graph triage.
- Check if renamed/moved functions have updated all callers
- Prefer `review_tool(mode="changes").analysis_summary` before calling drill-down
  review tools.
- Use graph risk labels as prioritization, not proof. Confirm behavioral issues
  in source or tests before reporting them as findings.
- Include `truncated`, `total`, approximation, or threshold metadata in the
  review when a tool's output is bounded.
- Cite `CROSS_ARTIFACT` documentation roles and query patterns when they drive a
  finding: `docs_for` for code→docs context, `implementations_of` for
  doc→implementation context.
- For empty or not-found graph results, report `zero_result_reason`,
  `next_action`, `answerability`, and `missingness` instead of concluding the
  symbol or relationship is absent.

## Efficiency Rules

- Review highest-risk files first from `analysis_summary`; do not read every
  changed file before triage on large PRs.
- Use `review_tool(mode="context")` for focused snippets, then full file reads
  only when behavior cannot be judged from the snippet.
- For broad PRs, cap graph drill-down to the top few impacted functions per
  risk area before reporting residual uncertainty.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a PR
review drill-down tool, run the same implementation through the CLI without
restarting the agent:

```bash
dagayn tool review_tool --arg mode='"changes"' --arg base='"main"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"context"' --arg base='"main"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"impact"' --arg base='"main"' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/spec.md::contract-section"'
dagayn tool flow_tool --arg mode='"list"' --arg detail_level='"minimal"'
```
