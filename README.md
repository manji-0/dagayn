# dagayn

> **DAG is All You Need** — a knowledge-graph-centered approach to code review and impact analysis.

`dagayn` is a fork of `code-review-graph` focused on practical AI-assisted review for polyglot repositories, especially infrastructure-heavy codebases.

This fork keeps the graph-centered review model from the upstream project, but it is documented and maintained as its own product. The most visible differences are first-class Terraform support, commit-pinned grammar fetching for fork-specific parsing, broader platform-install flows, and a stronger focus on monorepos that mix application code, docs, and infra.

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

See [NOTICE](NOTICE) for upstream attribution and original author information.

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

## Terraform support

`dagayn` treats Terraform as a first-class language alongside application code. Both `.tf` and `.tfvars` files are parsed by a dedicated Tree-sitter grammar.

### Parsed block types

| Block | Qualified-name pattern | Graph kind |
|---|---|---|
| `resource "type" "name"` | `resource.type.name` | Class |
| `data "type" "name"` | `data.type.name` | Class |
| `variable "name"` | `var.name` | Function |
| `locals { key = … }` | `local.key` (per attribute) | Function |
| `output "name"` | `output.name` | Function |
| `module "name"` | `module.name` | Class |
| `provider "name"` | `provider.name` | Class |
| `terraform {}` | `terraform` | Class |
| `check "name"` | `check.name` | Test |
| `ephemeral "type" "name"` | `ephemeral.type.name` | Class |
| `import {}` | edges only | — |
| `moved {}` | edges only | — |
| `removed {}` | edges only | — |

### Edge types produced

- **REFERENCES** — any `var.x`, `local.x`, `module.x`, `output.x`, `provider.x`, `data.type.name`, or `resource_type.name` expression inside a block body. The parser extracts these with a dedicated regular expression and skips Terraform built-in prefixes (`count`, `each`, `path`, `self`, `terraform`).
- **CALLS** — built-in function calls such as `merge(…)` or `length(…)`.
- **IMPORTS_FROM** — the `source` attribute in `module` and `terraform required_providers` blocks, and the target of `import` blocks.
- **CONTAINS** — file to every block defined in it.
- **DEPENDS_ON** — `required_providers` version constraints in `terraform` blocks.

### Cross-module analysis

When a `module` block references a local path in `source`, `dagayn` records an `IMPORTS_FROM` edge from the calling module to the target directory. This lets impact-radius queries cross module boundaries.

### `.tfvars` files

Variable value files (`.tfvars`) are parsed as Terraform. Their top-level attribute assignments become `var.name` nodes linked to the corresponding `variable` block in `.tf` files via REFERENCES edges, giving the graph a complete picture of variable data flow.

## Markdown support

`dagayn` extracts graph nodes and edges from Markdown documentation alongside source code, so prose architecture decisions and code they describe appear in the same graph.

### Parsed node types

| Element | Qualified-name pattern | Graph kind |
|---|---|---|
| Document | file path | File |
| `# Heading` … `###### Heading` | `file::slug` | Class |
| Setext H1 / H2 (underline style) | `file::slug` | Class |

Heading slugs follow the GitHub Markdown convention: lowercase, spaces and hyphens collapsed to `-`, non-alphanumeric characters removed. Duplicate headings within a file get a numeric suffix (`slug-1`, `slug-2`, …).

### Edge types produced

- **CONTAINS** — heading hierarchy. A level-2 heading that appears under a level-1 heading is recorded as a child of that section.
- **REFERENCES** — inline or reference-style links between sections: `[text](./other.md#heading)` or `[text](#local-heading)`. Source is the containing section; target is resolved to `file::slug` form.
- **IMPORTS_FROM** — cross-file links. When a link or directive points to a different Markdown file, an `IMPORTS_FROM` edge is added from the current file to the target.
- **DEPENDS_ON** — directive comments (see below).

### Directive comments

Directive comments are HTML comments with a structured form that express inter-document dependencies machine-readably:

```markdown
<!-- constrained-by ./decisions/adr-001.md#context -->
<!-- blocked-by ./specs/open-issue.md -->
<!-- supersedes ./old-api.md#endpoint-design -->
<!-- derived-from ./research/background.md#findings -->
```

Supported directive kinds:

| Directive | Meaning |
|---|---|
| `constrained-by` | This section's design is constrained by the referenced document or section |
| `blocked-by` | Implementation is blocked pending the referenced item |
| `supersedes` | This document replaces the referenced content |
| `derived-from` | This section is derived from the referenced source |

Each directive becomes a **DEPENDS_ON** edge. The `markdown_directive_kind` edge attribute records the specific directive type for downstream filtering.

### Link resolution

The parser handles:

- `[text](./relative/path.md#section)` — resolved relative to the source file
- `[text](#local-section)` — resolves to the same file
- `[ref]: path` reference-definition style
- External URLs (`http://`, `https://`, `mailto:`) are ignored

## Installation

```bash
pip install git+https://github.com/manji-0/dagayn.git
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

The graph is stored locally under `.dagayn/` by default. No external database is required.

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
