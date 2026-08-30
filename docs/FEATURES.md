# Features

<!-- constrained-by ./ARCHITECTURE.md -->

`dagayn` is optimized for graph-backed review and exploration, not just static indexing.

## Core capabilities

<!-- derived-from ./ARCHITECTURE.md -->

- full and incremental graph builds
- repo-root-relative graph registration for dagayn workflows
- local SQLite storage
- graph queries for callers, callees, imports, tests, and file summaries
- native Japanese FTS (Lindera IPADIC morphemes + CJK bigrams) so inflected queries still AND-match
- review context generation and change-impact analysis
- communities, flows, hub nodes, bridge nodes, and knowledge-gap analysis
- refactor previews and dead-code inspection
- wiki generation and static graph exports (GraphML, Mermaid C4, SVG, Cypher, Obsidian)

## Languages and formats the fork emphasizes

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

- application languages such as Python, TypeScript, JavaScript, Go, Rust, Java, C, C++, C#, Ruby, PHP, Kotlin, Swift, Scala, Dart, Perl, R, and more
- repo-adjacent assets such as Markdown, Jupyter notebooks, Databricks notebook sources/exports, and marimo `.py` / `.md` notebooks
- Terraform as a first-class graph source

## Notable fork-specific additions

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

- native Rust graph store (`dagayn._core`); the Python graph engine was removed
- commit-pinned Terraform grammar support fetched from the fork at build/test/runtime
- Terraform → application-code `CROSS_ARTIFACT` bridges (local-exec, Lambda/function
  source paths, `handler` / `entry_point`) with confidence/evidence metadata
- Markdown heading, body, reference, directive, and `dagayn:` documentation-link extraction
- namespace-aware call resolution (shared packages/namespaces, class-declaring files, include/URI imports)
- stronger mixed-monorepo testing across docs, app code, and infra
- updated CI stack using `ruff` and `pyrefly`

## Operational strengths

<!-- derived-from ./ARCHITECTURE.md#storage-model -->

- no external database required
- works well in terminal-oriented AI workflows
- supports auto-configuration for multiple coding assistants
- keeps graph analysis local by default
