---
name: architecture-analysis
description: Assess architecture with the dagayn graph — the declared units (crates, packages, modules) and how they depend on each other, import cycles, dependencies on less stable units, widely used untested code, and broken doc links. Use when the user asks about architecture, layering, modularity, coupling, cycles, the riskiest parts of the codebase, or wants an architecture health check.
---

# Architecture Analysis

`architecture_analysis_tool(mode="overview")` is the single entry point. It
returns a map of the units the repository declares and a list of `findings`,
structural facts worth acting on. Answer from those two first.

## Steps

1. **Orient**: `get_minimal_context_tool(task="<architecture goal>")`. If
   `graph_health.status` is `empty` or `sync.state` is `unbuilt` /
   `commit_drift`, follow `next`. The overview needs no
   post-processing: it answers right after an `ensure_graph_tool` bootstrap.
2. **Overview**: `architecture_analysis_tool(mode="overview",
   detail_level="minimal")`.
   - `units`: each Cargo crate, npm package, Go module, Python import package,
     or Terraform module, with its files, symbols, and tests. Code no manifest
     covers falls back to its top-level directory (`kind: "directory"`).
   - `unit_edges`: calls, imports, references, inheritance, and bridges between
     two units, heaviest first. `declared: true` marks a dependency a manifest
     lists; such a pair with no counts is one the graph cannot see (a call
     inside a macro).
   - `findings`: `import_cycle` (modules that import each other when they load,
     with `cut`, the imports that break it), `unstable_dependency` (a unit
     that depends on a less stable one, with the import `sites` and the SAP
     position of the unit depended on), `untested_core` (a symbol used
     from many files that no test reaches through its callers), and
     `broken_doc_link` (a directive pointing at a file, section, or symbol that
     is gone). Empty means nothing structural to act on; say so.
   - `detail_level="standard"` adds each unit's `surface`, the symbols other
     units use most: the unit's de facto public API.
3. **Confirm each finding** before stating it: `query_graph_tool`
   (`importers_of` on a cycle's modules, `callers_of` / `tests_for` on an
   untested symbol, `source_of` for the code), or read the directive's line.
4. **Every import cycle**, not only those the overview reports:
   `mode="import_cycles"` with `scope_kind` `"directory"`, `"file"`, or
   `"package"`, and `unit_filter` to narrow it. Each cycle lists its scopes
   in import order, once; `detail_level="standard"` names one import per hop,
   and `results_complete` says whether the list is whole. `import_kind=
   "statement"` leaves out lazy exports. Imports of modules the graph does
   not index (standard library, compiled extensions) are not edges.
5. **Doc ↔ code boundaries**: `docs_for` from code or Terraform nodes,
   `implementations_of` from a Markdown section. These are traceability links,
   not architectural coupling.

`communities` / `community` are
graph clusters for an explicit clustering question and need full
post-processing (`dagayn postprocess`).

## Evidence

Rank results with the Highest / Medium / Low trust tiers in the installed
dagayn instructions (full rules: `get_docs_section_tool(section_name="trust")`).

- A finding is a checkable claim with a location: cite its `kind`, `file` /
  `line` or `targets`, and `evidence`, after confirming it at the source.
- The map is aggregated graph structure: cite edge counts as counts, and note
  that a call the extractor cannot resolve is missing from them.
- An `unstable_dependency` is a **Medium** lead, not a design bug: cite both
  units' instability, the gap, and the import `sites`.
- For a zero-result query, cite `zero_result_reason` and `next` rather
  than treating it as proof that no relationship exists.

## CLI fallback

```bash
dagayn tool architecture_analysis_tool --arg mode='"overview"' --arg detail_level='"minimal"'
dagayn tool architecture_analysis_tool --arg mode='"overview"' --arg detail_level='"standard"'
dagayn tool architecture_analysis_tool --arg mode='"import_cycles"' --arg scope_kind='"directory"'
dagayn tool query_graph_tool --arg pattern='"importers_of"' --arg target='"pkg/a.py"'
```
