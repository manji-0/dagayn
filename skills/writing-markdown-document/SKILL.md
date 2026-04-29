---
name: writing-markdown-document
description: Author Markdown documents (READMEs, design docs, RFCs) so dagayn extracts correct dependency edges. Four-stage flow — outline & sort, draft & verify, polish, summary.
argument-hint: "[doc path]"
---

# Writing a Markdown Document

Write Markdown that dagayn can index correctly so the document becomes a first-class node in the knowledge graph, with dependency edges to other docs and to code.

## Stage 0 — Prerequisites

Run **once** at the start, regardless of what the rest of the flow says:

1. `list_graph_stats_tool` — if `last_updated` is `null` or `nodes == 0`, run `build_or_update_graph_tool(full_rebuild=True)` and stop until that returns.
2. Resolve the doc path:
   - If `[doc path]` was provided and the file exists → that's your target.
   - If it was provided but the file is new → continue (Stage 1 will create it).
   - If no arg → ask the user for the doc's purpose, audience, and intended path before going further. Do not invent a path.

## dagayn Markdown reference

dagayn's Markdown parser (`dagayn/parser/languages/markdown.py`) extracts edges from five constructs. Use them deliberately:

| Construct | Syntax | Edges produced |
|-----------|--------|----------------|
| **Heading** | `## Section Title` | `CONTAINS` (file → section, section → subsection) |
| **Directive** (HTML comment, case-insensitive) | `<!-- constrained-by ./other.md#Section -->` | `DEPENDS_ON` always; **plus** `IMPORTS_FROM` when the target is a different file |
| **Inline link, no anchor** | `[text](./other.md)` | `IMPORTS_FROM` only |
| **Inline link, with anchor** | `[text](./other.md#Section)` | `IMPORTS_FROM` (file→file) **and** `REFERENCES` (section→section) |
| **Reference-style link** | `[label]: ./other.md#Section` | Same as inline links — same regex path |
| **Code span** | `` `BridgeDetector` `` | `CROSS_ARTIFACT` (resolved to a code symbol during postprocessing) |

Directive kinds: `constrained-by`, `blocked-by`, `supersedes`, `derived-from`. All four emit identical edge kinds — the kind is preserved as metadata.

Directive / link target shapes:
- `#Section` — local section in the same doc (slug after `#` is itself slugified, so `#My Section` and `#my-section` are equivalent)
- `./relative/path.md` — whole-file dependency
- `./relative/path.md#Section` — specific section in another doc

**Slug rules** (`_markdown_slugify`, lines 219–228):
- Alphanumerics are lowercased.
- Spaces and hyphens both become `-`.
- Underscores are **preserved** (not converted to `-`).
- All other characters (punctuation, em-dashes, unicode symbols) are **stripped**.
- Duplicate headings get `-1`, `-2`, … suffixes appended in document order.

Worked examples:
- `## API Reference` → `api-reference`
- `## user_id lookup` → `user_id-lookup`
- `## What's new?` → `whats-new`
- `## Stage 1 — Outline` → `stage-1--outline` (em-dash stripped, two surrounding spaces both become `-`)

**Code-span identifier rules** (`markdown.py:13–22`):
- Regex: `^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*$` — dots only allowed *between* identifier segments (so `module.Class` is fine, `..foo` or `foo.` are not).
- Identifiers shorter than 3 chars are skipped.
- Identifiers without `_` or `.` need ≥ 10 chars (filters generic English words like `list` / `parser`).

**Postprocessing** (`postprocessing.py:56–125`): each unresolved `CROSS_ARTIFACT` edge is resolved against the code graph by symbol name. **Zero matches → edge dropped. Multiple matches → edge dropped.** Only a unique match keeps the edge (and bumps confidence to HIGH/0.8). Use distinctive, ideally qualified, symbol names.

## Stage 1 — Outline & sort sections

1. Draft a section list (one line per section, in any order).
2. For each prospective section, list its dependencies:
   - **Existing docs you intend to depend on** — for each candidate doc that is *already in the repo*, run `query_graph_tool(pattern="file_summary", target="<doc.md>", detail_level="minimal")` to confirm the section slug you plan to cite actually exists.
   - **Code symbols you plan to backtick** — run `semantic_search_nodes_tool(query="<symbol>", detail_level="minimal")` and require **exactly one** result. If zero, the symbol doesn't exist (don't reference it). If multiple, qualify the name with module/file prefix (`module.Foo`) and re-check.
