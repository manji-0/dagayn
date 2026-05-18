# Features

<!-- constrained-by ./ARCHITECTURE.md -->

`dagayn` is optimized for graph-backed review and exploration, not just static indexing.

## Core capabilities

<!-- derived-from ./ARCHITECTURE.md -->

- full and incremental graph builds
- repo-root-relative graph registration for dagayn workflows
- local SQLite storage
- graph queries for callers, callees, imports, tests, and file summaries
- review context generation and change-impact analysis
- communities, flows, hub nodes, bridge nodes, and knowledge-gap analysis
- refactor previews and dead-code inspection
- wiki generation and graph reports/exports (interactive HTML, GraphML, Mermaid C4, SVG, Cypher, Obsidian)

## Languages and formats the fork emphasizes

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

- application languages such as Python, TypeScript, JavaScript, Go, Rust, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Dart, and more
- repo-adjacent assets such as Markdown, Jupyter notebooks, and Databricks notebook sources/exports
- Terraform as a first-class graph source

## Notable fork-specific additions

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

- commit-pinned Terraform grammar support fetched from the fork at build/test/runtime
- Markdown heading, body, reference, and directive extraction
- stronger mixed-monorepo testing across docs, app code, and infra
- updated CI stack using `ruff` and `ty`

## Operational strengths

<!-- derived-from ./ARCHITECTURE.md#storage-model -->

- no external database required
- works well in terminal-oriented AI workflows
- supports auto-configuration for multiple coding assistants
- keeps graph analysis local by default
