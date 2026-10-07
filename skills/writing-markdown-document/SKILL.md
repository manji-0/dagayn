---
name: writing-markdown-document
description: Write or edit Markdown (design docs, ADRs, RFCs, specs, runbooks, READMEs) so dagayn indexes it correctly — dependency directives, links with the right section slugs, and links between doc sections and the code that implements them — in an outline, draft-and-verify, polish, summary flow. Use when the user asks to write, restructure, or update a doc, including after a code change.
argument-hint: "[doc path]"
---

# Writing a Markdown Document

A document dagayn can index becomes part of the knowledge graph: other docs and
code can depend on it, reviews flag it when the code it describes changes, and
readers can jump from a section to its implementation. That only works when
directives, links, and slugs resolve, so this flow verifies each one.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so code-symbol lookup handles FTS and hybrid search
correctly.
<!-- /dagayn skill embedding context -->

## Stage 0 — Prerequisites

1. `get_minimal_context_tool` once; if `graph_health.status` is `empty` or
   `sync.state` is `unbuilt`, call `ensure_graph_tool()` and wait for it.
2. Resolve the path: an existing file is the target; a new path is created in
   Stage 1; with no path, ask the user for purpose, audience, and location
   instead of inventing one.

## dagayn Markdown reference

| Construct | Syntax | Edges |
|---|---|---|
| Heading | `## Section Title` | `CONTAINS` (file → section → subsection) |
| Dependency directive | `<!-- constrained-by ./other.md#Section -->` | `DEPENDS_ON`, plus `IMPORTS_FROM` when the target is another file |
| Documentation directive | an HTML comment whose text is `dagayn: <kind> <target>` | `CROSS_ARTIFACT` from the enclosing section |
| Link, no anchor | `[text](./other.md)` | `IMPORTS_FROM` |
| Link with anchor | `[text](./other.md#Section)` | `IMPORTS_FROM` and `REFERENCES` (section → section) |
| Reference link | `[label]: ./other.md#Section` | same as inline links |
| Code span | `` `BridgeDetector` `` | `CROSS_ARTIFACT` to a code symbol, resolved in post-processing |

Dependency kinds: `constrained-by`, `blocked-by`, `supersedes`, `derived-from`
(case-insensitive; the kind is kept as metadata).

**Paths resolve differently by construct.** Dependency directives and links
resolve every path relative to **the document's own directory** (with or
without `./`), and a leading `/` path is dropped. `dagayn:` directives resolve
`./` and `../` relative to the document but treat any other path as
**repo-root**-relative. So in `docs/design/api.md`, a dependency directive
naming `docs/x.md` points at `docs/design/docs/x.md`, while a `dagayn:` directive
naming the same text points at `docs/x.md`.

**Slugs** (`markdown_slugify`, `crates/dagayn-parser/src/markdown.rs`): letters
and digits are lowercased (non-ASCII letters such as `é` or `日本` are kept),
spaces and hyphens become `-`, underscores stay, other symbols are dropped, and
duplicate headings get `-1`, `-2`, … in order. `## What's new?` → `whats-new`;
`## Stage 1 — Outline` → `stage-1--outline`.

**Code spans** become candidate links only when the identifier is at least 3
characters, and identifiers without `_` or `.` need at least 10 characters.
Post-processing resolves a span by the symbol's **bare name**: exactly one
non-Markdown node with that name promotes the edge at MEDIUM/0.4 (shown as
`heuristic_reachable`); zero or several matches delete it. A dotted span
(`module.Class`, `Class.method`) never matches a code symbol, because nodes are
matched by bare `name`. For an intentional link, use a documentation directive
instead.

## Markdown ↔ code documentation links

Use documentation directives for intentional obligations between docs and code;
use plain code spans only for low-intent mentions. The directive belongs to the
artifact that owns the assertion.

The examples below give the comment **text**. In a real document, wrap that
text in an HTML comment (`<!--` … `-->`). They are written this way because the
parser reads `dagayn:` directives even inside code fences and backticks, so a
complete example in a doc would create a real edge to a target that doesn't
exist.

| Authoring site | Comment text | Stored role | Use when |
|---|---|---|---|
| Markdown contract/spec section | `dagayn: implemented-by services/auth.py::refresh_token` | `implemented_by` | The doc defines intent and code realizes it. |
| Markdown explanation or issue | `dagayn: discusses services/auth.py::refresh_token`, `dagayn: raises-issue-for …` | `discusses_artifact`, `raises_issue_for` | The doc owns the discussion or issue. |
| Markdown section about one symbol | `dagayn: describes services/auth.py::refresh_token` | `describes_symbol` | A precise replacement for an ambiguous code span. |
| Code comment | `# dagayn: implements docs/auth-spec.md#Token Refresh` | `implements_contract` | The code declares conformance to a doc section. |
| Code comment | `# dagayn: explained-by docs/auth-runbook.md#Refresh Failures` | `explained_by` | The code points to rationale. |
| Code comment | `# dagayn: has-runbook docs/infra-runbook.md#Graph Store Bucket` | `has_runbook` | The code points to an operational runbook. |
| Code comment | `# dagayn: problem-described-by docs/audits/auth.md#Stale Cache` | `problem_described_by` | The code points to an audit or known issue. |