3. Topologically sort sections so each appears **after** every section it depends on. If a cycle remains after splitting offending sections in two, **stop and ask the user** which dependency to break — do not silently emit a forward reference.

Stage 1 done when: every prospective dependency is verified (or explicitly noted as "external — not in graph"), and the section list is acyclic.

Tool-call budget for Stage 1: ≤ 1 call per existing dependency + 1 per code symbol. Bound it by counting deps before you start.

## Stage 2 — Draft each section & verify edges

For each section, in dependency order:

1. **Draft the prose.**
2. **Express dependencies explicitly:**
   - Hard prerequisites → `<!-- constrained-by ./prereq.md#Section -->` near the top of the section.
   - Material this section is derived from → `<!-- derived-from … -->`.
   - Inline narrative references → `[text](./other.md#Section)`.
   - Code mentions → backtick the symbol exactly as it appears in code.
3. **Save the file** and unconditionally run `build_or_update_graph_tool()` (it's idempotent and cheap; do not try to detect whether hooks are running).
4. **Verify the edges resolved:**
   - `query_graph_tool(pattern="importers_of", target="<doc.md>", detail_level="minimal")` — file-level inbound edges. **Use the file path only — `importers_of` resolves the target to a file path; `<doc.md>::<section>` will silently return zero hits** (`tools/query.py:241`).
   - `get_impact_radius_tool(changed_files=["<doc.md>"], detail_level="minimal")` — outbound blast radius for the whole file.
5. **If a directive looks like it didn't take effect**, re-read your slug against the rules in the reference table above (most common bug: punctuation in heading not accounted for, or section slug typo). Fix and re-run step 3 + 4.

Tool-call budget for Stage 2: ≤ 3 calls per section in the happy path (build + importers_of + impact). Allow 1 extra retry per section for slug fixes.

Stage 2 done for the section when: the directive's intended target appears either as an inbound edge on the cited section or as an outbound entry in the file's impact radius.

## Stage 3 — Polish

1. Re-read the full draft top-to-bottom; tighten prose; merge or split sections if Stage 2 surfaced badly-balanced ones.
2. For every backticked `Symbol`, run `semantic_search_nodes_tool(query="<symbol>", detail_level="minimal")` and require exactly one match. If multiple, qualify (`module.Symbol`); if still multiple after qualification, **accept that this edge will be dropped by postprocessing** and either (a) leave the backticks for prose readability and add a `<!-- TODO: ambiguous symbol — qualify when API stabilizes -->` comment, or (b) remove the backticks and use plain text.
3. Run `build_or_update_graph_tool()` once more, then `get_impact_radius_tool` again. Compare its output to Stage 2's. **Done criterion: no edge that was present in Stage 2 has disappeared.**

Tool-call budget for Stage 3: ≤ 1 call per backticked symbol + 2 final builds.

## Stage 4 — Summary / Conclusion

Add the wrap-up sections **last**, once the rest of the body is stable:

- **Summary** — recap each major section with `<!-- derived-from #stage-N-title -->` so the graph shows the summary depends on the sections it summarizes (use the actual slugs, not the human title).
- **Conclusion** — if this document supersedes or extends another, declare it: `<!-- supersedes ./old-design.md -->`. List external follow-ups with explicit links.

Final check: `query_graph_tool(pattern="file_summary", target="<doc.md>")` should list every section. Done.

## Token Efficiency Rules (graph exploration only)

These bound the *graph-tool* spend; they don't apply to drafting prose or to the per-section verification loops which have their own budgets above.

- Before any *exploratory* graph call (i.e., not one of the per-stage targeted calls listed above), run `get_minimal_context_tool(task="<your task>")`.
- Use `detail_level="minimal"` on every call unless minimal omits something you specifically need.
- Hard ceiling for one full document end-to-end (Stages 0–4): ≤ 30 tool calls and ≤ 5,000 output tokens of graph-tool output across the session. If you're approaching it, stop and ask the user whether to continue.
