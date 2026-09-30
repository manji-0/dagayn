"""Claude Code skills and hooks auto-install.

Generates Claude Code agent skill files, hooks configuration, and
CLAUDE.md integration for seamless dagayn usage.
Also supports multi-platform MCP server installation and
Cursor hooks / OpenCode plugin generation.
"""

from __future__ import annotations

from pathlib import Path

from ..atomic_write import write_text_atomic
from .platforms import _embedding_hook_args, logger

# --- OpenCode plugin ---


def _opencode_plugin_content(extra_update_args: list[str] | None = None) -> str:
    """Return TypeScript source for the OpenCode user-level plugin.

    The plugin hooks into four OpenCode events to mirror the Claude Code
    hook behaviors:

    1. ``file.edited`` — enqueues a structure-only update
       (``dagayn queue add update``); a detached worker drains the queue
    2. ``session.created`` — prepares a usable+synced graph, then status
    3. ``tool.execute.before`` — when the tool is a shell command starting
       with ``git commit``, runs ``dagayn update --skip-flows`` followed by
       ``dagayn detect-changes --brief``
    4. ``tool.execute.after`` — when the tool is a HEAD-moving git command
       (``checkout`` / ``switch`` / ``pull`` / …), runs ``session prepare``
       so the graph catches up to the new HEAD

    Every command resolves the repository with ``git rev-parse --show-toplevel``
    and passes ``--repo`` explicitly so worktree sessions update the checkout
    they are running in, not whichever directory OpenCode happened to start in.

    All handlers use try/catch so errors never break the editor session.
    The plugin uses Bun's ``$`` shell API (provided by OpenCode's plugin
    context) for subprocess execution.
    """
    prepare_args = _embedding_hook_args(extra_update_args)
    template = """\
import type { Plugin } from "@opencode-ai/plugin"

/**
 * dagayn plugin for OpenCode.
 *
 * Keeps the knowledge graph up-to-date and surfaces status
 * information automatically during coding sessions.
 *
 * Installed by: dagayn install --platform opencode
 */

// Resolve the git repository root for the active project directory.
async function resolveRepo($: any): Promise<string> {
  let repo = ""
  try {
    const result = await $`git rev-parse --show-toplevel`.quiet()
    repo = result.stdout?.toString().trim() ?? ""
  } catch {
    repo = ""
  }
  // A jj workspace nested in the checkout has no .git of its own.
  try {
    const result = await $`jj workspace root`.quiet()
    const workspace = result.stdout?.toString().trim() ?? ""
    if (workspace && workspace.startsWith(repo + "/")) {
      return workspace
    }
  } catch {
    // jj missing or not a jj workspace
  }
  return repo
}

function shellCommand(ctx: any): string {
  const input = ctx?.input ?? ctx?.params ?? ctx?.args ?? {}
  const cmd =
    input.command ??
    input.cmd ??
    input.content ??
    ctx?.args?.command ??
    ""
  return typeof cmd === "string" ? cmd : ""
}

export default (app: any) => {
  // 1. Auto-update graph after file edits. Enqueue instead of running the
  // update inline: edit bursts coalesce into one structure-only pass drained
  // by a detached worker (dagayn.task_queue).
  app.on("file.edited", async ({ $ }: { $: any }) => {
    try {
      const repo = await resolveRepo($)
      if (repo) {
        await $`dagayn queue add update --repo ${repo}`.quiet()
      } else {
        await $`dagayn queue add update`.quiet()
      }
    } catch {
      // Swallow — graph may not be built yet for this project.
    }
  })

  // 2. Prepare a usable+synced graph when a session starts
  app.on("session.created", async ({ $ }: { $: any }) => {
    try {
      const prepare =
        "DAGAYN_HOOK_UPDATE=1 dagayn session prepare --budget-seconds 45__DAGAYN_PREPARE_ARGS__"
      const repo = await resolveRepo($)
      if (repo) {
        await $`${prepare} --repo ${repo}`.quiet()
        const result = await $`dagayn status --repo ${repo}`.quiet()
        const output = result.stdout?.toString().trim()
        if (output) {
          console.log("[dagayn]", output)
        }
      } else {
        await $`${prepare}`.quiet()
        const result = await $`dagayn status`.quiet()
        const output = result.stdout?.toString().trim()
        if (output) {
          console.log("[dagayn]", output)
        }
      }
    } catch {
      // Swallow — not every project has a graph.
    }
  })

  // 3. Detect changes before git commit commands
  app.on("tool.execute.before", async (ctx: any) => {
    try {
      const cmd = shellCommand(ctx)
      if (/(?:^|[\\/\\\\]|\\s)git(?:\\.exe)?\\s+commit\\b/i.test(cmd)) {
        const repo = await resolveRepo(ctx.$)
        if (repo) {
          await ctx.$`DAGAYN_HOOK_UPDATE=1 dagayn update --skip-flows --repo ${repo}`.quiet()
          const result =
            await ctx.$`dagayn detect-changes --brief --repo ${repo}`.quiet()
          const output = result.stdout?.toString().trim()
          if (output) {
            console.log("[dagayn] Pre-commit analysis:\\n" + output)
          }
        } else {
          await ctx.$`DAGAYN_HOOK_UPDATE=1 dagayn update --skip-flows`.quiet()
          const result =
            await ctx.$`dagayn detect-changes --brief`.quiet()
          const output = result.stdout?.toString().trim()
          if (output) {
            console.log("[dagayn] Pre-commit analysis:\\n" + output)
          }
        }
      }
    } catch {
      // Swallow — never block a commit.
    }
  })

  // 4. Re-prepare after HEAD-moving git commands (post-execution)
  app.on("tool.execute.after", async (ctx: any) => {
    try {
      const cmd = shellCommand(ctx)
      if (
        /(?:^|[\\/\\\\]|\\s)git(?:\\.exe)?\\s+(?:checkout|switch|reset|pull|merge|rebase|cherry-pick)\\b/i.test(
          cmd,
        )
      ) {
        const repo = await resolveRepo(ctx.$)
        const prepare =
          "DAGAYN_HOOK_UPDATE=1 dagayn session prepare --budget-seconds 45__DAGAYN_PREPARE_ARGS__"
        if (repo) {
          await ctx.$`${prepare} --repo ${repo}`.quiet()
        } else {
          await ctx.$`${prepare}`.quiet()
        }
      }
    } catch {
      // Swallow — never block a checkout.
    }
  })
}
"""
    return template.replace("__DAGAYN_PREPARE_ARGS__", prepare_args)


def install_opencode_plugin(extra_update_args: list[str] | None = None) -> Path:
    """Install the OpenCode user-level plugin for dagayn.

    Writes ``~/.config/opencode/plugins/crg-plugin.ts``.  Creates the
    directories if they don't exist.  If the file already exists it is
    overwritten (the plugin is self-contained and idempotent).

    Returns:
        Path to the plugin file that was written.
    """
    plugins_dir = Path.home() / ".config" / "opencode" / "plugins"
    plugin_path = plugins_dir / "crg-plugin.ts"

    plugins_dir.mkdir(parents=True, exist_ok=True)
    write_text_atomic(plugin_path, _opencode_plugin_content(extra_update_args), encoding="utf-8")
    logger.info("Wrote OpenCode plugin: %s", plugin_path)

    return plugin_path
