# Security

`dagayn` stores graph data locally and is designed to work without a hosted control plane, but it still processes repository contents and can integrate with external tools. Treat it like developer infrastructure.

## Reporting a vulnerability

If you find a vulnerability, report it privately to the maintainers before opening a public issue. Include:

- affected version or commit
- reproduction steps
- impact assessment
- any proposed mitigation

## Security posture

Current CI checks include:

- `ruff` linting and format validation
- `ty` type checking
- Bandit scanning
- full pytest coverage checks used by the repository workflow

## Local data

By default, graph data is written under `.dagayn/` inside the repository root unless configured otherwise.

That data can include:

- file paths
- symbol names
- graph structure
- metadata used for search, communities, flows, and embeddings

Treat generated graph databases as repository-derived artifacts.

## External providers

Optional embedding features may call external APIs. Those flows are opt-in. Review your environment variables and provider settings before enabling them in sensitive repositories.

## Safe operations

Before sharing a graph artifact or debug dump, confirm it does not expose proprietary code structure, internal filenames, or secrets embedded in source.
