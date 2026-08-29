# dagayn documentation index

<!-- constrained-by ./USAGE.md -->
<!-- constrained-by ./COMMANDS.md -->
<!-- constrained-by ./RECIPES.md -->
<!-- constrained-by ./ARCHITECTURE.md -->
<!-- constrained-by ./SCHEMA.md -->
<!-- constrained-by ./FEATURES.md -->
<!-- constrained-by ./EVALUATION-SEMANTICS.md -->
<!-- constrained-by ./SESSION-GRAPH-FRESHNESS.md -->

This directory documents the fork as `dagayn`.

## Core references

- [USAGE.md](./USAGE.md) — install, build, update, serve, and operate the graph
- [COMMANDS.md](./COMMANDS.md) — CLI commands, MCP tools, prompts, and exports
- [FEATURES.md](./FEATURES.md) — fork-specific capabilities and practical strengths
- [ARCHITECTURE.md](./ARCHITECTURE.md) — parser, storage, and post-processing pipeline
- [SCHEMA.md](./SCHEMA.md) — graph entities, tables, and stored metadata
- [MARKDOWN-AUTHORING.md](./MARKDOWN-AUTHORING.md) — graph-aware Markdown dependency and directive guidance
- [EVALUATION-SEMANTICS.md](./EVALUATION-SEMANTICS.md) — metric roles,
  profile summaries, gates, costs, proxy metrics, and semantic report outputs

## Operation

- [RECIPES.md](./RECIPES.md) — copy-paste recipes for watch, registry/daemon, and optional embeddings
- [SESSION-GRAPH-FRESHNESS.md](./SESSION-GRAPH-FRESHNESS.md) — session prepare, worktrees, and MCP first-tool readiness
- [DAEMON-CONFIG.md](./DAEMON-CONFIG.md) — registry and watch daemon file formats
- [LOCAL-EMBEDDINGS.md](./LOCAL-EMBEDDINGS.md) — managed sidecar and local embedding setup
- [GRAMMAR-PROVISIONING.md](./GRAMMAR-PROVISIONING.md) — pinned Tree-sitter grammar provisioning contract
- [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) — common operational fixes
- [LLM-OPTIMIZED-REFERENCE.md](./LLM-OPTIMIZED-REFERENCE.md) — machine-oriented reference sections for tool consumption
- [LEGAL.md](./LEGAL.md) — licensing and local-data notes

## Specifications (WIP / in progress)

- [PERFORMANCE-IMPROVEMENTS-WIP.md](./PERFORMANCE-IMPROVEMENTS-WIP.md) — N+1 query fixes, connection management, and shipped Python-layer benches
- [GRAPH-EFFICIENCY-PLAN.md](./GRAPH-EFFICIENCY-PLAN.md) — scale/query measurement, reverse-CALLS incremental flows, and Rust coarse JSON postprocess
- [SAP-METRICS.md](./SAP-METRICS.md) — Stable Abstractions Principle metrics (implemented)
- [CROSS-ARTIFACT-EDGES-WIP.md](./CROSS-ARTIFACT-EDGES-WIP.md) — cross-artifact edge extraction for cross-language bridges, Markdown↔code, manifest/codegen bridges, Terraform↔code, etc. (Phase 1–3 Layer-2 shipped; Terraform and analysis integration WIP)
- [RUST-CORE-MIGRATION-WIP.md](./RUST-CORE-MIGRATION-WIP.md) — spec for replacing the graph engine, post-processing, and parser core with Rust (Phase 4: Python engine removed; Rust is the only backend)

## Case studies and direction

- [USECASE-COHESION-REFACTOR.md](./USECASE-COHESION-REFACTOR.md) — observation-driven cohesion refactor using dagayn graph metrics (static case study)
- [ROADMAP.md](./ROADMAP.md) — direction of the fork

## Plan notes

- `docs/plans/README.md` — plan-note index
- `docs/plans/TREESITTER-TERRAFORM-INTEGRATION.md` — Terraform grammar integration design note
