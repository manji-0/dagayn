---
name: wiki-research
description: Generate and inspect dagayn graph wiki pages, architecture reports, and static graph export files for repository-level research.
argument-hint: "[topic]"
---

# Wiki Research

Use this when the user asks for a repo overview, architecture brief, generated
wiki, or static export file for humans to read.

## Workflow

1. Orient with `get_minimal_context_tool(task="<research topic>")`.
2. For architecture-wide questions, call
   `architecture_analysis_tool(mode="overview", detail_level="minimal")` first.
3. Generate or refresh wiki content only when the user needs a durable report:
   ```bash
   dagayn tool generate_wiki_tool --arg detail_level='"minimal"'
   ```
4. Read focused pages instead of opening generated files wholesale:
   ```bash
   dagayn tool get_wiki_page_tool --arg page='"overview"'
   ```
5. For external graph exports, use the static CLI export surface. Pick the
   smallest format that matches the downstream tool:
   ```bash
   dagayn visualize --format graphml
   dagayn visualize --format mermaid-c4
   dagayn visualize --format svg
   dagayn visualize --format cypher
   dagayn visualize --format obsidian
   ```
   Do not start or suggest a visualization webserver; `dagayn visualize` no
   longer has an interactive HTML graph mode.

## Evidence Rules

- Treat wiki pages as graph summaries, not proof. Verify specific claims with
  `query_graph_tool`, `review_tool(mode="impact")`, or source reads.
- Cite counts, communities, reason codes, and truncation flags from the tool
  output when making repository-level claims.
- Do not run broad graph exports when a minimal architecture overview
  answers the question.

## Efficiency Rules

- Start with one minimal overview. Generate wiki/export artifacts only when the
  user wants a shareable artifact or asks for broad repository documentation.
- Prefer `get_wiki_page_tool` for one topic; avoid reading entire generated wiki
  directories into context.
