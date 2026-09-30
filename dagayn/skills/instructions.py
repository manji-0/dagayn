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

When authoring or editing a Markdown document in this repository, declare
inter-section and inter-document dependencies as HTML directive comments so
they are captured by the dagayn graph (`DEPENDS_ON` / `IMPORTS_FROM` edges)
and discoverable via `query_graph_tool` / `review_tool(mode="impact")`.

### Required form

```markdown
<!-- <kind> <target> -->
```

`<kind>` MUST be one of: `constrained-by`, `blocked-by`, `supersedes`,
`derived-from`. Choose the kind whose semantics best match the dependency:

| Kind | Use when |
| ---- | -------- |
| `constrained-by` | This section's design is bounded by the referenced document/section |
| `blocked-by` | This item cannot proceed until the referenced item resolves |
| `supersedes` | This document replaces the referenced content |
| `derived-from` | This section is derived from the referenced source |

### Three target shapes

| Dependency type | Target syntax | Example |
| --------------- | ------------- | ------- |
| Within-document section | `#section-slug` | `<!-- derived-from #background -->` |
| Other document (whole file) | `./relative/path.md` | `<!-- blocked-by ./specs/open-issue.md -->` |
| Other document + section | `./path.md#slug` | `<!-- constrained-by ./adr.md#context -->` |

Slugs follow GitHub Markdown rules: lowercase, non-alphanumerics removed,
spaces and hyphens collapsed to `-`. Place the directive immediately under
the heading whose content depends on the target. External URLs
(`http://`, `https://`) are not graph-resolvable — keep them as ordinary
Markdown links, not directive targets.

### When to add a directive

- Section design references an ADR, spec, or research note → `constrained-by` or `derived-from`.
- A document replaces an older one → `supersedes` (place in the new document).
- A spec/task section is blocked on another being resolved → `blocked-by`.
- A later section extends an earlier one non-obviously → `derived-from #earlier-section`.

If no real dependency exists, do not invent one. Directives are signal, not decoration.
"""

_CLAUDE_MD_SECTION = f"""{_CLAUDE_MD_SECTION_MARKER}
## MCP Tools: dagayn

**IMPORTANT: This project has a knowledge graph. ALWAYS use the
dagayn MCP tools BEFORE using Grep/Glob/Read to explore
the codebase.** The graph is faster, cheaper (fewer tokens), and gives
you structural context (callers, dependents, test coverage) that file
scanning cannot.

### When to use graph tools FIRST

- **Any new task**: `get_minimal_context_tool` for graph freshness, risk, and next-tool hints
- **Exploring code**: `semantic_search_nodes_tool` or `query_graph_tool` instead of Grep
- **Understanding impact**: `review_tool(mode="impact")` instead of manually tracing imports
- **Code review**: `review_tool(mode="changes")` first; use its `analysis_summary` before
  calling drill-down tools
- **Finding relationships**: `query_graph_tool` with
  callers_of/callees_of/imports_of/tests_for/source_of; pass `depth` to
  callers_of/importers_of for a transitive chain in one call, and stop when
  `next_action` says the set is closed. The default `detail_level` lists every
  related node; `full` only adds per-edge rows
- **Architecture questions**: `architecture_analysis_tool(mode="overview")`
  first; use `architecture_health` and the Architecture Analysis skill before
  choosing a drill-down mode

Fall back to Grep/Glob/Read **only** when the graph result is missing, stale,
ambiguous, truncated, or `source_of` cannot supply the span. Do not re-read a
whole file just to inspect a function the graph already located.

### Tool surface

`dagayn serve` exposes the compact workflow tool surface by default. Use
`dagayn serve --tools ...` when a deployment needs an exact allow-list; the same
allow-list can be supplied with `CRG_TOOLS`. Use `all`, `full`, or `*` to expose
advanced/maintenance tools.

### Default workflow tools

| Tool | Use when |
| ------ | ---------- |
| `get_minimal_context_tool` | Start here: graph freshness, risk, communities, next tools |
| `ensure_graph_tool` | Empty or missing graph; safe bootstrap without embeddings |
| `review_tool` | Primary change review and review drill-down dispatcher |
| `flow_tool` | Reachable-set flow lists and BFS membership (not call sequences) |
| `architecture_analysis_tool` | Primary architecture review and drill-down dispatcher |
| `refactor_tool` | Planning renames, finding dead code, and evidence-ranked refactor suggestions |
| `query_graph_tool` | Tracing callers, callees, imports, tests, live source spans |
| `semantic_search_nodes_tool` | Finding functions/classes by name or keyword |

### Drill-down tools

| Tool | Use when |
| ------ | ---------- |
| `review_tool(mode="impact")` | Need a wider or deeper blast-radius view |
| `review_tool(mode="affected_flows")` | Need full affected execution-path details |
| `architecture_analysis_tool(mode=...)` | Architecture drill-downs for boundaries and metrics |

### How to judge analysis output

- Treat graph insights as **evidence-ranked leads**, not automatic truth.
- Prefer outputs that expose metrics, thresholds, counts, reason codes, and
  `truncated`/`total` fields; mention those numbers when making recommendations.
- Check test coverage with `query_graph_tool` pattern=\"tests_for\" before claiming a
  code path is untested.
- For refactors, verify public APIs, dynamic dispatch, generated code, test
  artifacts, and framework entry points before editing.
- If an output is truncated or approximate, narrow with `top_n`, `detail_level`,
  `max_depth`, or a targeted follow-up query before drawing conclusions.

### Workflow

1. Start with `get_minimal_context_tool(task=...)`.
2. Use the suggested next tool or a targeted query.
3. For reviews, use `review_tool(mode=\"changes\")` and read `analysis_summary`
   first. Call `review_tool(mode=\"context\")`, `review_tool(mode=\"affected_flows\")`,
   `review_tool(mode=\"impact\")`, or `query_graph_tool` only when the summary points there.
4. For architecture work, use
   `architecture_analysis_tool(mode=\"overview\", detail_level=\"minimal\")`
   and read `architecture_health` first. Use the Architecture Analysis skill to
   choose drill-down modes only when the health summary identifies a concrete risk.
5. For refactors, use `refactor_tool(mode=\"suggest\")` first, then preview
   renames with `refactor_tool(mode=\"rename\")`. Apply with
   `apply_refactor_tool` in the same `dagayn serve` MCP session
   (refactor_id is session-scoped; advanced MCP surface: `dagayn serve --tools all`).
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
