# Plan notes

<!-- constrained-by ../ROADMAP.md -->

This directory stores design notes and implementation plans that are more detailed than the top-level roadmap.

- `ANALYSIS-TOOL-STRATEGY.md` — plan for a smaller, workflow-oriented analysis tool surface
- `ARCHITECTURE-TOOL-TARGET.md` — what `architecture_analysis_tool` is for: the measured noise in today's modes, the declared-unit map plus findings target contract, the finding kinds, and the eval that gates them
- `OXC-PARSER-EVALUATION.md` — whether to parse JavaScript / TypeScript with oxc instead of tree-sitter: measurements, error-recovery gap, and the decision to keep tree-sitter
- `PURE-RUST-MIGRATION.md` — Phase 5: replacing the remaining Python layer with Rust, its measured effects, and the 5.0 fixes that came first
- `REVIEW-TOOL-TARGET.md` — what `review_tool` is for: the measured noise in today's output, the findings-first target contract, the finding kinds, and the eval that gates them
- `RUFF-PYTHON-PARSER.md` — parsing Python with Ruff's parser instead of tree-sitter: the differential comparison, every classified difference, error recovery, and performance
- `SCIP-CALL-RESOLUTION.md` — taking call targets from SCIP indexers and freezing the type-inference layer above Tree-sitter
- `TREESITTER-TERRAFORM-INTEGRATION.md` — Terraform grammar integration plan for the fork
