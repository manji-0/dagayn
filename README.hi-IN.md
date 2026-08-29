# dagayn

> **DAG is All You Need** — knowledge-graph-centered code review और impact analysis का तरीका।

`dagayn` `code-review-graph` का fork है, polyglot repositories — खासकर infrastructure-heavy codebases — के लिए practical AI-assisted review पर focused।

यह fork upstream के graph-centered review model को रखता है, लेकिन अपने उत्पाद के रूप में documented और maintained है। सबसे दिखाई देने वाले अंतर first-class Terraform support, fork-specific parsing के लिए commit-pinned grammar fetching, broader platform-install flows, और application code, docs, व infra मिलाकर monorepos पर stronger focus हैं।

## dagayn क्या करता है

`dagayn` आपके repository को local SQLite knowledge graph में parse करता है। यह files, symbols, references, call edges, imports, test links, communities, और execution flows रिकॉर्ड करता है। AI agents हर task पर पूरा repository दोबारा पढ़ने की बजाय उस graph से query कर सकते हैं।

व्यवहार में इसका मतलब:

- छोटे review context windows
- तेज़ impact analysis
- सुरक्षित refactors
- बड़े repositories में बेहतर navigation
- code, docs, notebooks, और Terraform के लिए एक ही workflow

## Fork status

`dagayn` स्पष्ट रूप से `code-review-graph` का fork है।

यह upstream documentation को canonical नहीं मानता। इस repository की सारी project guidance, examples, और command descriptions `dagayn` के लिए लिखी गई हैं।

Upstream attribution और original author information के लिए [NOTICE](NOTICE) देखें।

## Highlights

- `.tf` और `.tfvars` के लिए first-class Terraform parsing
- Markdown structure और dependency extraction, directive comments और `dagayn:` documentation links सहित
- `.ipynb` notebook parsing
- native Japanese FTS (Lindera IPADIC morphemes plus CJK bigrams), ताकि inflected queries AND-match करें
- incremental graph updates, watch mode, worktree sync, और session prepare
- AI coding tools के लिए MCP server
- impact radius, review context, communities, flows, और refactors के graph queries
- native Rust graph store, parsers, FTS, flows, और post-processing (`dagayn._core`)
- multi-repo registry और daemon workflows
- GraphML, Mermaid C4, SVG, Cypher, और Obsidian graph exports

## Supported languages और file types

`dagayn` mainstream application languages के साथ repo-adjacent formats कवर करता है।

Highlights:

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C, C++, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, Perl, R, GDScript, Vue, Svelte, Astro
- Markdown
- Jupyter notebooks और Databricks notebook sources/exports graph inputs के रूप में
- Terraform

वर्तमान coverage summary के लिए `docs/FEATURES.md` और `docs/LLM-OPTIMIZED-REFERENCE.md` देखें।

## Terraform support

`dagayn` Terraform को application code के साथ first-class language मानता है। `.tf` और `.tfvars` दोनों dedicated Tree-sitter grammar से parse होते हैं।

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

- **REFERENCES** — block body के अंदर `var.x`, `local.x`, `module.x`, `output.x`, `provider.x`, `data.type.name`, या `resource_type.name` expressions। Parser dedicated regular expression से इन्हें निकालता है और Terraform built-in prefixes (`count`, `each`, `path`, `self`, `terraform`) skip करता है।
- **CALLS** — built-in function calls जैसे `merge(…)` या `length(…)`।
- **IMPORTS_FROM** — `module` और `terraform required_providers` blocks का `source` attribute, और `import` blocks का target।
- **CONTAINS** — file से उसमें defined हर block।
- **DEPENDS_ON** — `terraform` blocks में `required_providers` version constraints।

### Cross-module analysis

जब `module` block `source` में local path reference करता है, `dagayn` calling module से target directory तक `IMPORTS_FROM` edge रिकॉर्ड करता है। इससे impact-radius queries module boundaries पार कर सकती हैं।

### `.tfvars` files

Variable value files (`.tfvars`) Terraform के रूप में parse होती हैं। उनके top-level attribute assignments `var.name` nodes बनते हैं और `.tf` files के corresponding `variable` block से REFERENCES edges से जुड़ते हैं, जिससे graph में variable data flow पूरा दिखता है।

## Markdown support

`dagayn` source code के साथ Markdown documentation से graph nodes और edges निकालता है, ताकि prose architecture decisions और वे code जिसका वे वर्णन करते हैं एक ही graph में दिखें।

