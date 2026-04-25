# dagayn for Claude Code

`dagayn` is a fork of `code-review-graph`. This repository keeps compatibility shims where useful, but the expected user-facing name in this fork is `dagayn`.

## Suggested local workflow

```bash
dagayn install
dagayn build
dagayn serve
```

## Verification commands

```bash
uv run ruff check .
uv run ruff format --check .
ty check dagayn --python-version 3.13 --ignore unresolved-import --exclude '**/*\ 2.py' --exclude '**/*\ 3.py'
uv run pytest --tb=short -q
```

## Good prompts to start with

- build the graph for this repository
- show the blast radius of the latest change
- list the affected flows for these files
- summarize the communities around this subsystem
- preview a rename before applying it

## Fork-specific emphasis

Claude Code users should assume Terraform and Markdown are first-class citizens in this fork and should not rely on upstream docs when describing those capabilities.
