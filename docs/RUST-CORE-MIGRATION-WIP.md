# Rust core migration specification (WIP)

> **Status:** Work in progress. This document describes the intended migration plan for replacing dagayn's parser, graph engine, and post-processing core with a Rust implementation while keeping fork behavior stable.

## Purpose

This specification defines how dagayn should migrate its **core graph pipeline** from Python to Rust without breaking the product contract that existing users and AI tool integrations rely on.

The immediate target is **not** a full product rewrite.

The target is the core pipeline only:

1. parser and language extraction
2. graph storage and query engine
3. post-processing layers such as FTS, flows, and communities

## Non-goals

This WIP spec does **not** require an immediate Rust replacement for:

- CLI install flows
- MCP tool definitions and prompt wiring
- editor/platform config injection
- daemon/watch process management
- embedding provider integrations
- wiki generation and other convenience surfaces

Those surfaces may remain in Python until the Rust core reaches parity.

## Migration strategy

dagayn should follow a **core-first replacement** strategy, not a big-bang rewrite.

Recommended shape:

1. define a stable compatibility contract around graph data and behavior
2. implement a Rust core behind that contract
3. let the existing Python CLI and MCP layers call the Rust core
4. replace outer Python surfaces only after parity is proven

This keeps user-visible behavior stable while moving the performance-sensitive and correctness-critical path first.

## Current contract that must be preserved

The Rust core must preserve these dagayn-specific behaviors:

- **repo-root-relative graph identity** for files and qualified names
- current node and edge kinds, including fork-specific Markdown and Terraform behavior
- SQLite-backed local graph storage
- compatibility with existing post-processing and query flows
- support for incremental updates
- fork-local grammar provisioning rules for pinned Markdown and Terraform grammars

Behavioral compatibility is more important than literal implementation parity.

## Scope boundaries

### In scope for the Rust core

- file discovery and language detection
- Tree-sitter parser orchestration
- notebook cell-aware extraction
- Markdown and Terraform extraction rules
- node and edge normalization
- SQLite schema creation and migration handling
- graph writes and read-side query primitives
- post-processing:
  - full-text search indexes
  - flow derivation
  - community derivation
  - graph statistics and traversal primitives

### Out of scope for the first Rust milestone

- platform install/config mutation
- `dagayn serve` MCP tool registration layer
- editor skill generation
- daemon supervisor logic
- cloud embedding provider wrappers

## Target architecture

The desired end state is a layered split:

### Layer 1: Rust core library

Responsible for:

- parsing supported files
- emitting normalized node and edge records
- reading and writing the graph database
- running post-processing passes
- exposing a stable programmatic API

### Layer 2: Rust execution surface

One or both of:

- a native Rust CLI for build/update/postprocess/status operations
- a stable machine interface callable from Python

Possible integration forms:

- subprocess JSON protocol
- shared SQLite schema with command invocation
- Python extension binding

The initial recommendation is **subprocess + SQLite schema compatibility**, because it minimizes packaging and debugging complexity during migration.

### Layer 3: Python compatibility shell

Python remains responsible for:

- current CLI UX where parity is already good
- FastMCP tool registration
- platform install/config flows
- daemon/watch orchestration
- optional integrations that are not performance-critical

## Compatibility contract

Before implementation starts, the migration should freeze a compatibility contract.

### Graph data contract

The Rust core must preserve:

- the existing SQLite schema or a migration-compatible successor
- node kinds and edge kinds already documented by dagayn
- `extra` metadata fields that downstream tools rely on
- repo-root-relative `file_path` and `qualified_name` normalization

### Parser contract

The Rust parser layer must preserve:

- current language detection rules
- pinned grammar provisioning behavior for Markdown and Terraform
- notebook cell attribution semantics
- Markdown heading slugging and directive-comment dependency extraction
- Terraform block naming and reference extraction rules

### Query contract

The Rust engine must return results compatible enough that existing Python tools can keep their current response shapes with at most thin adapter code.

That includes:

- graph stats
- impact radius primitives
- traversal primitives
- flow inputs
- community inputs
- search/index inputs

## Recommended migration phases

### Phase 0: freeze contracts and parity fixtures

Define:

- canonical fixture repositories
- expected node/edge snapshots
- expected SQLite-level invariants
- acceptable differences policy

This phase should produce parity fixtures for:

- Python-only repositories
- Terraform-only repositories
- Markdown-only repositories
- mixed monorepo layouts combining Markdown, Python, and Terraform
- notebook fixtures

### Phase 1: Rust parser prototype

Build a Rust parser pipeline that can:

