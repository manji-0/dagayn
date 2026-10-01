---
name: reading-markdown-document
description: Read a Markdown document — design doc, ADR, RFC, spec, runbook, README, or one section of it — with its dependency context — use the dagayn graph to see its sections, which docs it depends on and which depend on it, and the code that implements it, then read the prose. Consult this skill before opening the file whenever the user asks what a doc or spec says, requires, or promises, asks for a summary or explanation of documentation, asks what implements a section, or is about to edit a doc others depend on.
argument-hint: "[doc path]"
---

# Reading a Markdown Document

Load the doc's dependency graph first, pre-read what it relies on, then read
the prose with that context in mind. That avoids stopping mid-read to work out
what a section is built on.

## Stage 0 — Prerequisites

1. Confirm a doc path was given; ask if not.
2. Run `get_minimal_context_tool` once. If `graph_health.status` is `empty` or
   `sync.state` is `unbuilt`, call `ensure_graph_tool()`; if the doc has
   uncommitted edits (the graph doesn't index a dirty worktree on its own),
   call `ensure_graph_tool(force=True)`. Wait for it before Stage 1.
3. **Short-doc shortcut**: `wc -l <path>` and
   `rg -n '<!--|`[^`]+`|]\(' <path> | wc -l`. Under 100 lines with no matches
   means nothing to pre-read: go to Stage 3.

## Stage 1 — Graph snapshot

1. **Sections**: `query_graph_tool(pattern="file_summary", target="<doc.md>")`.
   Use the `DocSection` rows (their `name` is the slug); `DocBody` rows are the
   section bodies. If it's empty, refresh once with
   `ensure_graph_tool(force=True)`; still empty means a brand-new file — read
   it as plain text. For a question about one heading, fetch just that section
   with `query_graph_tool(pattern="source_of", target="<doc.md>::<slug>")`.
2. **Who depends on it**: `query_graph_tool(pattern="importers_of",
   target="<doc.md>")`. It always answers at file level (a `doc.md::slug`
   target is widened to the file), so it can't tell you which section is
   cited. Ignore the doc's own row, which comes from its in-page anchor links.
   Pass `depth` (up to 6) for transitive dependents.
3. **What it depends on**: `query_graph_tool(pattern="imports_of",
   target="<doc.md>")` lists the files its directives and links point to.
4. **Implementations** (specs, or when asked):
   `query_graph_tool(pattern="implementations_of", target="<doc.md>::<section-slug>")`;
   weigh each hit by `evidence_type` and `missingness`. From a code symbol, the reverse is
   `query_graph_tool(pattern="docs_for", target="<path::symbol>")`.
5. **Blast radius** only when the task is about changing the doc:
   `review_tool(mode="impact", changed_files=["<doc.md>"])`.

About three calls for an ordinary read, plus one each for implementations and
impact when they're in scope.

## Stage 2 — Pre-read by dependency type

Scan the raw file once:

```
rg -n '<!-- *(constrained-by|blocked-by|supersedes|derived-from)' <path>   # DEPENDS_ON / IMPORTS_FROM
rg -n 'dagayn:' <path>                                                     # documentation directives
rg -n '\[[^]]+\]\([^)]+\)' <path>                                          # IMPORTS_FROM / REFERENCES
rg -n '`[A-Za-z_][A-Za-z0-9_.]*`' <path>                                   # code-span candidates
```

The `dagayn:` scan also matches examples inside code fences and backticks —
and the parser turns those into real edges too, so a surprising link may come
from an example.

| What you found | Before reading the body |
|---|---|
| `constrained-by` | Hard prerequisite: fetch the cited section of the **target** doc with `source_of` (target `<target.md>::<slug>`, its path taken relative to the directory of the doc you're reading). Always worth it. |
| `derived-from` / `blocked-by` / `supersedes` | Note the relationship; open the target only if the body leans on it. |
| Links | Fetch the linked section once with `source_of`; don't chase its onward links unless this doc explicitly points you there. |
| `dagayn:` directives | Respect the authored direction. For `implemented-by`, `discusses`, or `raises-issue-for`, inspect the code target with `source_of` when it matters; a target that doesn't exist still shows up as an authored edge. |
| Backticked symbols | Top three by frequency: `source_of` on `<path::symbol>`; add `callers_of` only for call-site questions. |
| Headings | No calls: keep Stage 1's section list as the table of contents. |

Code-side directives (`# dagayn: implements docs/spec.md#Section` in Python or
Terraform, `//` / `///` in C#) don't appear in the Markdown file; Stage 1's
`implementations_of` finds them.

Prioritize `constrained-by`, then documentation bridges, then linked sections,
then symbols, if the list is long.

## Evidence

Reach comes from the graph; correctness from `source_of`; user-visible
effect from a reproduction or CLI output. Do not mix those in one claim.

- **Highest** — authored `CROSS_ARTIFACT` contracts
  (`implemented_by` / `implements_contract`, `evidence_type=authored`) and
  the `source_of` span of the linked code on a fresh orientation.
- **Medium** — `EXTRACTED` directive dependencies (`constrained-by`,
  `blocked-by`, …), explanatory doc roles, and structural importers /
  implementations lists.
- **Low** — `heuristic_reachable` bridges, empty `implementations_of` /
  `importers_of` on a degraded orientation, or treating prose as implemented
  without a code `source_of`.

## Stage 3 — Read the body

1. Compare the headings to Stage 1's section list; if they differ, the prose
   wins and the graph is stale — note it and continue.
2. Read each section with the Stage-2 context loaded.
3. For a directive or symbol you didn't pre-read, note it as unverified rather
   than tool-calling mid-read; keep any `zero_result_reason` / `next_action`
   from an empty query with the note instead of calling the link absent.
4. Collect surprises: places where the prose says something the dependency
   context didn't predict.

Report a one-paragraph summary plus the surprises list.

## CLI fallback

```bash
dagayn tool query_graph_tool --arg pattern='"file_summary"' --arg target='"docs/adr.md"'
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"docs/adr.md::context"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/adr.md::contract-section"'
dagayn tool query_graph_tool --arg pattern='"importers_of"' --arg target='"docs/adr.md"' --arg depth=3
```
