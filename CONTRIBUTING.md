# Contributing to dagayn

Thanks for contributing.

`dagayn` is a fork of `code-review-graph`, but contributions should be written and reviewed against the fork's own goals: infrastructure-aware graph analysis, mixed-language repositories, and MCP-driven AI workflows.

## Development setup

```bash
uv sync
```

If you are not using `uv`, install the package in editable mode with development extras.

## Core verification commands

```bash
uv run ruff check .
uv run ruff format --check .
ty check code_review_graph --python-version 3.10 --ignore unresolved-import --exclude '**/*\ 2.py' --exclude '**/*\ 3.py'
uv run pytest --tb=short -q
```

Use narrower test targets while iterating, then run the full suite before merging.

## Documentation expectations

Documentation in this fork should describe `dagayn`, not defer to upstream naming or assume upstream features. If you change commands, integrations, supported formats, or output behavior, update the relevant docs in the same change.

## Code change expectations

- prefer precise, behavior-preserving edits
- add or update tests when behavior changes
- keep failure modes explicit
- avoid undocumented compatibility breaks
- preserve repo-root-relative graph expectations where applicable

## Pull requests

A good pull request should include:

- a clear problem statement
- the behavior change or fix
- tests or verification notes
- docs updates for user-visible changes

## Large changes

For broad refactors or feature additions, open an issue or draft PR early so maintainers can align on scope before the implementation spreads across parser, graph, tools, and docs.

## Security issues

Do not file sensitive vulnerabilities as public issues first. Follow `SECURITY.md`.
