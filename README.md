# dagayn

`dagayn` is a fork of `code-review-graph` focused on practical AI-assisted review for polyglot repositories, especially infrastructure-heavy codebases.

This fork keeps the graph-centered review model from the upstream project, but it is documented and maintained as its own product. The most visible differences are first-class Terraform support, vendored grammar support for fork-specific parsing, broader platform-install flows, and a stronger focus on monorepos that mix application code, docs, and infra.

## What dagayn does

`dagayn` parses your repository into a local SQLite knowledge graph. It records files, symbols, references, call edges, imports, test links, communities, and execution flows. AI agents can query that graph instead of re-reading the whole repository on every task.

In practice, that means:

- smaller review context windows
- faster impact analysis
- safer refactors
- better navigation across large repositories
- a single workflow for code, docs, notebooks, and Terraform

## Fork status

`dagayn` is explicitly a fork of `code-review-graph`.

It does **not** treat upstream documentation as canonical. All project guidance, examples, and command descriptions in this repository are written for `dagayn` itself.

## Highlights

- first-class Terraform parsing for `.tf` and `.tfvars`
- Markdown structure and dependency extraction, including directive comments
- notebook parsing for `.ipynb`
- incremental graph updates and watch mode
- MCP server for AI coding tools
- graph queries for impact radius, review context, communities, flows, and refactors
- multi-repo registry and daemon workflows
- interactive visualization plus GraphML, SVG, Cypher, and Obsidian exports

## Supported languages and file types

`dagayn` covers mainstream application languages plus repo-adjacent formats.

Highlights include:

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, GDScript, Vue, Svelte, Astro, ReScript
- Markdown
- Jupyter notebooks and Databricks-style notebook exports
- Terraform

See `docs/FEATURES.md` and `docs/LLM-OPTIMIZED-REFERENCE.md` for the current coverage summary.

## Installation

```bash
pip install dagayn
```

If you prefer isolated tool installs, `pipx` also works.

## Quick start

```bash
dagayn install
dagayn build
dagayn status
```

`install` auto-detects supported AI coding platforms and writes MCP configuration where appropriate.

`build` creates the initial graph.

`status` confirms the graph exists and reports basic counts.

## Common CLI flows

```bash
dagayn build
dagayn update
dagayn watch
dagayn detect-changes --base HEAD~1
dagayn visualize --serve
dagayn serve
```

The legacy command name `code-review-graph` is still available as a compatibility alias, but `dagayn` is the preferred name in this fork.

## AI platform integration

`dagayn install` can configure MCP for these targets:

- Codex
- Claude / Claude Code
- Cursor
- Windsurf
- Zed
- Continue
- OpenCode
- Antigravity
- Qwen Code
- Kiro
- Qoder

You can limit installation to a single platform with `--platform <name>`.

## How the graph is used

A typical review loop looks like this:

1. build or update the graph
2. ask for minimal context or a change review
3. inspect only the affected files and symbols
4. follow communities, flows, or cross-file references as needed
5. refresh incrementally after edits

The graph is stored locally under `.code-review-graph/` by default. No external database is required.

## Documentation map

- `docs/USAGE.md` — installation and day-to-day workflows
- `docs/COMMANDS.md` — CLI, MCP tools, prompts, and exported artifacts
- `docs/FEATURES.md` — what the fork emphasizes and where it differs
- `docs/architecture.md` — parser, storage, and post-processing pipeline
- `docs/schema.md` — node, edge, and metadata model
- `docs/TROUBLESHOOTING.md` — practical fixes
- `docs/LLM-OPTIMIZED-REFERENCE.md` — machine-oriented reference sections

## Current development direction

The fork currently emphasizes:

- infra-aware review, especially Terraform
- mixed-language monorepos
- stable relative-path graph registration from the repo root
- MCP-first workflows for terminal and editor agents
- reproducible local analysis without hosted services

## Security and privacy

`dagayn` is designed around local graph storage. Some optional embedding providers can call remote APIs, but those flows are opt-in and documented separately.

See `SECURITY.md` and `docs/LEGAL.md` for details.

## Contributing

See `CONTRIBUTING.md` for development setup, verification commands, and contribution rules.

## License

MIT. See `LICENSE`.
