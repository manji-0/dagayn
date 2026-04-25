# dagayn

`dagayn` 是 `code-review-graph` 的一个 fork，作为独立维护的项目，文档以 `dagayn` 为准。

## 主要能力

- 把仓库构建成本地知识图谱
- 做改动影响范围分析
- 通过 MCP 给 AI 工具提供更小、更精确的上下文
- 处理同时包含应用代码、Markdown 和 Terraform 的仓库

## 快速开始

```bash
pip install dagayn
dagayn install
dagayn build
dagayn status
```

## fork 的重点

- Terraform 一等支持
- Markdown 结构和指令依赖提取
- 以仓库根目录相对路径为基础的图谱注册
- 使用 `ruff` 与 `ty` 的 CI

更多说明请阅读 `README.md` 和 `docs/`。