### Parsed node types

| Element | Qualified-name pattern | Graph kind |
|---|---|---|
| Document | file path | File |
| `# Heading` … `###### Heading` | `file::slug` | DocSection |
| Setext H1 / H2 (underline style) | `file::slug` | DocSection |
| Paragraph/list/table/code body under a heading | `file::slug--body-N` | DocBody |

Heading slugs GitHub Markdown convention follow करते हैं: lowercase, spaces और hyphens `-` में collapse, non-alphanumeric characters remove। एक file में duplicate headings को numeric suffix मिलता है (`slug-1`, `slug-2`, …)।

### Edge types produced

- **CONTAINS** — heading hierarchy। level-1 heading के नीचे आया level-2 heading उस section का child बनता है।
- **REFERENCES** — sections के बीच inline या reference-style links: `[text](./other.md#heading)` या `[text](#local-heading)`। Source containing section है; target `file::slug` form में resolve होता है।
- **IMPORTS_FROM** — cross-file links। जब link या directive दूसरे Markdown file को point करता है, current file से target तक `IMPORTS_FROM` edge जुड़ती है।
- **DEPENDS_ON** — directive comments (नीचे देखें)।

### Directive comments

Directive comments structured HTML comments हैं जो inter-document dependencies machine-readable रूप में व्यक्त करते हैं:

```markdown
<!-- constrained-by ./decisions/adr-001.md#context -->
<!-- blocked-by ./specs/open-issue.md -->
<!-- supersedes ./old-api.md#endpoint-design -->
<!-- derived-from ./research/background.md#findings -->
```

Supported directive kinds:

| Directive | Meaning |
|---|---|
| `constrained-by` | इस section का design referenced document या section से constrained है |
| `blocked-by` | referenced item pending होने तक implementation blocked है |
| `supersedes` | यह document referenced content को replace करता है |
| `derived-from` | यह section referenced source से derived है |

हर directive एक **DEPENDS_ON** edge बनती है। `markdown_directive_kind` edge attribute specific directive type रिकॉर्ड करता है।

### Documentation directives (`dagayn:`)

<!-- derived-from ./docs/MARKDOWN-AUTHORING.md -->

`<!-- dagayn: implemented-by path::symbol -->` रूप के HTML comments Markdown section से code (या अन्य artifact) target तक `CROSS_ARTIFACT` edges बनाते हैं। Supported kinds में `implemented-by`, `discusses-artifact`, और `raises-issue-for` शामिल हैं। Code दूसरी दिशा `# dagayn: implements docs/spec.md#Section` जैसे line comments से point कर सकता है।

पूरा contract [`docs/MARKDOWN-AUTHORING.md`](docs/MARKDOWN-AUTHORING.md) में है।

### Link resolution

Parser ये handle करता है:

- `[text](./relative/path.md#section)` — source file के relative resolve
- `[text](#local-section)` — उसी file में resolve
- `[ref]: path` reference-definition style
- External URLs (`http://`, `https://`, `mailto:`) ignore किए जाते हैं

## Installation

```bash
pip install dagayn
```

Persistent isolated CLI environment के लिए `uv tool install` भी काम करता है:

```bash
uv tool install dagayn
```

Isolated one-shot CLI के लिए `uvx` सुविधाजनक है:

```bash
uvx --from dagayn dagayn --help
```

Published wheels में supported targets के लिए compiled extension शामिल है, इसलिए सामान्य PyPI install paths को Git repository से build करने की ज़रूरत नहीं।

Isolated tool installs पसंद करते हैं तो `pipx` भी काम करता है।

## Quick start

```bash
dagayn install
dagayn build
dagayn status
```

`install` supported AI coding platforms को auto-detect करता है और उचित जगहों पर MCP configuration लिखता है। बिना arguments TTY पर चलाने पर embedding mode पूछा जाता है (नीचे देखें); `-y` या non-TTY stdin पर mode explicitly देना होता है।

`build` initial graph बनाता है।

मौजूदा graph database मिटाकर scratch से rebuild करने के लिए `dagayn build --force-full-build` (या `--force`) इस्तेमाल करें।

`status` graph के existence की पुष्टि करता है और basic counts report करता है।

### Choosing an install mode

`dagayn install` इन embedding strategies को first-class options के रूप में support करता है:

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

