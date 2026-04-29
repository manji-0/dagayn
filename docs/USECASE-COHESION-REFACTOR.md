# Refactoring a codebase by reading its graph, not its files

<!-- derived-from ./ARCHITECTURE.md -->

> **Snapshot:** This is a static case study captured during the cohesion refactoring effort (2026). The code paths described reflect the repository state at that time.

Most refactoring sessions start the same way: someone has a hunch that a file is too
large, or a class is doing too much, or imports look tangled. Then they grep, scroll,
and argue about whether the cleanup is worth doing. The hunch may be right, but the
evidence is qualitative.

This is a write-up of a refactoring session that ran the other way. The prompt was
literally one sentence: "Look at the overall cohesion and stability and propose a
refactoring plan." No file paths, no symptoms, no complaints. Every decision —
what to fix, in what order, and whether the fix worked — came from numerical
metrics produced by dagayn's own MCP tools, applied to dagayn's own codebase.

What follows walks through the metrics dagayn surfaces and what they mean,
the actual readings on this codebase, the priorities those readings forced,
the post-refactor numbers, and the concrete code-level wins those numbers turned into.

## What dagayn measures, and why those numbers matter

dagayn parses your codebase with Tree-sitter and stores the result as a directed graph
in SQLite: nodes are files, classes, functions, and types; edges are calls, imports,
inheritance, containment, and a few other kinds. On top of that graph it derives a
handful of structural metrics. Four of them carried this session.

**Community cohesion.** dagayn runs Leiden community detection on the graph and reports
each community's *cohesion*: the fraction of edges that stay inside the community
versus crossing into another. A cohesion of 1.0 means the community is internally
self-contained; near-zero means it's a loose collection of nodes that happen to have
been clustered together because nothing else fit. Low cohesion on a community that
should logically be a "subsystem" is a signal that the codebase has no real internal
boundary there — Leiden glued everything together because there was nothing structural
to split on.

**Hub nodes.** A hub is a node with unusually high in-degree (many things depend on it)
or out-degree (it depends on many things). High in-degree is *type coupling* or *utility
coupling*: a single symbol is referenced from everywhere, so it cannot be moved or
renamed cheaply. High out-degree is *dispatcher coupling*: one function reaches into
the entire rest of the system. Both make local changes globally expensive.

**Bridge nodes.** A bridge has high *betweenness centrality* — it sits on a large
fraction of the shortest paths between other nodes. Bridges are the chokepoints. If
the graph were a road network, these are the bridges that, when closed, cut off the
most traffic. In code, they are the symbols whose change has the largest blast
radius and whose breakage cascades the furthest.

**Function size.** Not graph-derived, but useful as a sanity check: a function with
several hundred lines is almost always doing several things, and the metrics above
usually point to the same culprits.

The motivating idea is that you can read all four numbers without opening a single
source file. They are derived from the structure of the graph, not from prose
judgments about the code.

## The initial readings

A `dagayn build` on this repository produced a graph with 3518 nodes, 29070 edges, and
194 files. Six MCP tool calls — `list_graph_stats_tool`, `get_architecture_overview_tool`,
`list_communities_tool`, `get_hub_nodes_tool`, `get_bridge_nodes_tool`, and
`find_large_functions_tool` — were enough to surface every problem this session ended
up addressing. No file was read during the observation phase.

The community map was the first surprise. Leiden split the codebase into roughly two
dozen communities, but one of them — labelled `dagayn-tool`, community id 33 — held
**687 nodes**, essentially the entirety of the `dagayn/` package. Its cohesion was
**0.1335**. For context, a healthy modular codebase tends to land its main subsystems
in the 0.3–0.6 range. 0.1335 is not "this subsystem is a bit tangled"; it is "Leiden
could not find a real seam to cut along." Three flat files — `parser.py` (7572 lines),
`graph.py` (1453 lines), and `cli.py` (1252 lines) — sat inside that community with no
internal package boundaries, so the algorithm had nothing to work with.

The hub list confirmed where the gravity was. The top in-degree nodes were `NodeInfo`
(in=191), `EdgeInfo` (in=152), `GraphStore` (in=84), and `CodeParser` (in=50). The
first two are simple data classes — yet they were defined inside the 7572-line
`parser.py`, which meant every consumer of those types pulled in the entire parser
module. The top out-degree node was `cli.py::main`, with **out=320** in a single
function — a 912-line dispatcher that imported, transitively, almost every subsystem.

