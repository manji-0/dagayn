# Contributing to dagayn

## Issues

Issues are the primary way to participate in the project. Bug reports, questions, and feature suggestions are all welcome.

When filing an issue, include enough context for maintainers to reproduce or understand the problem. Feature requests are read and considered, but there is no commitment to implement them.

## Pull requests

Pull requests are not accepted at this stage of development. The maintainers manage all changes directly. If the project matures to broader OSS adoption, this will be revisited.

## Security issues

Do not file sensitive vulnerabilities as public issues. Follow `SECURITY.md`.

## Development setup (maintainers)

```bash
uv sync
uv tool install prek
prek install
```

```bash
uv run ruff check .
uv run ruff format --check .
ty check dagayn --python-version 3.13 --ignore unresolved-import
uv run pytest --tb=short -q
```
