# Usage

<!-- constrained-by ./COMMANDS.md -->

## Install the package

```bash
pip install dagayn
```

For an isolated invocation without a persistent environment:

```bash
uvx --from dagayn dagayn --help
```

To run from the Git repository instead of a published wheel:

```bash
uvx --from git+https://github.com/manji-0/dagayn.git dagayn --help
```

Git/source installs build the PyO3 Rust extension locally. Install a Rust
toolchain and a C compiler first when no prebuilt wheel is available for your
platform.

## Register MCP integration

```bash
dagayn install
```

Useful flags:

- `--platform <name>` to target one integration
- `--dry-run` to preview generated config
- `--no-skills`, `--no-hooks`, `--no-instructions` to skip optional setup steps

## Build and refresh the graph

```bash
dagayn build
dagayn update
dagayn watch
dagayn status
```

Use `build` the first time, `update` for change-driven refreshes, and `watch` during active development.

## Use the Rust backend

The Python backend is still the default. Set `DAGAYN_BACKEND=rust` to use the
Rust-backed graph store and Rust-owned parser paths for Markdown, Terraform,
Rust, Python/notebooks, Bash/Go/Java/Ruby/C#/PHP/Kotlin/Scala, and core JavaScript/JSX/TypeScript/TSX files:

```bash
DAGAYN_BACKEND=rust dagayn build
DAGAYN_BACKEND=rust dagayn update
```

With `uvx`, pass the same environment variable:

```bash
DAGAYN_BACKEND=rust uvx --from dagayn dagayn build
```

## Review changes

```bash
dagayn detect-changes --base HEAD~1
```

In MCP clients, the equivalent workflow usually starts with `detect_changes`, `get_review_context`, or `get_minimal_context`.

## Start the MCP server

```bash
dagayn serve
```

By default the server runs over stdio. Use the HTTP flags if you explicitly need local HTTP transport.

## Visualize or export the graph

```bash
dagayn visualize --serve
dagayn visualize --mode community
dagayn visualize --format graphml
dagayn visualize --format mermaid-c4
dagayn visualize --format svg
dagayn visualize --format cypher
dagayn visualize --format obsidian
```

Notes:

- default output is `.dagayn/graph.html`
- HTML modes are `auto`, `full`, `community`, and `file`
- built-in export formats are `html`, `graphml`, `mermaid-c4`, `svg`, `cypher`, and `obsidian`
- `mermaid-c4` emits Mermaid `C4Component` code using files as components
- Graphviz / DOT is not a built-in export target
- Jupyter / Databricks notebooks are graph inputs, not report outputs
- `svg` export requires matplotlib, available via `dagayn[eval]`

## Multi-repo workflows

```bash
dagayn register /path/to/repo --alias app
dagayn repos
dagayn daemon start
```

The registry is useful when you want cross-repo search or long-running watch management.