The bridge list was where the cost showed up. Two nodes dominated betweenness:
`CodeParser` at **0.0209** (rank #1) and `GraphStore` at **0.0165** (rank #3). These
are the chokepoints. When dagayn flags two specific symbols as the #1 and #3 most
load-bearing nodes in your entire graph, what it is really saying is: any change to
either of these symbols is, on average, a change that affects the most paths in the
graph. That is the structural definition of "scary to touch."

The large-function check rounded out the picture: `_parse_rescript` at 405 lines,
`main` at 912 lines, `_extract_from_tree` at 298 lines, `_extract_julia_constructs` at
273 lines. The same files that the structural metrics were complaining about also
held the largest functions. The numbers all pointed in the same direction.

## Setting priorities from the readings

With those numbers on the table, the priority order picked itself.

**Bridges first, because they have the highest blast radius.** `CodeParser` and
`GraphStore` were the top-1 and top-3 chokepoints, and both lived in monolithic files.
Splitting `parser.py` and `graph.py` would not just shrink two files; it would
distribute the betweenness those symbols carried, because the graph would now route
through several smaller modules instead of converging on one.

**Type coupling next, because the fix was cheap and the payoff was disproportionate.**
The single largest in-degree node, `NodeInfo` at 191, lived inside `parser.py` purely
as a side effect of where it was defined. Moving `NodeInfo` and `EdgeInfo` into a
small `parser/types.py` cost almost nothing in code yet broke the dependency from
~340 consumers onto the full parser module. The metric pointed straight at this:
fan-in that high on a pure data class is almost always a symptom of misplacement.

**Dispatcher fan-out last, because it was orthogonal to the Rust migration.**
`cli.py::main` at out=320 was a Python-only surface. The companion document
`docs/RUST-CORE-MIGRATION-WIP.md` (frozen 2026-04-26) commits to migrating
parser, graph, and post-processing to Rust while keeping the CLI in Python, so
trimming `main` was pure Python-side maintainability work — important, but
independent of the parser and graph changes.

The first two priorities also had a second motivation that fell out of reading
`RUST-CORE-MIGRATION-WIP.md`: per-language modules inside `parser/` will map
1:1 with the Rust migration units the spec defines for Phase 3. Splitting now
makes the future Rust port a sequence of small, isolated translations rather
than one large rewrite.

## What changed in the code

Three commits, one per priority. Each was performed by a worktree-isolated sub-agent
so the main session context stayed clean, and each kept the public API intact via
`__init__.py` re-exports — no caller in the repo had to change a line.

| Commit | Before | After |
|--------|--------|-------|
| `0dcbd0c` | `parser.py` (7572 lines) | `parser/` package — `types.py` (data classes), `core.py` (the parser implementation), `__init__.py` (re-exports) |
| `bc30884` | `graph.py` (1453 lines) | `graph/` package — `types.py`, `helpers.py`, `core.py`, `__init__.py` |
| `502f23e` | `cli.py` (1252 lines) with a 912-line `main` | `cli/` package — `app.py` (now a 65-line `main`), `utils.py`, `commands/` (9 subcommand modules) |

The 1319-test suite stayed green at every step. After each commit the graph was
rebuilt with `build_or_update_graph_tool full_rebuild=true` — the incremental
updater does not detect file deletions, so a package-conversion commit needs a
full rebuild before the metrics will reflect the new structure.

## How the readings moved

The same six tools, run again on the rebuilt graph, told a clear story.

**Hub nodes.** The fan-out giant disappeared:

| Node | Before | After |
|------|--------|-------|
| `cli.py::main` | out=322 | `cli/app.py::main`, out=65 (−80%) |
| `CodeParser` | in=173 | `parser/core.py::CodeParser`, in=118 (−32%) |

`NodeInfo`, `EdgeInfo`, and `GraphStore` were no longer concentrated single hubs;
their fan-in had been redistributed across the new sub-modules of the `parser/` and
`graph/` packages. The graph stopped having a single point of import gravity.

**Bridge nodes.** The chokepoints were gone:

| Node | Before | After |
|------|--------|-------|
| `CodeParser` | 0.0209 (rank #1) | dropped out of top-10 |
| `GraphStore` | 0.0165 (rank #3) | dropped out of top-10 |

Both top-tier bridges fell out of the leaderboard. Whatever change happens next to
parsing or storage now has a structurally smaller blast radius, because the shortest
paths between unrelated subsystems no longer route through these symbols.

**Community cohesion.** This one was honest about its limits:

| Community | Before | After |
|-----------|--------|-------|
| `dagayn-tool` | 0.1335 | 0.1297 |

The cohesion barely moved. The reason is that Leiden still sees `dagayn/` as one
big cluster because the *internal* package split — separating per-language parser
modules into `parser/languages/markdown.py`, `parser/languages/terraform.py`, and
so on — has not landed yet. The current commits restructured three flat files into
three packages, but Leiden's edge density inside that whole set is still high enough
that it merges them. Cohesion is a community-shape metric; it lags structural change
and only catches up when the splits go deep enough to give the algorithm a seam.

So this session improved hub and bridge metrics decisively, and left cohesion
queued up for the next round of work. The numbers tell that story plainly, which
is the point.

## What the numbers turned into, in code terms

The metrics improvement is the headline, but the code-level effects are what
actually matter on a day-to-day basis.

`NodeInfo` and `EdgeInfo` now live in a 66-line `parser/types.py`. A consumer
that needs the data classes — and dozens of them do — imports from a 66-line
file instead of accidentally reaching into the entire 7572-line parser. The
import graph is dramatically narrower.

`GraphStore` writes, reads, and helpers were separated into three files that
each have one reason to change. The 84-symbol fan-in onto `GraphStore` is now
spread across the package, so a future change to, say, write semantics no
longer requires loading every read path into your head at once.

The cli `main` shrank from 912 lines to 65. Each subcommand lives in its own
file under `cli/commands/`, exposes a `register` and a `handle`, and can be
read end-to-end in under a screen. Adding a new subcommand is now a copy of an
existing 50-line file instead of a surgical insertion into a 900-line dispatcher.

The Rust migration sequence in `RUST-CORE-MIGRATION-WIP.md` becomes more
tractable. Phase 1 (graph) and Phase 3 (parser) both call out per-file Rust
modules; the Python side now exposes those modules as actual packages, so the
porting work is module-by-module rather than carving slices out of monoliths.

And every one of the changes above was backward-compatible. Not a single import
in any consumer of `dagayn` had to be updated. The `__init__.py` re-exports
preserved the public surface exactly.

## What dagayn made possible that wasn't possible before

Three things stand out from running this kind of session.

The observation phase had effectively zero context cost. Identifying the four
structural problems took six tool calls and read no source files. A hunch-driven
version of the same exercise would have meant reading `parser.py` (7572 lines),
`graph.py` (1453 lines), and `cli.py` (1252 lines) at minimum, just to confirm
they were the actual problem — and the bridge analysis would have been a guess
either way, because betweenness centrality is not something a human eye computes
from scrolling through code.

Verification was numerically falsifiable. "Did this refactor reduce the bridge
betweenness of `CodeParser`?" has a yes-or-no answer, produced by the same tool
that originally surfaced the problem. There was no debate about whether the
refactor "felt cleaner." The metric either moved or it didn't, and you could
see by how much.

Dependency completeness was free. The graph already accounted for transitive
imports, indirect calls, and inheritance edges that a manual read regularly
misses. Knowing that `NodeInfo` had 191 in-edges — not the 10 obvious call sites
visible in a grep — was the kind of fact that prevented under-scoping the type
extraction. You don't have to remember to look for it; the graph just hands it
to you.

## Reusing this pattern

The recipe is short enough to run on any codebase that dagayn supports:

1. Run `dagayn build` to get a fresh graph.
2. Call `list_communities_tool`. Any community with cohesion below ~0.20 and
   more than ~300 nodes is a candidate for splitting — Leiden is telling you it
   couldn't find an internal boundary.
3. Call `get_hub_nodes_tool`. Very high in-degree on a symbol that is *not* a
   facade is type or utility coupling; very high out-degree on a single function
   is dispatcher coupling.
4. Call `get_bridge_nodes_tool`. The top-5 by betweenness are your chokepoints,
   ranked by blast radius.
5. Call `find_large_functions_tool` to cross-check that the structural problems
   correlate with size problems. They usually will.
6. Refactor in priority order: bridges first (largest blast radius), then
   high-fan-in type coupling (cheapest fix per unit of impact), then
   high-fan-out dispatchers (mostly an internal-quality win).
7. After each commit, run `build_or_update_graph_tool full_rebuild=true`.
   File deletions during package conversion are not tracked by the incremental
   updater, so a full rebuild is required before the metrics will be honest.
8. Re-run the same six tools and diff the numbers against the pre-refactor
   baseline. If a metric didn't move, that is itself useful information — it
   tells you the next layer of work that is still queued.

## What's still on the list

The session intentionally stopped after three commits, because each one was
backward-compatible and could be merged independently. The follow-up items
that came out of this round:

- Splitting `parser/core.py` further into per-language modules under
  `parser/languages/`. This is the change expected to finally move the
  `dagayn-tool` community cohesion out of its current 0.13 plateau.
- Decomposing the remaining large helpers — `find_dead_code` (321 lines),
  `_compute_summaries` (226 lines), and the VS Code extension's
  `registerCommands` (685 lines).
- Splitting `tests/test_multilang.py` (2096 lines) once the per-language
  parser modules land, so test files mirror the source structure.

Each of those is a follow-up the metrics will rate honestly when the time
comes to do it.
