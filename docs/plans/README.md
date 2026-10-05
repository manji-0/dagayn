# Plan notes

<!-- constrained-by ../ROADMAP.md -->

This directory stores design notes and implementation plans that are more detailed than the top-level roadmap.

- `ANALYSIS-TOOL-STRATEGY.md` — plan for a smaller, workflow-oriented analysis tool surface
- `OXC-PARSER-EVALUATION.md` — whether to parse JavaScript / TypeScript with oxc instead of tree-sitter: measurements, error-recovery gap, and the decision to keep tree-sitter
- `PURE-RUST-MIGRATION.md` — Phase 5: replacing the remaining Python layer with Rust, its measured effects, and the 5.0 fixes that came first
- `SCIP-CALL-RESOLUTION.md` — taking call targets from SCIP indexers and freezing the type-inference layer above Tree-sitter
- `TREESITTER-TERRAFORM-INTEGRATION.md` — Terraform grammar integration plan for the fork
