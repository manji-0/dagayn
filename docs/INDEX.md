# dagayn documentation index

<!-- constrained-by ./USAGE.md -->
<!-- constrained-by ./COMMANDS.md -->
<!-- constrained-by ./ARCHITECTURE.md -->
<!-- constrained-by ./SCHEMA.md -->
<!-- constrained-by ./FEATURES.md -->

This directory documents the fork as `dagayn`.

## Core references

- [USAGE.md](./USAGE.md) — install, build, update, serve, and operate the graph
- [COMMANDS.md](./COMMANDS.md) — CLI commands, MCP tools, prompts, and exports
- [FEATURES.md](./FEATURES.md) — fork-specific capabilities and practical strengths
- [ARCHITECTURE.md](./ARCHITECTURE.md) — parser, storage, and post-processing pipeline
- [SCHEMA.md](./SCHEMA.md) — graph entities, tables, and stored metadata
- [MARKDOWN-AUTHORING.md](./MARKDOWN-AUTHORING.md) — graph-aware Markdown dependency and directive guidance

## Operation

- [DAEMON-CONFIG.md](./DAEMON-CONFIG.md) — registry and watch daemon file formats
- [GRAMMAR-PROVISIONING.md](./GRAMMAR-PROVISIONING.md) — pinned Tree-sitter grammar provisioning contract
- [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) — common operational fixes
- [LLM-OPTIMIZED-REFERENCE.md](./LLM-OPTIMIZED-REFERENCE.md) — machine-oriented reference sections for tool consumption
- [LEGAL.md](./LEGAL.md) — licensing and local-data notes

## Specifications (WIP / in progress)

- [PERFORMANCE-IMPROVEMENTS-WIP.md](./PERFORMANCE-IMPROVEMENTS-WIP.md) — N+1 query fixes, connection management, and benchmark infrastructure (multiple items shipped; others tracked)
- [SAP-METRICS.md](./SAP-METRICS.md) — Stable Abstractions Principle metrics (implemented)
- [CROSS-ARTIFACT-EDGES-WIP.md](./CROSS-ARTIFACT-EDGES-WIP.md) — cross-artifact edge extraction for cross-language bridges, Markdown↔code, Terraform↔code, etc. (Phase 1+2 shipped; Terraform and manifest bridges WIP)
- [RUST-CORE-MIGRATION-WIP.md](./RUST-CORE-MIGRATION-WIP.md) — spec for replacing the graph engine, post-processing, and parser core with Rust (decisions frozen 2026-04-26; implementation on a separate branch, not yet merged)

## Case studies and direction

- [USECASE-COHESION-REFACTOR.md](./USECASE-COHESION-REFACTOR.md) — observation-driven cohesion refactor using dagayn graph metrics (static case study)
- [ROADMAP.md](./ROADMAP.md) — direction of the fork

## Plan notes

- `docs/plans/README.md` — plan-note index
- `docs/plans/TREESITTER-TERRAFORM-INTEGRATION.md` — Terraform grammar integration design note
