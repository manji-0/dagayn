# dagayn

> **DAG is All You Need** — कोड समीक्षा और प्रभाव विश्लेषण के लिए knowledge graph केंद्रित दृष्टिकोण।

`dagayn`, `code-review-graph` का एक fork है जो विशेष रूप से बहु-भाषा repositories, खासकर infrastructure-heavy codebases के लिए व्यावहारिक AI-सहायक review पर केंद्रित है।

यह fork upstream project के graph-centered review model को बनाए रखता है, लेकिन इसे एक स्वतंत्र product के रूप में document और maintain किया जाता है। मुख्य अंतर हैं: Terraform की first-class support, fork-specific parsing के लिए commit-pinned grammar fetching, व्यापक platform-install flows, और ऐसे monorepos पर अधिक ध्यान जो application code, docs, और infra को मिलाते हैं।

## यह क्या करता है

`dagayn` आपकी repository को एक local SQLite knowledge graph में parse करता है। यह files, symbols, references, call edges, imports, test links, communities, और execution flows को रिकॉर्ड करता है। AI agents हर task पर पूरी repository को दोबारा पढ़ने की बजाय इस graph से query कर सकते हैं।

व्यावहारिक लाभ:

- छोटे review context windows
- तेज impact analysis
- सुरक्षित refactoring
- बड़े repositories में बेहतर navigation
- code, docs, notebooks, और Terraform के लिए एकल workflow

## Fork की स्थिति

`dagayn` स्पष्ट रूप से `code-review-graph` का fork है।

यह upstream documentation को canonical नहीं मानता। इस repository के सभी project guides, examples, और command descriptions `dagayn` के लिए लिखे गए हैं।

Upstream attribution और original author जानकारी के लिए [NOTICE](NOTICE) देखें।

## मुख्य विशेषताएं

- `.tf` और `.tfvars` के लिए first-class Terraform parsing
- directive comments सहित Markdown structure और dependency extraction
- `.ipynb` notebook parsing
- Incremental graph updates और watch mode
- AI coding tools के लिए MCP server
- impact radius, review context, communities, flows, और refactors के लिए graph queries
- Multi-repo registry और daemon workflows
- Interactive visualization के साथ GraphML / SVG / Cypher / Obsidian exports

## समर्थित भाषाएं और file types

मुख्यधारा application भाषाओं के साथ-साथ repository-adjacent formats को cover करता है।

मुख्य भाषाएं:

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, GDScript, Vue, Svelte, Astro, ReScript
- Markdown
- Jupyter notebooks और Databricks-style notebook exports
- Terraform

वर्तमान coverage summary के लिए `docs/FEATURES.md` और `docs/LLM-OPTIMIZED-REFERENCE.md` देखें।

## Terraform Support

`dagayn` Terraform को application code के साथ समकक्ष first-class भाषा के रूप में मानता है। `.tf` और `.tfvars` दोनों files को एक dedicated Tree-sitter grammar से parse किया जाता है।

### Parse किए गए block types

| Block | Qualified-name pattern | Graph kind |
|---|---|---|
| `resource "type" "name"` | `resource.type.name` | Class |
| `data "type" "name"` | `data.type.name` | Class |
| `variable "name"` | `var.name` | Function |
| `locals { key = … }` | `local.key` (प्रति attribute) | Function |
| `output "name"` | `output.name` | Function |
| `module "name"` | `module.name` | Class |
| `provider "name"` | `provider.name` | Class |
| `terraform {}` | `terraform` | Class |
| `check "name"` | `check.name` | Test |
| `ephemeral "type" "name"` | `ephemeral.type.name` | Class |
| `import {}` | केवल edges | — |
| `moved {}` | केवल edges | — |
| `removed {}` | केवल edges | — |

### उत्पन्न edge types

- **REFERENCES** — block body में `var.x`, `local.x`, `module.x`, `output.x`, `provider.x`, `data.type.name`, `resource_type.name` expressions। Dedicated regular expression से extract किया जाता है और Terraform built-in prefixes (`count`, `each`, `path`, `self`, `terraform`) को skip किया जाता है।
- **CALLS** — `merge(…)` या `length(…)` जैसे built-in function calls।
- **IMPORTS_FROM** — `module` block और `terraform required_providers` का `source` attribute, और `import` blocks का target।
- **CONTAINS** — file और उसमें defined प्रत्येक block के बीच containment relationship।
- **DEPENDS_ON** — `terraform` blocks में `required_providers` version constraints।

### Cross-module analysis

