"""Claude Code skills and hooks auto-install.

Generates Claude Code agent skill files, hooks configuration, and
CLAUDE.md integration for seamless dagayn usage.
Also supports multi-platform MCP server installation and
Cursor hooks / OpenCode plugin generation.
"""

from __future__ import annotations

from pathlib import Path

from ..atomic_write import write_text_atomic
from .platforms import logger, normalize_platform_target
from .trust import TRUST_TIERS_BLOCK

_CLAUDE_MD_SECTION_MARKER = "<!-- dagayn MCP tools -->"
_MARKDOWN_POLICY_MARKER = "<!-- dagayn markdown policy -->"
_CLAUDE_MD_SECTION_HEADING = "## MCP Tools: dagayn"
_MARKDOWN_POLICY_HEADING = (
    "## Markdown documentation policy: declare dependencies via directive comments"
)


def _instruction_section_aliases(marker: str) -> tuple[str, ...]:
    if marker == _CLAUDE_MD_SECTION_MARKER:
        return (_CLAUDE_MD_SECTION_HEADING,)
    if marker == _MARKDOWN_POLICY_MARKER:
        return (
            _MARKDOWN_POLICY_HEADING,
            "## Markdown documentation policy",
            "### Markdown documentation policy",
        )
    return ()


def _refresh_instruction_section(content: str, marker: str, section: str) -> str:
    """Replace the managed section that starts at *marker* with *section*.

    The managed section runs to the next dagayn marker, the next level-2
    heading after its own heading, or the end of the file. Blank lines that
    separated it from the following content are kept.
    """
    start = content.index(marker)
    lines = content[start:].splitlines(keepends=True)
    offset = start + len(lines[0])
    end = len(content)
    heading_seen = False
    for line in lines[1:]:
        stripped = line.strip()
        if stripped in (_CLAUDE_MD_SECTION_MARKER, _MARKDOWN_POLICY_MARKER):
            end = offset
            break
        if line.startswith("## "):
            if heading_seen:
                end = offset
                break
            heading_seen = True
        offset += len(line)
    old = content[start:end]
    trailing = old[len(old.rstrip("\n")) :] or "\n"
    return content[:start] + section.rstrip("\n") + trailing + content[end:]


def _has_instruction_section(content: str, marker: str) -> bool:
    """Return True when content already has a dagayn section, marker or not."""
    return marker in content or any(
        alias in content for alias in _instruction_section_aliases(marker)
    )


_MARKDOWN_POLICY_SECTION = f"""{_MARKDOWN_POLICY_MARKER}
## Markdown documentation policy: declare dependencies via directive comments

In Markdown that dagayn indexes, declare a real dependency on another section
or document with an HTML comment directly under the dependent heading, so the
graph records it (`DEPENDS_ON` / `IMPORTS_FROM`) and impact analysis sees it:

```markdown
<!-- <kind> <target> -->
```

- `<kind>`: `constrained-by` (bounded by the target), `blocked-by` (cannot
  proceed until it resolves), `supersedes` (replaces it; place it in the new
  document), or `derived-from` (built from it).
- `<target>`: `#section-slug` (same document), `./path.md`, or
  `./path.md#slug`. Slugs follow GitHub rules: lowercase, punctuation
  removed, spaces collapsed to `-`. External URLs stay ordinary links.
- No real dependency, no directive. The `writing-markdown-document` skill
  has the full rules.
"""

_CLAUDE_MD_SECTION = f"""{_CLAUDE_MD_SECTION_MARKER}
## MCP Tools: dagayn

**This project has a dagayn knowledge graph. Use the dagayn MCP tools before
grep, glob, or whole-file reads to explore the codebase.** The graph is
cheaper and gives structural context (callers, dependents, tests, linked
docs) that file scanning cannot. Fall back to file search only when a graph
result is missing, stale, ambiguous, or truncated, or `source_of` cannot
supply the span.

### Workflow

1. Start with `get_minimal_context_tool(task=...)`: it reports `sync.state`
   and the next tool to call.
2. Review: `review_tool(mode="changes")`, and read `analysis_summary` before
   any drill-down.
3. Explore: `semantic_search_nodes_tool` finds a node (code or a Markdown
   section); `query_graph_tool` traces it (callers_of, callees_of,
   importers_of, tests_for, docs_for, source_of). Pass `depth` to
   callers_of/importers_of for a transitive chain, and stop when
   `next_action` says the set is closed.
4. Architecture: `architecture_analysis_tool(mode="overview",
   detail_level="minimal")`; read `architecture_health` and drill down only
   into a risk it names (`architecture-analysis` skill).
5. Refactor: `refactor_tool(mode="suggest")`, then preview renames with
   `refactor_tool(mode="rename")`; apply with `apply_refactor_tool` in the
   same `dagayn serve` session.

### Default tools

| Tool | Use when |
| ------ | ---------- |
| `get_minimal_context_tool` | Start here: freshness, risk, next tools |
| `ensure_graph_tool` | Graph empty or behind HEAD; bootstrap without embeddings |
| `review_tool` | Change review: risk, tests, flows, blast radius |
| `query_graph_tool` | Callers, callees, imports, tests, linked docs, live source spans |
| `semantic_search_nodes_tool` | Find code or doc sections by name, keyword, or meaning |
| `flow_tool` | Reachable sets from entry points (not call sequences) |
| `architecture_analysis_tool` | Architecture health and its drill-downs |
| `refactor_tool` | Refactor suggestions, dead code, rename previews |
| `get_docs_section_tool` | dagayn reference sections, e.g. `trust` |

Drill-down tools: `review_tool(mode="impact" | "affected_flows" |
"context")` and `architecture_analysis_tool(mode=...)`. `dagayn serve --tools
all` (or `CRG_TOOLS`) exposes the advanced and maintenance tools.

### How to judge analysis output

{TRUST_TIERS_BLOCK}

- Cite the counts, thresholds, reason codes, and `truncated`/`total` fields
  behind a recommendation; narrow a truncated result with `top_n`,
  `detail_level`, or a targeted query before concluding.
- Check `query_graph_tool(pattern="tests_for")` before calling code
  untested; a file-level zero is Low trust, not proof.
- Before a refactor, check public APIs, dynamic dispatch, generated code,
  and framework entry points.
"""