- walk a repository
- detect supported languages
- parse files with Tree-sitter
- emit node/edge records in a schema-compatible interchange format

At this stage, Python still owns DB writes and downstream queries.

### Phase 2: Rust graph engine

Move these responsibilities into Rust:

- SQLite open/init/migration flow
- node and edge upserts
- incremental file replacement logic
- path normalization and qualified-name normalization
- base graph statistics

The success criterion is that `build` and `update` can be backed by Rust while preserving the same graph semantics.

### Phase 3: Rust post-processing

Move into Rust:

- FTS rebuilds
- flow derivation
- community derivation
- cached adjacency construction and traversal helpers

At this point, the high-volume and performance-sensitive graph pipeline is mostly in Rust.

### Phase 4: Python compatibility adapters

Refit existing Python surfaces to consume the Rust core:

- `dagayn build`
- `dagayn update`
- `dagayn status`
- MCP tool backends that depend on graph reads

The MCP-facing Python layer should remain thin and mostly translate arguments and output.

### Phase 5: optional outer-surface migration

Only after the core is stable should dagayn decide whether to migrate:

- the main CLI
- MCP server implementation
- install/config editing flows
- daemon/watch features

This should be treated as a separate decision, not an automatic consequence of the core migration.

## Key risks

### 1. Semantic drift in parser output

The largest migration risk is not compilation difficulty; it is **graph drift**.

If the Rust parser changes:

- heading slug rules
- Terraform reference extraction
- notebook cell attribution
- path normalization
- type edge semantics

then downstream analysis will become inconsistent even if the system appears to work.

### 2. SQLite compatibility breakage

If the Rust graph engine changes write ordering, uniqueness rules, normalization rules, or migration behavior without a clear compatibility plan, existing Python tools may silently misbehave.

### 3. Incremental update regressions

Incremental update logic is harder than full rebuild logic. The Rust migration should assume that incremental parity needs dedicated fixtures and failure injection tests.

### 4. Post-process divergence

Flows, communities, and search indexes are derived products. Small graph differences can cause large downstream differences, especially in community boundaries and impact paths.

### 5. Packaging and grammar build complexity

Rust adds a second toolchain and new packaging expectations. Tree-sitter grammar provisioning, local builds, CI, and platform distribution all become more complex if the rollout shape is not constrained.

### 6. Product-surface distraction

Rewriting outer Python surfaces too early would absorb time into install UX, daemon behavior, and MCP framework details before the core migration proves value.

## Feasibility assessment

### What is a strong Rust candidate

- parser orchestration
- normalized record extraction
- SQLite-heavy graph operations
- incremental graph mutation
- FTS rebuilds
- flow/community computation
- traversal primitives

These are deterministic, performance-sensitive, and easier to regression-test against fixtures.

### What should stay in Python initially

- FastMCP registration layer
- editor/platform integration logic
- daemon/watch process supervision
- optional embedding providers

These are integration-heavy and not the primary performance bottleneck.

## Interface recommendation

The first Rust adoption step should prefer a **coarse-grained interface** over fine-grained FFI.

Recommended first interface:

- `dagayn-core build`
- `dagayn-core update`
- `dagayn-core postprocess`
- `dagayn-core stats`

Each command should operate on the same repo-root-relative graph layout and SQLite database that dagayn already uses.

Why this is preferred first:

- easier debugging
- easier CI setup
- lower Python packaging risk
- simpler rollback path
- easier A/B comparison with the Python implementation

## Acceptance criteria

The Rust core should not replace the Python core until all of these are true:

1. graph snapshots match for canonical fixtures within an explicitly documented tolerance
2. mixed monorepo fixtures preserve repo-root-relative graph identity
3. Markdown and Terraform fork behavior matches documented dagayn behavior
4. incremental updates match full rebuild semantics on parity fixtures
5. post-process outputs are stable enough for existing review/query tools
6. Python CLI and MCP layers can switch backends without response-shape churn

## Open questions

- whether the Rust core should own schema migrations directly from day one
- whether community detection should use the same algorithm implementation or only compatible output semantics
- whether embeddings metadata should remain fully Python-owned even if search indexes move to Rust
- whether long-term packaging should use a separate binary, a Python extension, or both
- how much language support should be migrated at first release versus staged by language family

## Current recommendation

dagayn should proceed only with a **staged Rust core migration**.

The preferred order is:

1. freeze graph and parser contracts
2. build a Rust parser/export prototype
3. move graph writes and incremental updates
4. move post-processing
5. keep Python as the compatibility shell until parity is proven

That path gives dagayn the upside of Rust where it matters most without turning the migration into a product rewrite.