जब `module` block का `source` एक local path को reference करता है, `dagayn` calling module से target directory तक एक `IMPORTS_FROM` edge record करता है। इससे impact-radius queries module boundaries को पार कर सकती हैं।

### `.tfvars` files

Variable value files (`.tfvars`) को Terraform के रूप में parse किया जाता है। उनके top-level attribute assignments `var.name` nodes बनते हैं जो REFERENCES edges के माध्यम से `.tf` files में corresponding `variable` blocks से जुड़ते हैं, जिससे graph में variable data flow की पूरी तस्वीर मिलती है।

## Markdown Support

`dagayn` source code के साथ-साथ Markdown documentation से graph nodes और edges extract करता है, ताकि prose architecture decisions और उनके द्वारा वर्णित code एक ही graph में दिखाई दें।

### Parse किए गए node types

| Element | Qualified-name pattern | Graph kind |
|---|---|---|
| Document | file path | File |
| `# Heading` ～ `###### Heading` | `file::slug` | Class |
| Setext H1 / H2 (underline style) | `file::slug` | Class |

Heading slugs GitHub Markdown convention का पालन करते हैं: lowercase, spaces और hyphens को `-` में बदलना, non-alphanumeric characters को हटाना। एक file में duplicate headings को numeric suffix मिलता है (`slug-1`, `slug-2`, …)।

### उत्पन्न edge types

- **CONTAINS** — heading hierarchy। Level-1 heading के नीचे दिखने वाली level-2 heading उसके child के रूप में record होती है।
- **REFERENCES** — sections के बीच inline या reference-style links: `[text](./other.md#heading)` या `[text](#local-heading)`। Source containing section है; target `file::slug` form में resolve होता है।
- **IMPORTS_FROM** — Cross-file links। जब कोई link या directive एक अलग Markdown file को point करता है, तो current file से target तक `IMPORTS_FROM` edge जुड़ती है।
- **DEPENDS_ON** — Directive comments (नीचे देखें)।

### Directive comments

Directive comments machine-readable form में inter-document dependencies express करने वाले structured HTML comments हैं:

```markdown
<!-- constrained-by ./decisions/adr-001.md#context -->
<!-- blocked-by ./specs/open-issue.md -->
<!-- supersedes ./old-api.md#endpoint-design -->
<!-- derived-from ./research/background.md#findings -->
```

समर्थित directive kinds:

| Directive | अर्थ |
|---|---|
| `constrained-by` | इस section का design referenced document/section द्वारा constrained है |
| `blocked-by` | Implementation referenced item के resolve होने तक blocked है |
| `supersedes` | यह document referenced content को replace करता है |
| `derived-from` | यह section referenced source से derived है |

प्रत्येक directive एक **DEPENDS_ON** edge बनता है। Edge attribute `markdown_directive_kind` specific directive type record करता है।

### Link resolution

Parser इन link forms को handle करता है:

- `[text](./relative/path.md#section)` — source file के relative path से resolve होता है
- `[text](#local-section)` — same file के section में resolve होता है
- `[ref]: path` — reference-definition style
- External URLs (`http://`, `https://`, `mailto:`) ignore किए जाते हैं

## Installation

```bash
pip install git+https://github.com/manji-0/dagayn.git
```

Isolated tool installs पसंद करते हैं तो `pipx` भी काम करता है।

## Quick start

```bash
dagayn install
dagayn build
dagayn status
```

`install` supported AI coding platforms को auto-detect करता है और उचित जगहों पर MCP configuration लिखता है।

`build` initial graph बनाता है।

`status` graph के existence की पुष्टि करता है और basic counts report करता है।

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

`--platform <name>` से किसी एक platform तक सीमित कर सकते हैं।

## Graph का उपयोग

एक typical review loop इस प्रकार है:

1. Graph build या update करें
2. Minimal context या change review मांगें
3. केवल affected files और symbols inspect करें
4. आवश्यकतानुसार communities, flows, या cross-file references follow करें
5. Edits के बाद incrementally refresh करें

Graph default रूप से `.dagayn/` के अंतर्गत locally store होता है। कोई external database आवश्यक नहीं है।

## Documentation map

- `docs/USAGE.md` — Installation और day-to-day workflows
- `docs/COMMANDS.md` — CLI, MCP tools, prompts, और exported artifacts
- `docs/FEATURES.md` — Fork क्या emphasize करता है और upstream से कहाँ अलग है
- `docs/architecture.md` — Parser, storage, और post-processing pipeline
- `docs/schema.md` — Node, edge, और metadata model
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