def _inject_instructions(
    file_path: Path,
    marker: str,
    section: str,
    *,
    errors: list[str] | None = None,
) -> bool:
    """Append an instruction section to a file, or refresh a stale one.

    Idempotent: a marked section that already matches *section* is left alone;
    a marked section with older text is replaced in place. Creates the file if
    it doesn't exist.

    Returns True if the file was modified.
    """
    existing = ""
    try:
        if file_path.exists():
            existing = file_path.read_text(encoding="utf-8", errors="replace")

        if marker in existing:
            refreshed = _refresh_instruction_section(existing, marker, section)
            if refreshed == existing:
                logger.info("%s already contains instructions, skipping.", file_path.name)
                return False
            write_text_atomic(file_path, refreshed, encoding="utf-8")
            logger.info("Refreshed dagayn instructions in %s", file_path)
            return True

        for marker_heading in _instruction_section_aliases(marker):
            if marker_heading in existing:
                updated = existing.replace(marker_heading, f"{marker}\n{marker_heading}", 1)
                write_text_atomic(file_path, updated, encoding="utf-8")
                logger.info("Added missing dagayn marker to %s", file_path)
                return True

        separator = "\n" if existing and not existing.endswith("\n") else ""
        extra_newline = "\n" if existing else ""
        file_path.parent.mkdir(parents=True, exist_ok=True)
        write_text_atomic(
            file_path, existing + separator + extra_newline + section, encoding="utf-8"
        )
    except OSError as exc:
        message = f"{file_path} ({exc})"
        if errors is not None:
            errors.append(message)
        logger.debug("Skipped instruction injection for %s: %s", file_path, exc)
        return False
    logger.info("Appended MCP tools section to %s", file_path)
    return True


def inject_claude_md(
    repo_root: Path | None = None,
    *,
    errors: list[str] | None = None,
) -> list[str]:
    """Append MCP tools section and Markdown policy to ``~/.claude/CLAUDE.md``."""
    claude_md = Path.home() / ".claude" / "CLAUDE.md"
    updated = False
    if _inject_instructions(
        claude_md,
        _CLAUDE_MD_SECTION_MARKER,
        _CLAUDE_MD_SECTION,
        errors=errors,
    ):
        updated = True
    if _inject_instructions(
        claude_md,
        _MARKDOWN_POLICY_MARKER,
        _MARKDOWN_POLICY_SECTION,
        errors=errors,
    ):
        updated = True
    return ["~/.claude/CLAUDE.md"] if updated else []


# Cross-platform instruction files and which platforms own each one.
# Used to filter writes when the user passes --platform <X>: only files
# whose owner set includes the target (or "all") are written.
_PLATFORM_INSTRUCTION_FILES: dict[str, tuple[str, ...]] = {
    "AGENTS.md": ("codex", "cursor", "opencode", "antigravity"),
    "GEMINI.md": ("antigravity",),
    ".cursorrules": ("cursor",),
    ".windsurfrules": ("windsurf",),
    "QODER.md": ("qoder",),
    ".kiro/steering/dagayn.md": ("kiro",),
}


def _platform_instruction_paths(repo_root: Path, filename: str, target: str) -> list[Path]:
    """Return the destination path(s) for a platform instruction file."""
    target = normalize_platform_target(target)

    if filename != "AGENTS.md":
        return [repo_root / filename]

    if target == "codex":
        return [Path.home() / ".codex" / "AGENTS.md"]
    if target == "opencode":
        return [Path.home() / ".config" / "opencode" / "AGENTS.md"]
    if target == "all":
        return [
            repo_root / "AGENTS.md",
            Path.home() / ".codex" / "AGENTS.md",
            Path.home() / ".config" / "opencode" / "AGENTS.md",
        ]
    return [repo_root / "AGENTS.md"]


