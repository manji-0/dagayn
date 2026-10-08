# Plan notes

<!-- constrained-by ../ROADMAP.md -->

This directory stores design notes and implementation plans that are more detailed than the top-level roadmap.

- `AGENT-WORKFLOW-TARGET.md` — the workflow an agent follows across the default tools (orient, locate, read, trace, judge, confirm), the response contract that carries it (one `next`, per-level budgets, discriminating caveats, ambiguity retries), the measured seams, and the eval that gates them
- `ANALYSIS-TOOL-STRATEGY.md` — plan for a smaller, workflow-oriented analysis tool surface
- `ARCHITECTURE-TOOL-TARGET.md` — what `architecture_analysis_tool` is for: the measured noise in today's modes, the declared-unit map plus findings target contract, the finding kinds, and the eval that gates them
- `FLOW-TOOL-TARGET.md` — what `flow_tool` is for: the measured noise in the stored reachable sets, the entry-points-for-a-symbol target contract, and the eval that gates it
- `OXC-PARSER-EVALUATION.md` — whether to parse JavaScript / TypeScript with oxc instead of tree-sitter: measurements, error-recovery gap, and the decision to keep tree-sitter
- `PURE-RUST-MIGRATION.md` — Phase 5: replacing the remaining Python layer with Rust, its measured effects, and the 5.0 fixes that came first
- `REFACTOR-TOOL-TARGET.md` — what `refactor_tool(mode="suggest")` is for: the measured threshold hits, the findings target contract, the finding kinds, and the eval that gates them
- `REVIEW-TOOL-TARGET.md` — what `review_tool` is for: the measured noise in today's output, the findings-first target contract, the finding kinds, and the eval that gates them
- `RUFF-PYTHON-PARSER.md` — parsing Python with Ruff's parser instead of tree-sitter: the differential comparison, every classified difference, error recovery, and performance
- `SCIP-CALL-RESOLUTION.md` — taking call targets from SCIP indexers and freezing the type-inference layer above Tree-sitter
- `STABILITY-FINDING-TARGET.md` — SDP as the finding `unstable_dependency` (every one in the architecture overview, the ones a change introduced in a review), SAP as its evidence, and the eval cases that gate it
- `TEST-REACH-TARGET.md` — tests the call graph cannot see: why a deeper caller walk is wrong (the dispatch hub), the `dagayn: tests` declaration that replaces it, and dispatch-aware reach as the follow-up
- `TREESITTER-TERRAFORM-INTEGRATION.md` — Terraform grammar integration plan for the fork