`--mode remote-embedding` के लिए AI coding tool को launch करने वाले shell में provider के environment variables सेट करें (`openai` के लिए `CRG_OPENAI_API_KEY`, `CRG_OPENAI_BASE_URL`, `CRG_OPENAI_MODEL`); MCP server launch time पर उन्हें inherit करता है और generated `dagayn serve --remote-embedding <provider>` entry MCP search को वह provider automatically इस्तेमाल कराती है। Exact env-var list install time पर print होती है। Legacy shortcuts (`--mode fts`, `--mode local`, `--mode local --preset low`, `--mode llama-qwen3`, `--mode remote`, `--local-embedding low`) नए explicit mode names के aliases के रूप में काम करते हैं।

### Native graph store

<!-- derived-from ./docs/USAGE.md#native-graph-store -->

Graph store, parsers, FTS, flows, और post-processing native Rust extension (`dagayn._core`) में चलते हैं। Fall back करने के लिए Python graph engine नहीं है: `DAGAYN_BACKEND=python` reject होता है। Hybrid search ranking और manifest-bridge extraction Python में रहते हैं।

Parsers Markdown, Terraform, Rust, Python/notebooks, Bash, Go, Java, Ruby, C#, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, C / C headers / Perl XS, C++, Objective-C, Elixir, GDScript, R, Julia, Perl, Vue, Svelte, Zig, PowerShell, supported scripting languages के extensionless shebang scripts, और core JavaScript / JSX / TypeScript / TSX / Astro files कवर करते हैं:

```bash
dagayn build
dagayn update
```

Native extension के बिना source checkouts स्पष्ट रूप से fail होते हैं।

## Common CLI flows

```bash
dagayn build
dagayn update
dagayn watch
dagayn worktree sync
dagayn detect-changes --base HEAD~1
dagayn visualize --format graphml
dagayn serve
```

### MCP tool surface

<!-- derived-from ./docs/COMMANDS.md#mcp-tool-surface -->

`dagayn serve` compact default workflow surface expose करता है: मुख्य tools के साथ `review_tool`, `flow_tool`, और `architecture_analysis_tool` जैसे dispatcher tools, इसलिए routine sessions को named server profiles की ज़रूरत नहीं।

```bash
dagayn serve
dagayn serve --tools query_graph_tool,semantic_search_nodes_tool
```

`--tools` deployments के लिए exact comma-separated allow-list है जिन्हें कुछ public tools छिपाने हैं। Persistent server configs उसी control के लिए `CRG_TOOLS` इस्तेमाल कर सकते हैं।

Tool responses calibrated guidance contract इस्तेमाल करते हैं। Compatibility fields जैसे `status`, `summary`, `_hints`, और `next_tool_suggestions` रहते हैं; review, architecture, flow, refactor, search, और query responses में `guidance`, `answerability`, और `missingness` भी हो सकते हैं। Guidance items में `claim`, `evidence`, `confidence`, `missingness`, `action`, `reason_codes`, और `counts` होते हैं ताकि agents graph output को verdict नहीं, evidence-ranked leads मानें। Top recommendations के लिए `detail_level="minimal"` और full supporting sections के लिए `detail_level="standard"` इस्तेमाल करें। `query_graph_tool` zero-result और not-found responses में `zero_result_reason`, `next_action`, `result_count`, `results`, `answerability`, और `missingness` शामिल हैं; absence को source या tests confirm करने तक graph-limited मानें। Documentation bridge results evidence को `authored`, `extracted`, या `heuristic_reachable` लेबल करते हैं ताकि Markdown traceability verified contract से confuse न हो।

## Reporting और export outputs

`dagayn visualize` static graph artifacts export करता है।

- `--format` required है और `graphml`, `mermaid-c4`, `svg`, `cypher`, `obsidian` supported हैं
- `mermaid-c4` Mermaid `C4Component` code emit करता है, जहाँ files components और cross-file dependencies relations बनती हैं
- `svg` export matplotlib use करता है; ज़रूरत हो तो eval extra install करें: `pip install "dagayn[eval]"`
- Jupyter / Databricks notebooks report outputs नहीं, graph inputs हैं

## AI platform integration

`dagayn install` इन targets के लिए MCP configure कर सकता है:

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

`--platform <name>` से किसी एक platform तक सीमित कर सकते हैं।
Codex के लिए install global `~/.codex/hooks.json` भी बनाता है और `~/.codex/config.toml` में hooks enable करता है ताकि Codex sessions के दौरान graph refresh हो। Claude hooks global `~/.claude/settings.json` में लिखे जाते हैं। Installed git hooks commit-time checks से पहले `dagayn update --skip-flows` और हर commit के बाद full `dagayn update` चलाते हैं। Local embedding install mode चुनने पर generated AI-tool update hooks वही local embedding sidecar arguments pass करते हैं ताकि edit-time refreshes vectors current रखें।

