---
name: install-dagayn
description: Install or repair dagayn MCP integration, skills, hooks, and instruction files across Codex and other AI coding tools.
argument-hint: "[platform]"
---

# Install Dagayn

Use this when setting up or repairing dagayn itself: MCP config, global skills,
hooks, instruction injection, or embedding mode selection.

## Workflow

1. Check the installed version and current tool surface:
   ```bash
   dagayn --version
   dagayn tool --list
   ```
2. Prefer the smallest platform target that matches the request:
   - Codex global only: `dagayn install --platform codex --mode local --preset low --no-instructions -y`
   - All detected tools: `dagayn install --platform all --mode local --preset low -y`
   - No embeddings: `dagayn install --platform <platform> --mode fts -y`
3. Use `--dry-run` before changing instruction files or when the user cares
   about repo-local files:
   ```bash
   dagayn install --platform codex --mode local --preset low --no-instructions --dry-run
   ```
4. Verify the result:
   - Codex MCP config: `~/.codex/config.toml` should contain `dagayn serve`.
   - Local embeddings: the serve args should include `--local-embedding low`.
   - Codex hooks: `~/.codex/hooks.json` should run `dagayn update --skip-flows`.
   - Skills: `~/.codex/skills/<skill>/SKILL.md` should exist for packaged skills.
   - Markdown/code traceability guidance: installed writing/review/explore
     skills should mention `docs_for`, `implementations_of`, and `dagayn:`
     documentation directives.
5. Run `dagayn status` or `list_graph_stats_tool` in a target repo. If the graph
   is missing, run `build_or_update_graph_tool(full_rebuild=True)`.

## Repo-Local Files

`--platform all` may create repo-local config for tools such as Claude Code,
Kiro, Qoder, OpenCode, Cursor, or Antigravity. If the user wants global-only
Codex behavior, use `--platform codex --no-instructions` and do not create
repo-local `.mcp.json`, `.opencode.json`, `.kiro/`, `.qoder/`, or `AGENTS.md`.

If a previous install created unwanted repo-local files, remove only files that
are clearly dagayn-generated and verify with `git status --short`.

## Safety Rules

- Do not overwrite user-authored instruction files blindly. Use `--dry-run`
  and inspect the exact target list first.
- If a file already has dagayn headings but lacks markers, prefer marker repair
  over appending a duplicate block.
- After changing install behavior, run `uv run pytest tests/test_skills.py -q`.

## Efficiency Rules

- For ordinary Codex setup, target 4 shell checks: version, dry-run, install,
  verify config.
- Avoid reading broad home directories. Inspect only the platform config paths
  named by the installer output.
