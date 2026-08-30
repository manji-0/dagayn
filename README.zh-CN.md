# dagayn

> **DAG is All You Need** — 以知识图谱为核心的代码审查与影响分析方法。

`dagayn` 是 `code-review-graph` 的一个 fork，专注于为多语言仓库（尤其是基础设施比重较高的代码库）提供实用的 AI 辅助代码审查。

该 fork 保留了上游项目以图谱为中心的审查模型，但作为独立产品进行文档维护。主要差异在于：Terraform 的一等语言支持、用于特定解析的提交固定语法包获取、更广泛的平台安装流程，以及对混合了应用代码、文档和基础设施的单体仓库的更强支持。

## 主要能力

`dagayn` 将仓库解析为本地 SQLite 知识图谱，记录文件、符号、引用、调用边、导入、测试链接、社区和执行流。AI 智能体可以查询该图谱，而无需在每次任务时重新读取整个仓库。

实际效果：

- 更小的审查上下文窗口
- 更快的影响范围分析
- 更安全的重构
- 大型仓库中更好的导航
- 代码、文档、笔记本和 Terraform 的统一工作流

## Fork 状态

`dagayn` 是 `code-review-graph` 的显式 fork。

本仓库不以上游文档为规范。所有项目指引、示例和命令说明均针对 `dagayn` 本身编写。

上游归属和原作者信息请参见 [NOTICE](NOTICE)。

## 主要特性

- 针对 `.tf` 和 `.tfvars` 的一等 Terraform 解析
- Markdown 结构与依赖提取，包括指令注释和 `dagayn:` 文档链接
- `.ipynb` 笔记本与 marimo `.py` / `.md` 笔记本解析
- 原生日文 FTS（Lindera IPADIC 词素 + CJK bigram），屈折查询仍能 AND 匹配
- 增量图谱更新、监听模式、worktree sync 与 session prepare
- 面向 AI 编码工具的 MCP 服务器
- 影响半径、审查上下文、社区、流程和重构的图谱查询
- 原生 Rust 图谱存储、解析器、FTS、流程与后处理（`dagayn._core`）
- 多仓库注册表和守护进程工作流
- GraphML / Mermaid C4 / SVG / Cypher / Obsidian 图谱导出

## 支持的语言和文件类型

除主流应用语言外，还覆盖仓库配套格式。

主要包括：

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C, C++, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, Perl, R, GDScript, Vue, Svelte, Astro
- Markdown
- Jupyter 笔记本、Databricks 笔记本源码/导出、以及 marimo `.py` / `.md` 笔记本作为图谱输入解析
- Terraform

当前覆盖范围摘要请参见 `docs/FEATURES.md` 和 `docs/LLM-OPTIMIZED-REFERENCE.md`。

## Terraform 支持

`dagayn` 将 Terraform 视为与应用代码同等地位的一等语言，使用专用 Tree-sitter 语法解析 `.tf` 和 `.tfvars` 文件。

### 解析的块类型

| 块 | 限定名模式 | 图谱类型 |
|---|---|---|
| `resource "type" "name"` | `resource.type.name` | Class |
| `data "type" "name"` | `data.type.name` | Class |
| `variable "name"` | `var.name` | Function |
| `locals { key = … }` | `local.key`（每个属性） | Function |
| `output "name"` | `output.name` | Function |
| `module "name"` | `module.name` | Class |
| `provider "name"` | `provider.name` | Class |
| `terraform {}` | `terraform` | Class |
| `check "name"` | `check.name` | Test |
| `ephemeral "type" "name"` | `ephemeral.type.name` | Class |
| `import {}` | 仅生成边 | — |
| `moved {}` | 仅生成边 | — |
| `removed {}` | 仅生成边 | — |

### 生成的边类型

- **REFERENCES** — 块体内的 `var.x`、`local.x`、`module.x`、`output.x`、`provider.x`、`data.type.name`、`resource_type.name` 表达式。通过专用正则表达式提取，跳过 Terraform 内置前缀（`count`、`each`、`path`、`self`、`terraform`）。
- **CALLS** — 内置函数调用，如 `merge(…)` 或 `length(…)`。
- **IMPORTS_FROM** — `module` 块和 `terraform required_providers` 的 `source` 属性，以及 `import` 块的目标。
- **CONTAINS** — 文件与其中定义的每个块之间的包含关系。
- **DEPENDS_ON** — `terraform` 块中的 `required_providers` 版本约束。

### 跨模块分析

