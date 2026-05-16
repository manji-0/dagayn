---
name: reading-markdown-document
description: Read a Markdown document with full dependency context — query the dagayn graph first, pre-read referenced docs and code by edge type, then read the body.
argument-hint: "[doc path]"
---

# Reading a Markdown Document

Read a Markdown doc the way dagayn indexes it: load its dependency graph first, pre-read what the doc relies on, then read the prose with that context already in mind. This avoids the common failure mode of rediscovering "what does this section even mean" mid-read.

## Stage 0 — Prerequisites

1. Confirm `[doc path]` was provided. If not, ask the user before continuing.
2. Run `list_graph_stats_tool` once. If `last_updated` is `null`, **or** the doc's mtime (`stat <path>`) is newer than `last_updated`, run `build_or_update_graph_tool()` and wait for it to finish before Stage 1.
3. **Skip-to-prose shortcut.** Run a quick check:
   ```
   wc -l <path>
   grep -E '<!--|`[^`]+`|]\(' <path> | wc -l
   ```
   If line count < 100 **and** the second grep returns 0, jump straight to Stage 3 — there are no graph-relevant constructs to pre-read.

## Stage 1 — Graph snapshot

Take a structural snapshot before opening the file:

1. **Section list** — `query_graph_tool(pattern="file_summary", target="<doc.md>", detail_level="minimal")` to get every heading and its slug. This is the table of contents.
   - If this returns empty (the doc isn't yet in the graph), Stage 0 missed an update — re-run `build_or_update_graph_tool()` once. If still empty, the file is brand new; treat it as a plain text read and skip to Stage 3.
2. **Inbound edges** — `query_graph_tool(pattern="importers_of", target="<doc.md>", detail_level="minimal")`. **Use the file path only**, not `<doc.md>::<section>` — `importers_of` resolves to file paths; section-form targets silently return zero hits (`tools/query.py:241`).
3. **Outbound file-level imports** — `query_graph_tool(pattern="imports_of", target="<doc.md>", detail_level="minimal")` to list cross-doc `IMPORTS_FROM` edges (directives + links targeting other files).
4. **Outbound blast radius** — `review_tool(mode="impact", changed_files=["<doc.md>"], detail_level="minimal")` to see everything that would be affected if this doc changed. This is your set of downstream consumers.

Tool-call budget for Stage 1: ≤ 4 calls (one per step). Stop here before any pre-reading.

## Stage 2 — Pre-read by dependency type

The directives, links, and code-spans in the doc itself are the authoritative outbound-edge list (the parser regexes are at `markdown.py:13–22`). Before reading prose, scan the raw file with one pass and build a per-edge-type triage list:

```
grep -nE '<!-- *(constrained-by|blocked-by|supersedes|derived-from)' <path>   # DEPENDS_ON / IMPORTS_FROM
grep -nE '\[[^]]+\]\([^)]+\)' <path>                                          # IMPORTS_FROM / REFERENCES
grep -nE '`[A-Za-z_][A-Za-z0-9_.]*`' <path>                                   # CROSS_ARTIFACT (candidates)
```

Then handle each type with a fixed budget:

| Edge kind | What you found | Action before reading the body |
|-----------|----------------|--------------------------------|
| `DEPENDS_ON` — `constrained-by` directive | hard prerequisite | Read the cited section in full. This is the only kind that is *always* worth pre-reading. |
| `DEPENDS_ON` — `derived-from` / `blocked-by` / `supersedes` | softer dependency | One-line mental summary from the directive comment itself; only open the target if the doc body later references it explicitly. |
| `IMPORTS_FROM` / `REFERENCES` | inline / reference-style links | Read the target section once, depth 1 only. Do not chase its onward links unless the linking sentence in *this* doc contains "see", "refer to", or a clearly imperative pointer. |
| `CROSS_ARTIFACT` | backticked symbols | **Cap: top 3 most-frequent symbols.** For each, run `query_graph_tool(pattern="callers_of", target="<symbol>", detail_level="minimal")` to know how it's used. Skip the rest unless the body specifically asks you to look them up. |
| `CONTAINS` | heading hierarchy | No tool calls. Hold the section tree from Stage 1 step 1 in mind as a TOC. |

Tool-call budget for Stage 2: ≤ 1 call per `constrained-by` target + ≤ 1 call per linked section actually read + ≤ 3 `callers_of` calls for code symbols. If your triage list exceeds this, prioritize `constrained-by` first, then linked sections, then symbols.

## Stage 3 — Read the body

Now read the file top-to-bottom:

1. **Sanity check the section list.** Compare the headings you see in the file to Stage 1's `file_summary` output. If they differ (added / renamed / deleted sections), the graph is stale relative to the file; the prose is the source of truth — make a mental note and continue.
2. **Read each section** with the Stage-2 dependency context already loaded. When you encounter a directive or link, you should already have its target's gist.
3. **When you hit a backticked `` `Symbol` ``**, recall the `callers_of` slice from Stage 2 if the symbol was in your top 3; otherwise note it as "unverified" and continue — don't tool-call mid-read.
4. **Flag surprises.** If a section says something the dependency context did not predict, that's the *interesting* part — note it in a running list. Do not stop and dig in mid-doc.

Stage 3 done when: you reach EOF, you've read every section, and your "surprises" list is captured. The list is the output of this skill — surface it back to the user along with a one-paragraph summary.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a graph
tool needed for document reading, run the same implementation through the CLI
without restarting the agent:

```bash
dagayn tool list_graph_stats_tool
dagayn tool query_graph_tool --arg pattern='"file_summary"' --arg target='"docs/adr.md"'
dagayn tool review_tool --arg mode='"impact"' --arg 'changed_files=["docs/adr.md"]' --arg detail_level='"minimal"'
```

## Token Efficiency Rules

- For ad-hoc graph exploration *outside* the per-stage calls listed above, start with `get_minimal_context_tool(task="<your task>")` first.
- Always pass `detail_level="minimal"` unless you've established that minimal is missing what you need.
- Hard ceiling for one full read end-to-end: ≤ 12 tool calls, ≤ 2,000 tokens of graph-tool output. If you're approaching it (typically: a doc with many code spans), drop to a depth-0 read and report the budget squeeze to the user.
