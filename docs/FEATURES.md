# Features

`dagayn` is optimized for graph-backed review and exploration, not just static indexing.

## Core capabilities

- full and incremental graph builds
- repo-root-relative graph registration for dagayn workflows
- local SQLite storage
- graph queries for callers, callees, imports, tests, and file summaries
- review context generation and change-impact analysis
- communities, flows, hub nodes, bridge nodes, and knowledge-gap analysis
- refactor previews and dead-code inspection
- wiki generation and visualization exports

## Languages and formats the fork emphasizes

- application languages such as Python, TypeScript, JavaScript, Go, Rust, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Dart, and more
- repo-adjacent assets such as Markdown and notebooks
- Terraform as a first-class graph source

## Notable fork-specific additions

- vendored Terraform grammar support
- Markdown heading, reference, and directive extraction
- stronger mixed-monorepo testing across docs, app code, and infra
- updated CI stack using `ruff` and `ty`

## Operational strengths

- no external database required
- works well in terminal-oriented AI workflows
- supports auto-configuration for multiple coding assistants
- keeps graph analysis local by default