当 `module` 块的 `source` 引用本地路径时，`dagayn` 从调用模块到目标目录记录 `IMPORTS_FROM` 边。这使影响半径查询能够跨越模块边界。

### `.tfvars` 文件

变量值文件（`.tfvars`）作为 Terraform 解析。顶层属性赋值成为 `var.name` 节点，通过 REFERENCES 边连接到 `.tf` 文件中对应的 `variable` 块，使图谱呈现完整的变量数据流。

## Markdown 支持

`dagayn` 在解析源代码的同时，从 Markdown 文档中提取图谱节点和边，使散文架构决策与其描述的代码出现在同一图谱中。

### 解析的节点类型

| 元素 | 限定名模式 | 图谱类型 |
|---|---|---|
| 文档 | 文件路径 | File |
| `# 标题` ～ `###### 标题` | `file::slug` | DocSection |
| Setext H1 / H2（下划线形式） | `file::slug` | DocSection |
| 标题下的段落/列表/表格/代码正文 | `file::slug--body-N` | DocBody |

标题 slug 遵循 GitHub Markdown 规范：小写化，空格和连字符统一为 `-`，删除非字母数字字符。同一文件中的重复标题会添加数字后缀（`slug-1`、`slug-2`、…）。

### 生成的边类型

- **CONTAINS** — 标题层级结构。出现在一级标题下的二级标题被记录为其子节点。
- **REFERENCES** — 章节间的内联或引用式链接：`[text](./other.md#heading)` 或 `[text](#local-heading)`。源为包含章节，目标解析为 `file::slug` 形式。
- **IMPORTS_FROM** — 跨文件链接。当链接或指令指向另一个 Markdown 文件时，从当前文件到目标添加 `IMPORTS_FROM` 边。
- **DEPENDS_ON** — 指令注释（见下文）。

### 指令注释

指令注释是以结构化形式机器可读地表达文档间依赖关系的 HTML 注释：

```markdown
<!-- constrained-by ./decisions/adr-001.md#context -->
<!-- blocked-by ./specs/open-issue.md -->
<!-- supersedes ./old-api.md#endpoint-design -->
<!-- derived-from ./research/background.md#findings -->
```

支持的指令类型：

| 指令 | 含义 |
|---|---|
| `constrained-by` | 本章节的设计受引用文档/章节约束 |
| `blocked-by` | 实现被引用项阻塞，等待解决 |
| `supersedes` | 本文档替换引用内容 |
| `derived-from` | 本章节派生自引用来源 |

每个指令生成一个 **DEPENDS_ON** 边。边属性 `markdown_directive_kind` 记录具体的指令类型。

### 文档指令（`dagayn:`）

<!-- derived-from ./docs/MARKDOWN-AUTHORING.md -->

`<!-- dagayn: implemented-by path::symbol -->` 形式的 HTML 注释会从 Markdown 章节指向代码（或其他产物）创建 `CROSS_ARTIFACT` 边。支持的类型包括 `implemented-by`、`discusses-artifact` 和 `raises-issue-for`。代码侧可用行注释反向指向，例如 `# dagayn: implements docs/spec.md#Section`。

完整约定见 [`docs/MARKDOWN-AUTHORING.md`](docs/MARKDOWN-AUTHORING.md)。

### 链接解析

解析器处理的链接形式：

- `[text](./relative/path.md#section)` — 相对于源文件路径解析
- `[text](#local-section)` — 解析为同一文件的章节
- `[ref]: path` — 引用定义形式
- 外部 URL（`http://`、`https://`、`mailto:`）被忽略

## 安装

```bash
pip install dagayn
```

持久隔离的 CLI 环境也可使用 `uv tool install`：

```bash
uv tool install dagayn
```

一次性隔离 CLI 适合用 `uvx`：

```bash
uvx --from dagayn dagayn --help
```

已发布的 wheel 包含受支持目标的编译扩展，因此常规 PyPI 安装路径无需从 Git 仓库构建。

如果偏好隔离的工具安装，也可以使用 `pipx`。

## 快速开始

```bash
dagayn install
dagayn build
dagayn status
```

`install` 自动检测支持的 AI 编码平台并在适当位置写入 MCP 配置。在 TTY 上无参数运行时会提示选择嵌入模式（见下文）；在 `-y` 或非 TTY stdin 下必须显式传入模式。

`build` 创建初始图谱。

若要删除现有图谱数据库并从头重建，使用 `dagayn build --force-full-build`（或 `--force`）。

