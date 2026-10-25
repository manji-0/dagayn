# Positioning

<!-- derived-from ./plans/AGENT-WORKFLOW-TARGET.md -->
<!-- derived-from ./plans/TEST-REACH-TARGET.md#evaluation -->
<!-- derived-from ./plans/SCIP-CALL-RESOLUTION.md#measured-comparison -->

This note says what dagayn is for, the evidence behind that choice, and what
follows for each feature. A feature that does not serve the reader named here
needs a measurement before it gets more work.

## Who dagayn is for

dagayn answers repository-structure questions for a coding agent, locally,
over MCP. The agent asks one question per call and gets the whole answer with
a statement of whether it is complete. The questions are the ones a short
script cannot answer:

- transitive callers, callees, importers, and imports
- which tests reach a change
- import cycles and dependencies on less stable units
- which docs depend on a section, and which code implements it
- what a change left dangling or unedited

A change-time review (`review_tool(mode="changes")`) applies the same graph to
a diff. It is not a separate product.

dagayn covers polyglot repositories that mix application code, Markdown, and
Terraform. Terraform stays a headline capability, and the next step is to
measure it on real repositories (see [open measurements](#open-measurements)).

dagayn is not for a human reading a report. It has no visualizations, static
exports, wiki, or editor extension, and it does not rank code by a score.

## Evidence

Each item names its source. The usage counts were taken on 2026-10-25.

- The consumer is the agent. Over 24 Claude Code sessions from one user
  (2026-09 to 2026-10), dagayn received about 120 MCP calls, 59 of them from
  repositories other than dagayn. Outside this repository the CLI ran only
  `build`.
- Those calls went to `review_tool(changes)` 21 times, search 17,
  `callers_of` 13, `dead_code` 9, and the architecture modes about 20.
  `flow_tool` got 2 calls. The wiki, exports, `traverse_graph_tool`, and
  `find_large_functions_tool` got none.
- Agent experiments (Haiku, 100 runs each, kept outside this repository) show
  the gain where no short script answers. Rust questions took 0.74x tokens and
  0.61x time, and TypeScript questions 0.65x time, at equal accuracy. Python
  and Markdown questions took 1.16x to 1.40x tokens, because a short `ast`
  script answers them.
- Test reach is the one capability scored against ground truth (coverage).
  Python untested precision is 0.864. Rust untested precision is 0.387, so
  most Rust "untested" claims are wrong.
- SCIP agreed with dagayn on 788 of 789 Rust nodes, and 215 of 217 package
  disagreements were dagayn's bug.

## What follows

### Quality before breadth

Rust is where dagayn measurably helps, and Rust test reach is the weakest
measured answer. Raising Rust untested precision comes before new languages or
new analyses.

### Call resolution comes from SCIP

Where a SCIP indexer exists, its call targets are authoritative. dagayn's own
receiver-type inference above Tree-sitter is frozen. It gets bug fixes and no
new rules, and a gap it leaves is closed through SCIP.

### Language breadth stays

The dedicated extractors for the other languages stay as they are. They get
bug fixes, and new investment goes to the measured languages first.

### Human-facing surfaces go

The VSCode extension, the static exports (GraphML, Mermaid C4, SVG, Cypher,
Obsidian), HTML `visualize`, and the wiki tools are removed.

### Overlapping tools go

These duplicate an answer another tool already gives:

- `traverse_graph_tool`, which `query_graph_tool` with `depth` replaces
- `find_large_functions_tool`, which `refactor_tool(mode="suggest")`
  `complex_hotspot` replaces
- the SDP and SAP metric modes of `architecture_analysis_tool`, which the
  `unstable_dependency` finding replaces
- the per-function `risk_score` in review replies
- Python `find_bridge_nodes`, whose only callers are tests and one eval
  benchmark; the Rust centrality pass it reads goes once no reader is
  confirmed

### Measured and cut

<!-- derived-from ./plans/OPEN-MEASUREMENTS.md#verdicts -->

These were measured on real repositories and did not help the agent:

- `review_tool` `impact` and `context`: 11 actionable facts beyond `changes`
  in 44 commits, all also returned by `callers_of`
- the `communities` and `community` modes of `architecture_analysis_tool`:
  3 of 35 hand-checked communities useful, against units that already state
  the module boundaries in `mode="overview"`
- the `undocumented_surface` finding of `refactor_tool(mode="suggest")`: 3 of
  18 actionable; the rest were self-describing structs or types whose
  variants were already documented

## Open measurements

Each of these keeps its place only if a measurement shows it helps the agent:

- Terraform cross-artifact bridges: false-positive rate on a real Terraform
  repository, which acceptance criterion 4 of
  [the cross-artifact spec](./CROSS-ARTIFACT-EDGES-WIP.md#acceptance-criteria)
  never measured
- embeddings: hybrid search against FTS alone on the same queries
- `flow_tool(mode="entry_points")`, which has no agent experiment
- `refactor_tool(mode="suggest")`
- the multi-repo registry, daemon, and `cross_repo_search_tool`

## How this would be wrong

The positioning fails if the Rust and TypeScript agent experiments do not
replicate, or if `dangling_reference` and `unchanged_caller` never fire on real
history. Neither has fired so far.