All kinds: `implemented-by`, `implements`, `explained-by`, `has-runbook`,
`problem-described-by`, `discussed-by`, `discusses` (or `discusses-artifact`),
`raises-issue-for`, `describes` (or `describes-symbol`).

Targets:
- Markdown → code: a concrete `path::symbol` node (e.g.
  `services/auth.py::AuthService.refresh_token`); confirm it exists with
  `query_graph_tool(pattern="source_of")` before writing it. A dangling target
  is **not** flagged: the edge stays authored and HIGH, so an
  `implementations_of` hit doesn't prove the target exists. A bare symbol
  starts as `<unresolved:Symbol>` (LOW) until exactly one node has that name.
- Code → Markdown: always a Markdown path plus `#Heading`; the heading is
  slugified (`docs/auth-spec.md#Token Refresh` → `docs/auth-spec.md::token-refresh`).
  From code, a bare `#Heading` would point at the code file.
- Code-side directives are read from Python `#` comments, Terraform `#` / `//`,
  and C# `//` / `///` comments, attached to the enclosing node or one within
  the next 3 lines.
- Author one direction per fact; query tools show the inverse, and duplicate
  inverse edges go stale on incremental updates.

### Trust when reading these links later

- **Highest** — authored `implemented_by` / `implements_contract` plus
  `source_of` on the concrete target (confirm the target exists before you
  write the directive).
- **Medium** — explanatory / runbook / issue roles (`explained_by`,
  `has_runbook`, `problem_described_by`, `discusses_artifact`, …).
- **Low** — bare code-span `heuristic_reachable` mentions; do not treat them
  as contracts.

## Stage 1 — Outline and order sections

1. List the sections.
2. For each, list its dependencies and verify them:
   - Sections of existing docs: `query_graph_tool(pattern="file_summary",
     target="<doc.md>")` and use the `DocSection` rows' slugs.
   - Code symbols to backtick: `semantic_search_nodes_tool(query="<symbol>")`
     and require exactly one exact symbol match (`exactness.exact_match_count
     == 1`). Ignore semantic near-matches: only exact `name` matches count.
     With several exact matches, use a `describes` directive with a
     `path::symbol` target instead of the span.
   - Directive targets: confirm the `path::symbol` exists.
3. Order sections so each comes after everything it depends on. If a cycle
   survives splitting sections, ask the user which dependency to break rather
   than writing a forward reference.

## Stage 2 — Draft and verify each section

1. Draft the prose.
2. Express dependencies: `constrained-by` for hard prerequisites near the top
   of the section, `derived-from` for source material, links for narrative
   references, documentation directives for code obligations, and backticks
   for exact low-intent symbol mentions.
3. Save and run `ensure_graph_tool(force=True)` (minimal post-processing, which
   includes Markdown link resolution). Use
   `build_or_update_graph_tool(local_embedding="none")` only when you need the
   advanced build controls.
4. Verify:
   - `query_graph_tool(pattern="importers_of", target="<doc.md>")` for inbound
     edges (file level only: a `doc.md::slug` target is widened to the file).
   - `review_tool(mode="impact", changed_files=["<doc.md>"])` for outbound reach.
   - `query_graph_tool(pattern="implementations_of", target="<doc.md>::<section-slug>")`
     for code links; `docs_for` from the code side.
   Keep `zero_result_reason`, `next`, and `missingness` from any empty
   result in your notes instead of assuming the link can't exist.
5. If a link didn't take effect, recheck the slug and the path base. A link to
   a misspelled anchor leaves its `REFERENCES` edge at LOW, so look for that;
   dangling dependency directives and `dagayn:` targets show no such signal.

## Stage 3 — Polish

1. Re-read top to bottom; tighten prose, rebalance sections.
2. Re-check every backticked symbol for exactly one exact symbol match; if
   ambiguous, switch to a `describes` directive or plain text.
3. `ensure_graph_tool(force=True)`, then `review_tool(mode="impact")` again: no
   edge present in Stage 2 should have disappeared. (`markdown_artifact_refs_dropped`
   counts both deleted code spans and demoted directives.)

## Stage 4 — Summary and conclusion

Add these last. In the summary, give each recap a dependency directive such as
`<!-- derived-from #section-slug -->` so the graph shows what it summarizes; if
the document replaces another, say `<!-- supersedes ./old-design.md -->`. Then
`file_summary` should list every section.

## CLI fallback

```bash
dagayn tool ensure_graph_tool --arg force=true
dagayn tool query_graph_tool --arg pattern='"file_summary"' --arg target='"docs/design.md"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/design.md::contract-section"'
dagayn tool review_tool --arg mode='"impact"' --arg 'changed_files=["docs/design.md"]' --arg detail_level='"minimal"'
dagayn tool semantic_search_nodes_tool --arg query='"BridgeDetector"' --arg detail_level='"minimal"'
```