`status` 确认图谱存在并报告基本统计。

### 选择安装模式

`dagayn install` 将以下嵌入策略作为一等选项：

```bash
# 1. 仅 FTS — 无嵌入，最快，无需下载模型。
dagayn install --mode fts-only

# 2. 本地 — 托管的 BGE-M3 llama.cpp GGUF sidecar。
dagayn install --mode local-embedding

# 3. 托管的 Qwen3 llama.cpp GGUF sidecar。
dagayn install --mode local-embedding-llama --preset low    # Qwen3-Embedding-0.6B (~1 GB)

# 4. 远程 — OpenAI 兼容 / Google / MiniMax 云嵌入。
dagayn install --mode remote-embedding --provider openai
dagayn install --mode remote-embedding --provider google
dagayn install --mode remote-embedding --provider minimax
```

对于 `--mode remote-embedding`，请在启动 AI 编码工具的 shell 中设置该提供方的环境变量（`openai` 为 `CRG_OPENAI_API_KEY`、`CRG_OPENAI_BASE_URL`、`CRG_OPENAI_MODEL`）；MCP 服务器在启动时继承这些变量，生成的 `dagayn serve --remote-embedding <provider>` 条目会让 MCP 搜索自动使用该提供方。确切的环境变量列表会在安装时打印。旧的安装快捷方式（`--mode fts`、`--mode local`、`--mode local --preset low`、`--mode llama-qwen3`、`--mode remote`、`--local-embedding low`）仍可作为新显式模式名的别名。

### 原生图谱存储

<!-- derived-from ./docs/USAGE.md#native-graph-store -->

图谱存储、解析器、FTS、流程和后处理运行在原生 Rust 扩展（`dagayn._core`）中。没有可回退的 Python 图谱引擎：`DAGAYN_BACKEND=python` 会被拒绝。混合搜索排序和 manifest-bridge 提取仍留在 Python。

解析器覆盖 Markdown、Terraform、Rust、Python/笔记本、Bash、Go、Java、Ruby、C#、PHP、Kotlin、Swift、Scala、Solidity、Dart、Lua、Luau、C / C 头文件 / Perl XS、C++、Objective-C、Elixir、GDScript、R、Julia、Perl、Vue、Svelte、Zig、PowerShell、受支持脚本语言的无扩展名 shebang 脚本，以及核心 JavaScript / JSX / TypeScript / TSX / Astro 文件：

```bash
dagayn build
dagayn update
```

没有原生扩展的源码检出将明确失败。

## 常用 CLI 流程

```bash
dagayn build
dagayn update
dagayn watch
dagayn worktree sync
dagayn detect-changes --base HEAD~1
dagayn visualize --format graphml
dagayn serve
```

### MCP 工具表面

<!-- derived-from ./docs/COMMANDS.md#mcp-tool-surface -->

`dagayn serve` 暴露紧凑的默认工作流表面：主要工具加上 `review_tool`、`flow_tool`、`architecture_analysis_tool` 等分发器，日常会话不再需要命名的服务器配置文件。

```bash
dagayn serve
dagayn serve --tools query_graph_tool,semantic_search_nodes_tool
```

`--tools` 是精确的逗号分隔允许列表，用于需要隐藏部分公开工具的部署。持久服务器配置可用 `CRG_TOOLS` 做同样的控制。

工具响使用校准后的 guidance 约定。兼容字段如 `status`、`summary`、`_hints`、`next_tool_suggestions` 仍然存在；审查、架构、流程、重构、搜索和查询响应还可包含 `guidance`、`answerability` 和 `missingness`。guidance 项带有 `claim`、`evidence`、`confidence`、`missingness`、`action`、`reason_codes` 和 `counts`，智能体应将图谱输出视为按证据排序的线索而非裁决。顶级建议使用 `detail_level="minimal"`，完整支撑章节使用 `detail_level="standard"`。`query_graph_tool` 的零结果和未找到响应包含 `zero_result_reason`、`next_action`、`result_count`、`results`、`answerability` 和 `missingness`；在源码或测试确认之前，将缺席视为受当前图谱限制。文档桥接结果将证据标记为 `authored`、`extracted` 或 `heuristic_reachable`，以免把 Markdown 可追溯性与已验证契约混淆。

## 导出输出

`dagayn visualize` 用于导出静态图谱产物。

