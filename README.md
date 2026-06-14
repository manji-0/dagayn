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
- GraphML, Mermaid C4, SVG, Cypher, and Obsidian graph exports

## Supported languages and file types

`dagayn` covers mainstream application languages plus repo-adjacent formats.

Highlights include:

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, GDScript, Vue, Svelte, Astro
- Markdown
- Jupyter notebooks and Databricks notebook sources/exports as graph inputs
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
| `# Heading` … `###### Heading` | `file::slug` | DocSection |
| Setext H1 / H2 (underline style) | `file::slug` | DocSection |
| Paragraph/list/table/code body under a heading | `file::slug--body-N` | DocBody |

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
pip install dagayn
```

For a persistent isolated CLI environment, `uv tool install` works too:

```bash
uv tool install dagayn
```

For an isolated one-shot CLI, `uvx` works well:

```bash
uvx --from dagayn dagayn --help
```

Published wheels include the compiled extension for supported targets, so the
normal PyPI install paths do not require building from the Git repository.

If you prefer persistent isolated tool installs, `pipx` also works.

## Quick start

```bash
dagayn install
dagayn build
dagayn status
```

`install` auto-detects supported AI coding platforms and writes MCP configuration where appropriate.  Run without arguments on a TTY to be prompted for an embedding mode (see below); under `-y` or a non-TTY stdin the mode must be passed explicitly.

`build` creates the initial graph.

Use `dagayn build --force-full-build` (or `--force`) when you want to delete the
existing graph database before rebuilding from scratch.

`status` confirms the graph exists and reports basic counts.

### Choosing an install mode

`dagayn install` supports these embedding strategies as first-class options:

```bash
# 1. FTS only — no embeddings, fastest, no model download.
dagayn install --mode fts-only

# 2. Local — managed BGE-M3 llama.cpp GGUF sidecar.
dagayn install --mode local-embedding

# 3. Managed Qwen3 llama.cpp GGUF sidecar.
dagayn install --mode local-embedding-llama --preset low    # Qwen3-Embedding-0.6B (~1 GB)

# 4. Remote — OpenAI-compatible / Google / MiniMax cloud embeddings.
dagayn install --mode remote-embedding --provider openai
dagayn install --mode remote-embedding --provider google
dagayn install --mode remote-embedding --provider minimax
```

For `--mode remote-embedding`, set the provider's environment variables in the shell that launches your AI coding tool (e.g. `CRG_OPENAI_API_KEY`, `CRG_OPENAI_BASE_URL`, `CRG_OPENAI_MODEL` for `openai`); the MCP server inherits those at launch time and the generated `dagayn serve --remote-embedding <provider>` entry makes MCP search use that provider automatically.  The exact env-var list is printed at install time.  Legacy install shortcuts such as `--mode fts`, `--mode local`, `--mode local --preset low`, `--mode llama-qwen3`, `--mode remote`, and `--local-embedding low` still work as aliases for the new explicit mode names.

### Rust backend

<!-- derived-from ./docs/USAGE.md#use-the-rust-backend -->

The Rust-backed graph store and Rust-owned parser paths are the default for
Markdown, Terraform, Rust, Python/notebooks, and
Bash/Go/Java/Ruby/C#/PHP/Kotlin/Swift/Scala/Solidity/Dart/Lua/Luau/C/C headers/Perl XS/C++/Objective-C/Elixir/GDScript/R/Julia/Perl/Vue/Svelte/Zig/PowerShell, extensionless shebang scripts for supported scripting languages, plus core JavaScript/JSX/TypeScript/TSX and Astro files:

```bash
dagayn build
dagayn update
```

Source checkouts without the native extension now fail clearly instead of
falling back to the removed Python parser implementation.

## Common CLI flows

```bash
dagayn build
dagayn update
dagayn watch
dagayn detect-changes --base HEAD~1
dagayn visualize --format graphml
dagayn serve
```

### MCP tool surface

<!-- derived-from ./docs/COMMANDS.md#mcp-tool-surface -->

`dagayn serve` exposes every public MCP main tool by default. Workflow-specific
analysis is routed through dispatcher tools such as `review_tool`,
`flow_tool`, and `architecture_analysis_tool`, so routine sessions no longer
need named server profiles.

```bash
dagayn serve
dagayn serve --tools query_graph_tool,semantic_search_nodes_tool
```

`--tools` is an exact comma-separated allow-list for deployments that need to
hide some public tools. Persistent server configs can use `CRG_TOOLS` for the
same control.

Tool responses use a calibrated guidance contract. Compatibility fields such as
`status`, `summary`, `_hints`, and `next_tool_suggestions` remain, while review,
architecture, flow, refactor, search, and query responses can also include
`guidance`, `answerability`, and `missingness`. Guidance items carry `claim`,
`evidence`, `confidence`, `missingness`, `action`, `reason_codes`, and `counts`
so agents can treat graph output as evidence-ranked leads rather than verdicts.
Use `detail_level="minimal"` for the top recommendations and
`detail_level="standard"` for the full supporting sections.
`query_graph_tool` zero-result and not-found responses include
`zero_result_reason`, `next_action`, `result_count`, `results`,
`answerability`, and `missingness`; treat absence as graph-limited until source
or tests confirm it. Documentation bridge results label evidence as `authored`,
`extracted`, or `heuristic_reachable` so Markdown traceability is not confused
with a verified contract.

## Reporting and export outputs

`dagayn visualize` exports static graph artifacts.

- `--format` is required and supports `graphml`, `mermaid-c4`, `svg`, `cypher`, and `obsidian`
- `mermaid-c4` emits Mermaid `C4Component` code with files collapsed into components and cross-file relations
- `svg` export uses matplotlib, so install the eval extra when you need it: `pip install "dagayn[eval]"`
- Jupyter / Databricks notebooks are parsed as graph inputs, not emitted as report formats

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
- Pi
- Hermes Agent

You can limit installation to a single platform with `--platform <name>`.
For Codex, install also creates global `~/.codex/hooks.json` and enables
hooks in `~/.codex/config.toml` so the graph refreshes during Codex sessions.
Claude hooks are written to global `~/.claude/settings.json`. Installed git
hooks run `dagayn update --skip-flows` before commit-time checks and a full
`dagayn update` after each commit. When a local embedding install mode is
selected, generated AI-tool update hooks also pass the same local embedding
sidecar arguments so edit-time refreshes keep vectors current.

Platform-specific instruction files are also installed where needed:

- Claude uses `~/.claude/CLAUDE.md`
- Codex uses `~/.codex/AGENTS.md`
- OpenCode uses `~/.config/opencode/AGENTS.md`
- Qoder uses `QODER.md`
- `--platform qcoder` is accepted as an alias for `qoder`

## How the graph is used

A typical review loop looks like this:

1. build or update the graph
2. ask for minimal context or a change review
3. inspect only the affected files and symbols
4. follow communities, flows, or cross-file references as needed
5. refresh incrementally after edits

The graph is stored locally under `.dagayn/` by default. No external database is required.

## Semantic search and embeddings

<!-- derived-from ./docs/ARCHITECTURE.md#hybrid-search -->

`semantic_search_nodes` combines exact/name search with embedding-backed fuzzy
search when embeddings are available, and falls back to FTS-only search when
they are not. It reports which search path contributed through `search_mode`
and per-result `source` fields.

For implementation details such as FTS indexing, RRF merge, reranking, text
modes, and provider setup, see
[`docs/ARCHITECTURE.md#hybrid-search`](docs/ARCHITECTURE.md#hybrid-search) and
[`docs/LOCAL-EMBEDDINGS.md`](docs/LOCAL-EMBEDDINGS.md).