def inject_platform_instructions(
    repo_root: Path,
    target: str = "all",
    *,
    errors: list[str] | None = None,
) -> list[str]:
    """Inject 'use graph first' instructions into platform rule files.

    Writes AGENTS.md, GEMINI.md, .cursorrules, and/or .windsurfrules
    depending on ``target``:

    - ``"all"`` (default): writes every file — matches pre-filter behavior.
    - ``"claude"``: writes nothing (``~/.claude/CLAUDE.md`` is handled by ``inject_claude_md``).
    - any other platform key (``cursor``, ``windsurf``, ``antigravity``,
      ``opencode``): writes only the files associated with that platform.

    Returns list of filenames that were created or updated.
    """
    target = normalize_platform_target(target)
    updated: list[str] = []
    for filename, owners in _PLATFORM_INSTRUCTION_FILES.items():
        if target != "all" and target not in owners:
            continue
        changed = False
        for path in _platform_instruction_paths(repo_root, filename, target):
            if _inject_instructions(
                path,
                _CLAUDE_MD_SECTION_MARKER,
                _CLAUDE_MD_SECTION,
                errors=errors,
            ):
                changed = True
            if _inject_instructions(
                path,
                _MARKDOWN_POLICY_MARKER,
                _MARKDOWN_POLICY_SECTION,
                errors=errors,
            ):
                changed = True
        if changed:
            updated.append(filename)
    return updated


# --- Worktree file inheritance (.worktreeinclude) ---


_WORKTREEINCLUDE_START = "# >>> dagayn worktree include"
_WORKTREEINCLUDE_END = "# <<< dagayn worktree include"


def worktree_include_patterns(repo_root: Path, platform_keys: list[str] | None = None) -> list[str]:
    """Return ``.worktreeinclude`` patterns for dagayn config in *repo_root*.

    Only patterns whose target exists **and** is gitignored are returned:
    tracked files are already checked out into every worktree, and
    ``.worktreeinclude`` copies gitignored matches only.
    """
    from ..worktree import PLATFORM_CONFIG_PATTERNS, config_pattern_target, is_gitignored

    keys = list(PLATFORM_CONFIG_PATTERNS) if platform_keys is None else platform_keys
    patterns: list[str] = []
    for key in keys:
        for pattern in PLATFORM_CONFIG_PATTERNS.get(normalize_platform_target(key), ()):
            target = config_pattern_target(pattern)
            path = repo_root / target
            if not path.exists():
                continue
            if not is_gitignored(repo_root, target):
                continue
            if pattern not in patterns:
                patterns.append(pattern)
    return patterns


def ensure_worktree_include(
    repo_root: Path,
    patterns: list[str],
    dry_run: bool = False,
) -> str:
    """Maintain a dagayn-managed block in ``<repo_root>/.worktreeinclude``.

    Claude Code copies gitignored files matching this file into every worktree
    it creates (``--worktree``, ``EnterWorktree``, subagent and desktop
    worktrees), which is how MCP config survives into a worktree session.

    Returns ``"created"``, ``"updated"``, ``"unchanged"``, or ``"skipped"``
    (nothing to add).
    """
    if not patterns:
        return "skipped"

    block_lines = [
        _WORKTREEINCLUDE_START,
        "# Copied into new git worktrees so agent sessions there keep the dagayn",
        "# MCP server and skills. Managed by 'dagayn install'.",
        *patterns,
        _WORKTREEINCLUDE_END,
    ]
    block = "\n".join(block_lines) + "\n"

    path = repo_root / ".worktreeinclude"
    existing = ""
    if path.exists():
        try:
            existing = path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            logger.warning("Could not read %s: %s", path, exc)
            return "skipped"

    if _WORKTREEINCLUDE_START in existing:
        start = existing.index(_WORKTREEINCLUDE_START)
        end_marker = existing.find(_WORKTREEINCLUDE_END, start)
        if end_marker == -1:
            # No end marker (hand-edited or a partially-written file). Replacing
            # everything from the start marker onward silently discarded the
            # user's own patterns below it; keep the remainder instead.
            remainder = existing[start + len(_WORKTREEINCLUDE_START) :]
            updated = existing[:start] + block.rstrip("\n") + "\n" + remainder.lstrip("\n")
        else:
            end = end_marker + len(_WORKTREEINCLUDE_END)
            updated = existing[:start] + block.rstrip("\n") + existing[end:]
        if updated == existing:
            return "unchanged"
        if not dry_run:
            write_text_atomic(path, updated, encoding="utf-8")
        return "updated"

    if not existing:
        if not dry_run:
            write_text_atomic(path, block, encoding="utf-8")
        return "created"

    prefix = existing if existing.endswith("\n") else existing + "\n"
    if not dry_run:
        write_text_atomic(path, prefix + block, encoding="utf-8")
    return "updated"
