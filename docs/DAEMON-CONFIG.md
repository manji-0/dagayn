# Daemon and registry configuration

<!-- constrained-by ./COMMANDS.md -->
<!-- constrained-by ./RECIPES.md#multi-repo-registry--search -->

## Purpose

<!-- derived-from ./COMMANDS.md#multi-repo-management -->

dagayn supports multi-repo workflows through:

- a JSON registry of known repositories
- a watch daemon configured by TOML

This document describes those local file contracts. For copy-paste register →
search and daemon recipes, see
[RECIPES.md](./RECIPES.md#multi-repo-registry--search).

## Registry file

<!-- derived-from ./COMMANDS.md#multi-repo-management -->

The repository registry lives at:

```text
~/.dagayn/registry.json
```

Stored shape:

```json
{
  "repos": [
    {
      "path": "/absolute/path/to/repo",
      "alias": "app"
    }
  ]
}
```

Rules:

- `path` is stored as an absolute resolved path
- `alias` is optional but should be unique when present
- registration accepts repositories containing either `.git` or `.dagayn`

## Watch daemon config

<!-- derived-from ./COMMANDS.md#multi-repo-management -->

The watch daemon config lives at:

```text
~/.dagayn/watch.toml
```

Example:

```toml
[daemon]
session_name = "dagayn-watch"
log_dir = "/Users/example/.dagayn/logs"
poll_interval = 2

[[repos]]
path = "/Users/example/src/app"
alias = "app"

[[repos]]
path = "/Users/example/src/infra"
alias = "infra"
```

### `[daemon]`

- `session_name` — logical daemon name used in status and logging
- `log_dir` — directory for per-repo logs
- `poll_interval` — config polling interval in seconds

### `[[repos]]`

- `path` — absolute or user-expandable repository path
- `alias` — short unique name; defaults to the directory name if omitted

Validation rules:

- nonexistent directories are skipped
- entries without `.git` or `.dagayn` are skipped
- duplicate aliases are skipped

## Runtime state files

<!-- derived-from ./COMMANDS.md#multi-repo-management -->

Additional local state files live under `~/.dagayn/`:

- `daemon.pid`
- `daemon-state.json`

These files are daemon-managed runtime state, not hand-edited configuration.
