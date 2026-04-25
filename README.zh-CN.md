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
- Markdown 结构和依赖关系提取，包括指令注释
- `.ipynb` 笔记本解析
- 增量图谱更新与监听模式
- 面向 AI 编码工具的 MCP 服务器
- 影响半径、审查上下文、社区、流程和重构的图谱查询
- 多仓库注册表和守护进程工作流
- 交互式可视化及 GraphML / Mermaid C4 / SVG / Cypher / Obsidian 导出

## 支持的语言和文件类型

除主流应用语言外，还覆盖仓库配套格式。

主要包括：

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, GDScript, Vue, Svelte, Astro, ReScript
- Markdown
- Jupyter 笔记本和 Databricks 笔记本源码/导出会作为图谱输入解析
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
| `# 标题` ～ `###### 标题` | `file::slug` | Class |
| Setext H1 / H2（下划线形式） | `file::slug` | Class |

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

### 链接解析

解析器处理的链接形式：

- `[text](./relative/path.md#section)` — 相对于源文件路径解析
- `[text](#local-section)` — 解析为同一文件的章节
- `[ref]: path` — 引用定义形式
- 外部 URL（`http://`、`https://`、`mailto:`）被忽略

## 安装

```bash
pip install git+https://github.com/manji-0/dagayn.git
```

如果偏好隔离的工具安装，也可以使用 `pipx`。

## 快速开始

```bash
dagayn install
dagayn build
dagayn status
```

`install` 自动检测支持的 AI 编码平台并在适当位置写入 MCP 配置。

`build` 创建初始图谱。

`status` 确认图谱存在并报告基本统计。

## 常用 CLI 流程

```bash
dagayn build
dagayn update
dagayn watch
dagayn detect-changes --base HEAD~1
dagayn visualize --serve
dagayn serve
```

## 报告 / 导出输出

`dagayn visualize` 是当前图谱报告 / 导出的主要命令面。

- 默认输出是 `.dagayn/graph.html` 里的交互式 HTML 报告
- HTML 渲染支持 `--mode auto|full|community|file`
- `--format` 支持 `html`、`graphml`、`mermaid-c4`、`svg`、`cypher`、`obsidian`
- `mermaid-c4` 会输出 Mermaid `C4Component` 代码，并将文件折叠为组件、将跨文件依赖聚合为关系
- `svg` 导出依赖 matplotlib；需要时安装 eval extra：`pip install "dagayn[eval] @ git+https://github.com/manji-0/dagayn.git"`
- 这个 fork 不内置 Graphviz / DOT 导出
- Jupyter / Databricks 笔记本是图谱输入，不是报告输出格式

## AI 平台集成

`dagayn install` 可为以下目标配置 MCP：

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

可使用 `--platform <name>` 限制仅安装特定平台。

## 图谱的使用方式

典型的审查循环如下：

1. 构建或更新图谱
2. 请求最小上下文或变更审查
3. 仅检查受影响的文件和符号
4. 根据需要追踪社区、流程或跨文件引用
5. 编辑后增量刷新

图谱默认本地存储在 `.dagayn/` 目录下，无需外部数据库。

## 文档地图

- `docs/USAGE.md` — 安装和日常工作流
- `docs/COMMANDS.md` — CLI、MCP 工具、提示和导出产物
- `docs/FEATURES.md` — fork 的重点和与上游的差异
- `docs/architecture.md` — 解析器、存储和后处理管道
- `docs/schema.md` — 节点、边和元数据模型
- `docs/TROUBLESHOOTING.md` — 实用修复方法
- `docs/LLM-OPTIMIZED-REFERENCE.md` — 面向机器的参考章节

## 当前开发方向

该 fork 目前重点关注：

- 基础设施感知审查，尤其是 Terraform
- 混合语言单体仓库
- 从仓库根目录稳定的相对路径图谱注册
- 面向终端和编辑器智能体的 MCP 优先工作流
- 无需托管服务的可复现本地分析

## 安全与隐私

`dagayn` 围绕本地图谱存储进行设计。部分可选的嵌入提供商可能调用远程 API，但这些流程是可选加入的，并单独记录文档。

详情请参见 `SECURITY.md` 和 `docs/LEGAL.md`。

## 贡献

开发设置、验证命令和贡献规则请参见 `CONTRIBUTING.md`。

## 许可证

MIT。请参见 `LICENSE`。