- `--format` 是必需的，支持 `graphml`、`mermaid-c4`、`svg`、`cypher`、`obsidian`
- `mermaid-c4` 输出 Mermaid `C4Component` 代码，将文件折叠为组件、跨文件关系作为关系
- `svg` 使用 matplotlib，需要时安装 eval extra：`pip install "dagayn[eval]"`
- Jupyter / Databricks / marimo 笔记本是图谱输入，不是报告输出格式

## AI 平台集成

`dagayn install` 可为这些目标配置 MCP：

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

可用 `--platform <name>` 限定到单一平台。
对于 Codex，安装还会创建全局 `~/.codex/hooks.json` 并在 `~/.codex/config.toml` 中启用 hooks，以便 Codex 会话期间刷新图谱。Claude hooks 写入全局 `~/.claude/settings.json`。安装的 git hooks 在提交前检查运行 `dagayn update --skip-flows`，提交后运行完整 `dagayn update`。选择本地嵌入安装模式时，生成的 AI 工具更新 hooks 也会传入相同的本地嵌入 sidecar 参数，使编辑时刷新保持向量最新。

还会按需安装平台特定的指令文件：

- Claude 使用 `~/.claude/CLAUDE.md`
- Codex 使用 `~/.codex/AGENTS.md`
- OpenCode 使用 `~/.config/opencode/AGENTS.md`
- Qoder 使用 `QODER.md`
- `--platform qcoder` 作为 `qoder` 的别名被接受

## 图谱的使用方式

典型审查循环：

1. 构建或更新图谱
2. 请求最小上下文或变更审查
3. 仅检查受影响的文件和符号
4. 必要时跟随社区、流程或跨文件引用
5. 编辑后增量刷新

图谱默认本地存储在 `.dagayn/` 下。不需要外部数据库。

## 语义搜索与嵌入

<!-- derived-from ./docs/ARCHITECTURE.md#hybrid-search -->

`semantic_search_nodes` 在有嵌入时将精确/名称搜索与嵌入模糊搜索结合，没有嵌入时回退到仅 FTS。它通过 `search_mode` 和每条结果的 `source` 报告搜索路径。原生 FTS 用 Lindera IPADIC 词素（外加词典基本形）和重叠 CJK bigram 切分日文，因此像 `検索する` 这样的屈折查询会 AND 匹配 `検索を行う`。

