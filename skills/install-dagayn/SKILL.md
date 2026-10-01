---
name: install-dagayn
description: Install, upgrade, or repair dagayn's integration with AI coding tools — MCP server config, skills, hooks, instruction files, and the embedding mode — for Claude Code, Codex, Cursor, and the other supported platforms. Consult this skill before inspecting config files whenever dagayn tools or skills are missing from a tool, the graph never updates on edits, the user upgraded dagayn, wants to switch embedding mode, or wants dagayn set up or removed.
argument-hint: "[platform]"
---

# Install Dagayn

`dagayn install` writes MCP config, skills, hooks, and instruction blocks for
the platforms it detects. Re-running it after an upgrade is the supported way
to refresh all of them; the steps below keep the change as narrow as the user
wants and verify it landed.

## Workflow

1. **Check the version**: `dagayn --version`. (`dagayn tool --list` lists every
   CLI-callable tool; the MCP server exposes only the 9 workflow tools unless
   started with `--tools all`.)
2. **Pick the narrowest target**. `--mode` is required with `-y` or without a
   TTY:
   - All detected tools: `dagayn install --platform all --mode local-embedding -y`
   - One platform: `--platform claude` (or `codex`, `cursor`, `windsurf`, `zed`,
     `continue`, `opencode`, `antigravity`, `qwen`, `kiro`, `qoder`, `pi`,
     `hermes`)
   - Managed Qwen sidecar: `--mode local-embedding-llama --preset low`
   - Remote embeddings: `--mode remote-embedding --provider openai|google|minimax`
   - No embeddings: `--mode fts-only`
   - Leave parts out: `--no-skills`, `--no-hooks`, `--no-instructions`
3. **Preview first** when instruction files or repo-local files matter:
   `dagayn install --platform <p> --mode <m> --dry-run`.
4. **Verify what landed**:
   - Claude Code: MCP in the repo's `.mcp.json`; skills only in
     `~/.claude/skills/<name>/SKILL.md` (Claude Code also loads
     `<repo>/.claude/skills`, so install removes dagayn copies there to avoid
     listing every skill twice; `/skill-doctor` should show each once); hooks
     in `~/.claude/settings.json`
     (PostToolUse `Edit|Write` queues `dagayn queue add update`, SessionStart
     runs `dagayn session prepare`); a git pre-commit hook.
   - Codex: `~/.codex/config.toml` has the `dagayn serve` server and
     `hooks = true`; `~/.codex/hooks.json` queues `dagayn queue add update`
     after edits and runs `dagayn session prepare` at session start; skills in
     `~/.codex/skills/<name>/SKILL.md`.
   - Embedding mode: the serve args carry `--local-embedding` (or
     `--remote-embedding <provider>`) for the chosen mode.
   - The server command may be `dagayn serve`, `uvx dagayn serve`,
     `uv run dagayn serve`, or `python -m dagayn serve`, depending on how
     dagayn was installed.
5. **Check a repository**: `dagayn status` or `get_minimal_context_tool`; if the
   graph is empty, `ensure_graph_tool()` (or `dagayn build`; add `--scip` to let
   installed SCIP indexers settle call targets).
6. **Tell the user to restart** their AI tool: running `dagayn serve` processes
   keep the old version until restarted.

## Worktree bootstrap

Install leaves the main checkout ready for linked worktrees: `.worktreeinclude`
lists gitignored MCP config so Claude Code copies it into new worktrees (commit
it), and `.cursor/worktrees.json` runs `dagayn session prepare` in Cursor's
parallel-agent worktrees. Inside a worktree, `dagayn worktree info` then
`dagayn worktree sync`; prefer sync over a rebuild when the main checkout's
graph is healthy (see the `worktree-sync` skill).

## Repo-local files

Every install ensures `.gitignore` ignores `.dagayn/` and may write
`.worktreeinclude`; `--platform all` or `claude` also writes repo-local MCP
config. For a global-only Codex setup use
`dagayn install --platform codex --mode local-embedding --no-instructions -y`,
then check `git status --short`. When
cleaning up, remove only files that are clearly dagayn-generated.

## Safety

- Don't overwrite user-authored instruction files blindly: dry-run and read the
  target list first. Installs replace their own marked blocks; if a file has
  dagayn headings without markers, repair the markers instead of appending.
- Installs replace dagayn's own skills (and remove retired ones) but leave
  other skills alone.
- After changing install behavior in the dagayn repo, run
  `uv run pytest tests/test_skills.py -q`.