Platform-specific instruction files भी जहाँ ज़रूरी हों install होते हैं:

- Claude `~/.claude/CLAUDE.md` इस्तेमाल करता है
- Codex `~/.codex/AGENTS.md` इस्तेमाल करता है
- OpenCode `~/.config/opencode/AGENTS.md` इस्तेमाल करता है
- Qoder `QODER.md` इस्तेमाल करता है
- `--platform qcoder` `qoder` का alias है

## Graph का उपयोग

एक typical review loop इस प्रकार है:

1. Graph build या update करें
2. Minimal context या change review मांगें
3. केवल affected files और symbols inspect करें
4. आवश्यकतानुसार communities, flows, या cross-file references follow करें
5. Edits के बाद incrementally refresh करें

Graph default रूप से `.dagayn/` के अंतर्गत locally store होता है। कोई external database आवश्यक नहीं है।

## Semantic search और embeddings

<!-- derived-from ./docs/ARCHITECTURE.md#hybrid-search -->

`semantic_search_nodes` embeddings उपलब्ध होने पर exact/name search को embedding-backed fuzzy search के साथ जोड़ता है, और नहीं होने पर FTS-only पर fall back करता है। कौन सा search path योगदान दिया, यह `search_mode` और per-result `source` से report होता है। Native FTS Japanese को Lindera IPADIC morphemes (plus dictionary base forms) और overlapping CJK bigrams से segment करता है, इसलिए `検索する` जैसी inflected query `検索を行う` से AND-match करती है।