### Providers

| Provider | Runs where | Extra install | Required env vars |
|---|---|---|---|
| `local` (default) | Fully offline | included in standard dependencies | — |
| `openai` | Cloud or self-hosted gateway | — | `CRG_OPENAI_API_KEY`, `CRG_OPENAI_BASE_URL`, `CRG_OPENAI_MODEL` |
| `google` | Google Cloud | `dagayn[google-embeddings]` | `GOOGLE_API_KEY` |
| `minimax` | MiniMax Cloud | — | `MINIMAX_API_KEY` |

The `openai` provider speaks the standard `/v1/embeddings` schema, so it works with real OpenAI, Azure OpenAI, LiteLLM, vLLM, LocalAI, Ollama (in OpenAI mode), and similar gateways. When `CRG_OPENAI_BASE_URL` points to localhost the cloud egress warning is suppressed automatically.

Vector search uses numpy by default for the cosine-similarity matrix path.
`dagayn serve --local-embedding` runs BGE-M3 through a managed llama.cpp GGUF
sidecar so Apple Metal acceleration stays out of the Python process. The
built-in sentence-transformers provider remains available for explicit
`provider="local"` embedding runs.

### Running embedding

Call `embed_graph_tool` via MCP (or let your AI agent call it after `build_or_update_graph_tool`). Pass `provider` and optionally `model` to override the defaults.

```
embed_graph_tool(provider="local")
embed_graph_tool(provider="local", model="BAAI/bge-m3")
embed_graph_tool(provider="openai")   # reads CRG_OPENAI_* from env
embed_graph_tool(provider="google")   # reads GOOGLE_API_KEY from env
embed_graph_tool(provider="minimax")  # reads MINIMAX_API_KEY from env
```

Embeddings are stored in the `embeddings` table inside `.dagayn/graph.db`. Switching provider, model, or `DAGAYN_EMBEDDING_TEXT_MODE` partitions the cache and triggers a re-embed for that provider/text-mode pair on the next call.

### Search quality

The current search benchmark has 20 queries: 12 standard queries for exact/name
and purpose-style lookup, plus 8 structural queries for purpose and
process-pattern prose over function behavior.

| Search mode | Query set | MRR | Hit@5 | Hit@20 |
|---|---|---:|---:|---:|
| `material` text | all (20) | 0.5528 | 14/20 | 18/20 |
| `narrative` text | all (20) | 0.6671 | 18/20 | 19/20 |
| intent-routed | all (20) | **0.6725** | **18/20** | **19/20** |

On the 8 structural queries, `narrative` improves over `material` from 0.2881
to 0.5875 MRR and from 3/8 to 7/8 Hit@5. See
[`docs/LOCAL-EMBEDDINGS.md#search-quality`](docs/LOCAL-EMBEDDINGS.md#search-quality)
for the detailed benchmark tables, search-mode notes, and local model
comparison.

### Privacy and cloud egress

Before sending any data to a cloud provider, `dagayn` prints a warning to stderr listing what will be transmitted (function names, docstrings, file paths). To acknowledge once and suppress the warning in subsequent runs:

```bash
export CRG_ACCEPT_CLOUD_EMBEDDINGS=1
```

To stay fully offline, use the `local` provider. No API key or network access is required.

## Documentation map

- `docs/USAGE.md` — installation and day-to-day workflows
- `docs/COMMANDS.md` — CLI, MCP tools, prompts, and exported artifacts
- `docs/FEATURES.md` — what the fork emphasizes and where it differs
- `docs/ARCHITECTURE.md` — parser, storage, and post-processing pipeline
- `docs/SCHEMA.md` — node, edge, and metadata model
- `docs/EVALUATION-SEMANTICS.md` — metric roles, profile summaries, gates,
  costs, and semantic report outputs
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
