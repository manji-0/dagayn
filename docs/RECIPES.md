# Ops recipes

<!-- constrained-by ./COMMANDS.md -->
<!-- constrained-by ./LOCAL-EMBEDDINGS.md -->
<!-- constrained-by ./DAEMON-CONFIG.md -->

Short copy-paste recipes for common `dagayn` workflows. Full flag reference:
[COMMANDS.md](./COMMANDS.md). Config file shapes:
[DAEMON-CONFIG.md](./DAEMON-CONFIG.md). Embedding details:
[LOCAL-EMBEDDINGS.md](./LOCAL-EMBEDDINGS.md).

## Single-repo watch / session prepare

<!-- constrained-by ./COMMANDS.md#core-graph-lifecycle -->
<!-- constrained-by ./COMMANDS.md#git-worktrees -->
<!-- derived-from ./USAGE.md#build-and-refresh-the-graph -->

Foreground watch in one repository (auto-update on file changes):

```bash
dagayn build
dagayn watch
```

Session bootstrap used by editor hooks (budgeted structure sync; embeddings
optional):

```bash
dagayn session prepare --budget-seconds 45
dagayn session prepare --budget-seconds 45 --local-embedding
dagayn session prepare --budget-seconds 45 --embedding defer
```

`--embedding auto` (default) runs Phase 2 vectors only when budget remains;
`defer` / `skip` leave embeddings for the next MCP `ensure_graph_tool` /
`get_minimal_context_tool` call; `inline` ignores the remaining-budget gate.
Use `--budget-seconds 0` for no wall-clock limit.

MCP serve for the same repo (stdio; optional local embeddings):

```bash
dagayn serve
dagayn serve --local-embedding
```

## Multi-repo registry → search

<!-- constrained-by ./COMMANDS.md#multi-repo-management -->
<!-- constrained-by ./DAEMON-CONFIG.md#registry-file -->
<!-- derived-from ./USAGE.md#multi-repo-workflows -->

Register repositories, then search across them. Registry file:
`~/.dagayn/registry.json`.

```bash
dagayn register /path/to/app --alias app
dagayn register /path/to/infra --alias infra
dagayn repos
```

Build each repo's graph at least once (registry search skips repos without
`.dagayn/graph.db`):

```bash
dagayn build --repo /path/to/app
dagayn build --repo /path/to/infra
```

List and search via CLI tool surface (`cross_repo_search_tool` is advanced;
default `dagayn serve` does not expose it unless `--tools` includes it or
`all`):

```bash
dagayn tool list_repos_tool
dagayn tool cross_repo_search_tool --arg 'query="authentication handler"'
```

Or expose registry tools from MCP:

```bash
dagayn serve --tools list_repos_tool,cross_repo_search_tool,semantic_search_nodes_tool
# or
dagayn serve --tools all
```

Long-running multi-repo watch daemon (config: `~/.dagayn/watch.toml`):

```bash
dagayn daemon add /path/to/app --alias app
dagayn daemon add /path/to/infra --alias infra
dagayn daemon start
dagayn daemon status
dagayn daemon logs
dagayn daemon stop
```

`dagayn register` updates the search registry; `dagayn daemon add` updates the
watch daemon config. Use both when you want cross-repo search *and* background
watches.

## Optional embedding providers

<!-- constrained-by ./COMMANDS.md#local-embedding-refresh -->
<!-- constrained-by ./LOCAL-EMBEDDINGS.md#local-modes -->
<!-- derived-from ./LOCAL-EMBEDDINGS.md#build-and-update -->

| Goal | Command |
| --- | --- |
| Graph only (no vectors) | `dagayn build` or `dagayn build --local-embedding none` |
| Managed BGE-M3 sidecar (default local) | `dagayn build --local-embedding` |
| Managed Qwen3 sidecar | `dagayn build --local-embedding --mode llama-qwen3` or `dagayn build --local-embedding low` |
| Serve with BGE-M3 | `dagayn serve --local-embedding` |
| Serve with Qwen3 | `dagayn serve --local-embedding --mode llama-qwen3` |
| OpenAI-compatible / cloud for MCP search | `dagayn serve --remote-embedding openai` |

Install modes that bake the same choices into generated MCP config:

```bash
dagayn install --mode fts-only
dagayn install --mode local-embedding
dagayn install --mode local-embedding-llama --preset low
dagayn install --mode remote-embedding --provider openai
```

OpenAI-compatible remote env (shell that launches the AI tool / MCP server):

```bash
export CRG_OPENAI_API_KEY=...
export CRG_OPENAI_BASE_URL=https://api.openai.com/v1   # or a localhost gateway
export CRG_OPENAI_MODEL=text-embedding-3-small
export CRG_ACCEPT_CLOUD_EMBEDDINGS=1                   # suppress egress warning after review
```

`google` needs `GOOGLE_API_KEY` (and `dagayn[google-embeddings]`); `minimax`
needs `MINIMAX_API_KEY`. When `CRG_OPENAI_BASE_URL` points at localhost, the
cloud egress warning is suppressed automatically.

Require `llama-server` on `PATH` for managed local sidecars (`brew install
llama.cpp` or build from source). See
[LOCAL-EMBEDDINGS.md](./LOCAL-EMBEDDINGS.md#qwen-sidecar-setup).

## Failure modes

<!-- derived-from ./LOCAL-EMBEDDINGS.md#troubleshooting -->
<!-- derived-from ./COMMANDS.md#git-worktrees -->

| Symptom | Likely cause | What to do |
| --- | --- | --- |
| `Could not find 'llama-server'` | Sidecar binary missing | Install llama.cpp, or pass `--local-embedding-bin /path/to/llama-server` |
| `unknown value for --flash-attn: '--cache-type-k'` | llama.cpp too old for `--flash-attn on\|off\|auto` | Upgrade llama.cpp, or start the server manually with a supported `--flash-attn` flag |
| Port / incompatible endpoint on `127.0.0.1:18080` | Wrong process, or server not OpenAI `/v1/embeddings` | Stop the other listener, change `--local-embedding-port`, or fix the endpoint |
| Sidecar start timeout | First-run GGUF download or slow host | Raise `--local-embedding-timeout` (default 300), or start `llama-server` manually and watch progress |
| Session prepare skips / defers embeddings | `--budget-seconds` exhausted (`skipped_budget` / `pending`) | Raise budget, use `--embedding inline`, or let MCP `ensure_graph_tool` finish Phase 2 later |
| `cross_repo_search` returns nothing for a repo | Repo not registered, or missing `.dagayn/graph.db` | `dagayn register …`, then `dagayn build --repo …` |
| Remote embedding refused / egress warning | Cloud provider without acknowledgement | Set required `CRG_*` / provider env vars; set `CRG_ACCEPT_CLOUD_EMBEDDINGS=1` only after reviewing the warning |

Do not use embedding-enabled full rebuilds as routine verification. Prefer
`dagayn update --local-embedding none` (or omit `--local-embedding`) for parser
/ flow / docs checks; reserve `dagayn build --force-full-build --local-embedding`
for explicit embedding maintenance.