FTS indexing, RRF merge, reranking, text modes, और provider setup जैसी implementation details के लिए
[`docs/ARCHITECTURE.md#hybrid-search`](docs/ARCHITECTURE.md#hybrid-search) और
[`docs/LOCAL-EMBEDDINGS.md`](docs/LOCAL-EMBEDDINGS.md) देखें।

### Embedding modes और providers

| Mode/provider | Runs where | Extra install | Required env vars |
|---|---|---|---|
| `--local-embedding` | Managed localhost llama-server GGUF sidecar | — | — |
| `openai` | Cloud या self-hosted gateway | — | `CRG_OPENAI_API_KEY`, `CRG_OPENAI_BASE_URL`, `CRG_OPENAI_MODEL` |
| `google` | Google Cloud | `dagayn[google-embeddings]` | `GOOGLE_API_KEY` |
| `minimax` | MiniMax Cloud | — | `MINIMAX_API_KEY` |

`openai` provider standard `/v1/embeddings` schema बोलता है, इसलिए real OpenAI, Azure OpenAI, LiteLLM, vLLM, LocalAI, Ollama (OpenAI mode), और similar gateways पर काम करता है। जब `CRG_OPENAI_BASE_URL` localhost point करता है तो cloud egress warning automatically suppress होती है।

Vector search default में Rust native cosine-similarity backend इस्तेमाल करता है। यह architecture-specific SIMD (aarch64 पर NEON, x86_64 पर AVX with SSE fallback, अन्यत्र scalar) से Rust में ही dot products compute करता है, इसलिए external BLAS या Accelerate नहीं चाहिए। Native search unavailable होने पर Python path के लिए `DAGAYN_EMBEDDING_SEARCH_BACKEND=auto`, A/B testing के लिए `DAGAYN_EMBEDDING_SEARCH_BACKEND=python` सेट करें। Python path numpy install होने पर optional BLAS matmul (`pip install "dagayn[numpy]"`) और वरना pure-Python cosine loop इस्तेमाल करता है — numpy required hard dependency नहीं है।
`dagayn serve --local-embedding` BGE-M3 को managed llama.cpp GGUF sidecar से चलाता है ताकि acceleration Python process के बाहर रहे। पुराना sentence-transformers/PyTorch `provider="local"` mode हटा दिया गया है; local embedding अब managed llama-server sidecar या दूसरा localhost OpenAI-compatible endpoint है।

### Running embedding

MCP से `embed_graph_tool` कॉल करें (या AI agent को `build_or_update_graph_tool` के बाद कॉल करने दें)। Fully local embeddings के लिए `dagayn build --local-embedding`, `dagayn update --local-embedding`, या `dagayn serve --local-embedding` prefer करें; ये llama-server manage करते हैं और internally OpenAI-compatible localhost endpoint इस्तेमाल करते हैं। पहले से configured provider इस्तेमाल करते समय ही `provider` और optionally `model` pass करें।

```
dagayn build --local-embedding
embed_graph_tool(provider="openai")   # env से CRG_OPENAI_* पढ़ता है
embed_graph_tool(provider="google")   # env से GOOGLE_API_KEY पढ़ता है
embed_graph_tool(provider="minimax")  # env से MINIMAX_API_KEY पढ़ता है
```

Embeddings `.dagayn/graph.db` के `embeddings` table में store होते हैं। Provider, model, या `DAGAYN_EMBEDDING_TEXT_MODE` बदलने पर cache partition होता है और अगले call पर उस pair के लिए re-embed चलता है।

### Search quality

वर्तमान search benchmark में 20 queries हैं: exact/name और purpose-style lookup के लिए 12 standard queries, plus function behavior पर purpose और process-pattern prose के लिए 8 structural queries।

| Search mode | Query set | MRR | Hit@5 | Hit@20 |
|---|---|---:|---:|---:|
| `material` text | all (20) | 0.5528 | 14/20 | 18/20 |
| `narrative` text | all (20) | 0.6671 | 18/20 | 19/20 |
| intent-routed | all (20) | **0.6725** | **18/20** | **19/20** |

8 structural queries पर `narrative` `material` से MRR 0.2881 से 0.5875 और Hit@5 3/8 से 7/8 तक improve करता है। Detailed benchmark tables, search-mode notes, और local model comparison के लिए
[`docs/LOCAL-EMBEDDINGS.md#search-quality`](docs/LOCAL-EMBEDDINGS.md#search-quality)
देखें।

### Privacy और cloud egress

Cloud provider को कोई data भेजने से पहले `dagayn` stderr पर warning print करता है जिसमें क्या transmit होगा (function names, docstrings, file paths) listed होता है। एक बार acknowledge करके subsequent runs में warning दबाने के लिए:

```bash
export CRG_ACCEPT_CLOUD_EMBEDDINGS=1
```

पूरी तरह offline रहने के लिए `--local-embedding` इस्तेमाल करें ताकि dagayn localhost llama-server endpoint manage करे। Python ML stack या PyTorch dependency की ज़रूरत नहीं।

## Documentation map

- `docs/USAGE.md` — Installation और day-to-day workflows
- `docs/RECIPES.md` — watch, registry/daemon, और embeddings के copy-paste recipes
- `docs/COMMANDS.md` — CLI, MCP tools, prompts, और exported artifacts
- `docs/FEATURES.md` — Fork क्या emphasize करता है और upstream से कहाँ अलग है
- `docs/ARCHITECTURE.md` — Parser, storage, और post-processing pipeline
- `docs/SCHEMA.md` — Node, edge, और metadata model
- `docs/MARKDOWN-AUTHORING.md` — graph-aware Markdown directives और `dagayn:` links
- `docs/SESSION-GRAPH-FRESHNESS.md` — session prepare, worktrees, और MCP first-tool readiness
- `docs/EVALUATION-SEMANTICS.md` — metric roles, profile summaries, gates, costs, और semantic report outputs
- `docs/LOCAL-EMBEDDINGS.md` — managed sidecar और local embedding setup
- `docs/DAEMON-CONFIG.md` — registry और watch daemon file formats
- `docs/TROUBLESHOOTING.md` — Practical fixes
- `docs/LLM-OPTIMIZED-REFERENCE.md` — Machine-oriented reference sections

## वर्तमान development direction

यह fork इन बातों पर जोर देता है:

- Infra-aware review, विशेष रूप से Terraform
- Mixed-language monorepos
- Repo root से stable relative-path graph registration
- Terminal और editor agents के लिए MCP-first workflows
- Hosted services के बिना reproducible local analysis

## Security और privacy

`dagayn` को local graph storage के इर्द-गिर्द design किया गया है। कुछ optional embedding providers remote APIs call कर सकते हैं, लेकिन वे flows opt-in हैं और अलग से documented हैं।

विवरण के लिए `SECURITY.md` और `docs/LEGAL.md` देखें।

## Contributing

Development setup, verification commands, और contribution rules के लिए `CONTRIBUTING.md` देखें।

## License

MIT. `LICENSE` देखें।