FTS 索引、RRF 合并、重排序、文本模式和提供方设置等实现细节见
[`docs/ARCHITECTURE.md#hybrid-search`](docs/ARCHITECTURE.md#hybrid-search) 和
[`docs/LOCAL-EMBEDDINGS.md`](docs/LOCAL-EMBEDDINGS.md)。

### 嵌入模式与提供方

| 模式/提供方 | 运行位置 | 额外安装 | 所需环境变量 |
|---|---|---|---|
| `--local-embedding` | 托管的 localhost llama-server GGUF sidecar | — | — |
| `openai` | 云或自托管网关 | — | `CRG_OPENAI_API_KEY`、`CRG_OPENAI_BASE_URL`、`CRG_OPENAI_MODEL` |
| `google` | Google Cloud | `dagayn[google-embeddings]` | `GOOGLE_API_KEY` |
| `minimax` | MiniMax Cloud | — | `MINIMAX_API_KEY` |

`openai` 提供方使用标准 `/v1/embeddings` 模式，因此可用于真正的 OpenAI、Azure OpenAI、LiteLLM、vLLM、LocalAI、Ollama（OpenAI 模式）等类似网关。当 `CRG_OPENAI_BASE_URL` 指向 localhost 时，会自动抑制云出口警告。

向量搜索默认使用 Rust 原生余弦相似度后端。它在 Rust 中用架构特定 SIMD 计算点积（aarch64 上为 NEON，x86_64 上为 AVX 与 SSE 回退，其他为标量），因此不需要外部 BLAS 或 Accelerate。当原生搜索不可用时设置 `DAGAYN_EMBEDDING_SEARCH_BACKEND=auto` 回退到 Python 路径，或用 `DAGAYN_EMBEDDING_SEARCH_BACKEND=python` 做 A/B 测试。Python 路径在安装 numpy 时使用可选 BLAS matmul（`pip install "dagayn[numpy]"`），否则使用纯 Python 余弦循环 — numpy 从来不是必需的硬依赖。
`dagayn serve --local-embedding` 通过托管的 llama.cpp GGUF sidecar 运行 BGE-M3，使加速留在 Python 进程之外。旧的 sentence-transformers/PyTorch `provider="local"` 模式已移除；本地嵌入现在指托管的 llama-server sidecar 或另一个 localhost 上的 OpenAI 兼容端点。

### 运行嵌入

通过 MCP 调用 `embed_graph_tool`（或让 AI 智能体在 `build_or_update_graph_tool` 之后调用）。完全本地嵌入请优先使用 `dagayn build --local-embedding`、`dagayn update --local-embedding` 或 `dagayn serve --local-embedding`；它们会管理 llama-server，然后在内部使用 OpenAI 兼容的 localhost 端点。仅在使用已配置的提供方时传递 `provider` 和可选的 `model`。

```
dagayn build --local-embedding
embed_graph_tool(provider="openai")   # 读取环境中的 CRG_OPENAI_*
embed_graph_tool(provider="google")   # 读取环境中的 GOOGLE_API_KEY
embed_graph_tool(provider="minimax")  # 读取环境中的 MINIMAX_API_KEY
```

嵌入存储在 `.dagayn/graph.db` 的 `embeddings` 表中。切换提供方、模型或 `DAGAYN_EMBEDDING_TEXT_MODE` 会分区缓存，并在下次调用时为该提供方/文本模式对触发重新嵌入。

### 搜索质量

当前搜索基准有 20 条查询：12 条标准查询用于精确/名称和目的式查找，加上 8 条结构查询用于函数行为上的目的和过程模式散文。

| 搜索模式 | 查询集 | MRR | Hit@5 | Hit@20 |
|---|---|---:|---:|---:|
| `material` 文本 | 全部 (20) | 0.5528 | 14/20 | 18/20 |
| `narrative` 文本 | 全部 (20) | 0.6671 | 18/20 | 19/20 |
| intent-routed | 全部 (20) | **0.6725** | **18/20** | **19/20** |

在 8 条结构查询上，`narrative` 相对 `material` 将 MRR 从 0.2881 提升到 0.5875，Hit@5 从 3/8 提升到 7/8。详见
[`docs/LOCAL-EMBEDDINGS.md#search-quality`](docs/LOCAL-EMBEDDINGS.md#search-quality)
中的基准表、搜索模式说明和本地模型比较。

### 隐私与云出口

在向云提供方发送任何数据之前，`dagayn` 会向 stderr 打印警告，列出将传输的内容（函数名、文档字符串、文件路径）。若要一次性确认并在后续运行中抑制警告：

```bash
export CRG_ACCEPT_CLOUD_EMBEDDINGS=1
```

若要完全离线，使用 `--local-embedding` 让 dagayn 管理 localhost 上的 llama-server 端点。不需要 Python ML 栈或 PyTorch 依赖。

## 文档地图

- `docs/USAGE.md` — 安装与日常工作流
- `docs/RECIPES.md` — watch、注册表/守护进程和嵌入的复制粘贴配方
- `docs/COMMANDS.md` — CLI、MCP 工具、提示词和导出产物
- `docs/FEATURES.md` — fork 的重点以及与上游的差异
- `docs/ARCHITECTURE.md` — 解析器、存储和后处理管道
- `docs/SCHEMA.md` — 节点、边和元数据模型
- `docs/MARKDOWN-AUTHORING.md` — 图谱感知的 Markdown 指令和 `dagayn:` 链接
- `docs/SESSION-GRAPH-FRESHNESS.md` — session prepare、worktree 与 MCP 首个工具就绪
- `docs/EVALUATION-SEMANTICS.md` — 指标角色、配置摘要、门控、成本和语义报告输出
- `docs/LOCAL-EMBEDDINGS.md` — 托管 sidecar 与本地嵌入设置
- `docs/DAEMON-CONFIG.md` — 注册表和监听守护进程文件格式
- `docs/TROUBLESHOOTING.md` — 实用修复
- `docs/LLM-OPTIMIZED-REFERENCE.md` — 面向机器的参考章节

## 当前发展方向

该 fork 当前强调：

- 基础设施感知审查，尤其是 Terraform
- 混合语言单体仓库
- 从仓库根目录出发的稳定相对路径图谱注册
- 面向终端和编辑器智能体的 MCP 优先工作流
- 无需托管服务的可复现本地分析

## 安全与隐私

`dagayn` 围绕本地图谱存储设计。部分可选嵌入提供方可调用远程 API，但这些流程是选择加入的，并单独文档化。

详情见 `SECURITY.md` 和 `docs/LEGAL.md`。

## 贡献

开发设置、验证命令和贡献规则见 `CONTRIBUTING.md`。

## 许可

MIT。见 `LICENSE`。
