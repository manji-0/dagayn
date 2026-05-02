"""install / init command — argument registration and handler."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from ._shared import _PLATFORM_CHOICES, _confirm_yes_no


def register_commands(sub: argparse._SubParsersAction) -> dict:
    """Register install and init subcommands. Returns {cmd_name: subparser} dict."""

    def _add_common(p: argparse.ArgumentParser) -> None:
        p.add_argument("--repo", default=None, help="Repository root (auto-detected)")
        p.add_argument(
            "--dry-run",
            action="store_true",
            help="Show what would be done without writing files",
        )
        p.add_argument(
            "--no-skills",
            action="store_true",
            help="Skip generating Claude Code skill files",
        )
        p.add_argument(
            "--no-hooks",
            action="store_true",
            help="Skip installing Claude Code hooks",
        )
        p.add_argument(
            "--no-instructions",
            action="store_true",
            help="Skip injecting graph instructions into ~/.claude/CLAUDE.md / AGENTS.md / etc.",
        )
        p.add_argument(
            "-y",
            "--yes",
            action="store_true",
            help="Auto-confirm instruction injection without an interactive prompt",
        )
        # Legacy flags (kept for backwards compat, now no-ops since all is default)
        p.add_argument("--skills", action="store_true", help=argparse.SUPPRESS)
        p.add_argument("--hooks", action="store_true", help=argparse.SUPPRESS)
        p.add_argument("--all", action="store_true", dest="install_all", help=argparse.SUPPRESS)
        p.add_argument(
            "--platform",
            choices=_PLATFORM_CHOICES,
            default="all",
            help="Target platform for MCP config (default: all detected)",
        )

    install_cmd = sub.add_parser("install", help="Register MCP server with AI coding platforms")
    _add_common(install_cmd)

    init_cmd = sub.add_parser("init", help="Alias for install")
    _add_common(init_cmd)

    return {"install": install_cmd, "init": init_cmd}


def _instruction_files_to_modify(
    repo_root: Path,
    target: str,
) -> list[str]:
    """Return the list of instruction files that ``install`` would write
    or modify, given the current state of the repo and the selected
    platform target. Used for the dry-run / confirm preview (#173).
    """
    from ...skills import (
        _CLAUDE_MD_SECTION_MARKER,
        _MARKDOWN_POLICY_MARKER,
        _PLATFORM_INSTRUCTION_FILES,
        _platform_instruction_paths,
        normalize_platform_target,
    )

    def _needs_update(content: str) -> bool:
        return _CLAUDE_MD_SECTION_MARKER not in content or _MARKDOWN_POLICY_MARKER not in content

    target = normalize_platform_target(target)
    targets: list[str] = []

    if target in ("claude", "all"):
        claude_md = Path.home() / ".claude" / "CLAUDE.md"
        if claude_md.exists():
            content = claude_md.read_text(encoding="utf-8")
            if _needs_update(content):
                targets.append("~/.claude/CLAUDE.md (append)")
        else:
            targets.append("~/.claude/CLAUDE.md (new)")

    for filename, owners in _PLATFORM_INSTRUCTION_FILES.items():
        if target != "all" and target not in owners:
            continue
        for path in _platform_instruction_paths(repo_root, filename, target):
            if path == Path.home() / ".codex" / "AGENTS.md":
                display_name = "~/.codex/AGENTS.md"
            elif path == Path.home() / ".config" / "opencode" / "AGENTS.md":
                display_name = "~/.config/opencode/AGENTS.md"
            else:
                display_name = filename
            if path.exists():
                content = path.read_text(encoding="utf-8")
                if _needs_update(content):
                    targets.append(f"{display_name} (append)")
            else:
                targets.append(f"{display_name} (new)")

    return targets


def handle(args: argparse.Namespace) -> None:
    """Set up MCP config for detected AI coding platforms."""
    from ...incremental import ensure_repo_gitignore_excludes_crg, find_repo_root
    from ...skills import install_platform_configs, normalize_platform_target

    repo_root = Path(args.repo) if args.repo else find_repo_root()
    if not repo_root:
        repo_root = Path.cwd()

    dry_run = getattr(args, "dry_run", False)
    target = normalize_platform_target(getattr(args, "platform", "all") or "all")
    auto_yes = getattr(args, "yes", False)
    skip_instructions = getattr(args, "no_instructions", False)

    print("Installing MCP server config...")
    configured = install_platform_configs(repo_root, target=target, dry_run=dry_run)

    if not configured:
        print("No platforms detected.")
    else:
        print(f"\nConfigured {len(configured)} platform(s): {', '.join(configured)}")

    # Preview the instruction files that would be touched (#173).
    instr_targets = _instruction_files_to_modify(repo_root, target)
    if instr_targets:
        print()
        print("Graph instructions will be injected into:")
        for t in instr_targets:
            print(f"  {t}")

    if dry_run:
        print("\n[dry-run] Would ensure .gitignore ignores .dagayn/.")
        print("[dry-run] No files were modified.")
        return

    gitignore_state = ensure_repo_gitignore_excludes_crg(repo_root)
    if gitignore_state == "created":
        print("Created .gitignore and added .dagayn/.")
    elif gitignore_state == "updated":
        print("Updated .gitignore with .dagayn/.")
    else:
        print(".gitignore already contains .dagayn/.")

    # Skills and hooks are installed by default so Claude actually uses the
    # graph tools proactively.  Use --no-skills / --no-hooks / --no-instructions
    # to opt out.
    skip_skills = getattr(args, "no_skills", False)
    skip_hooks = getattr(args, "no_hooks", False)
    # Legacy: --skills/--hooks/--all still accepted (no-op, everything is default)

    from ...skills import (
        PLATFORMS,
        generate_skills,
        inject_claude_md,
        inject_platform_instructions,
        install_cursor_hooks,
        install_git_hook,
        install_global_skills,
        install_hooks,
        install_opencode_plugin,
        install_qoder_skills,
    )

    if not skip_skills:
        if target in ("all", "claude"):
            skills_dir = generate_skills(repo_root)
            print(f"Generated skills in {skills_dir}")
            try:
                global_skills_dir = install_global_skills()
                print(f"Installed global skills to {global_skills_dir}")
            except OSError as e:
                print(f"Skipped global skills install ({e})", file=sys.stderr)

    # Confirm before writing instruction files (#173). --yes skips the
    # prompt; --no-instructions skips the whole block.
    if not skip_instructions and instr_targets:
        if auto_yes or _confirm_yes_no(
            "Inject graph instructions into the files above?",
            default_yes=True,
        ):
            if target in ("claude", "all"):
                inject_claude_md(repo_root)
            inject_platform_instructions(repo_root, target=target)
            # Use the precomputed instr_targets list for the confirmation
            # message; we don't need the fresh return value from
            # inject_platform_instructions here.
            names = [t.split(" ")[0] for t in instr_targets]
            print(f"Injected graph instructions into: {', '.join(names)}")
        else:
            print("Skipped instruction injection (user declined).")
    elif skip_instructions:
        print("Skipped instruction injection (--no-instructions).")

    # Install Qoder skills (global user-level skills directory)
    if not skip_skills and target in ("qoder", "all"):
        qoder_skills_dir = install_qoder_skills(repo_root)
        if qoder_skills_dir:
            print(f"Installed Qoder skills to {qoder_skills_dir}")
    if not skip_hooks and target in ("claude", "qoder", "all"):
        platforms_to_install = [target] if target != "all" else ["claude", "qoder"]
        for plat in platforms_to_install:
            install_hooks(repo_root, platform=plat)
            print(f"Installed hooks in {repo_root / f'.{plat}' / 'settings.json'}")
        git_hook = install_git_hook(repo_root)
        if git_hook:
            print(f"Installed git pre-commit hook in {git_hook}")

        # Cursor hooks (user-level, only if ~/.cursor exists — matching MCP detect)
        if target in ("all", "cursor") and PLATFORMS["cursor"]["detect"]():
            try:
                hooks_path = install_cursor_hooks()
                print(f"Installed Cursor hooks in {hooks_path}")
            except Exception as exc:
                import logging

                logging.getLogger(__name__).warning("Could not install Cursor hooks: %s", exc)

    # OpenCode plugin (user-level, gated by same detect() as MCP config)
    if not skip_hooks and target in ("all", "opencode") and PLATFORMS["opencode"]["detect"]():
        try:
            plugin_path = install_opencode_plugin()
            print(f"Installed OpenCode plugin in {plugin_path}")
        except Exception as exc:
            import logging

            logging.getLogger(__name__).warning("Could not install OpenCode plugin: %s", exc)

    print()
    print("Next steps:")
    print("  1. dagayn build    # build the knowledge graph")
    print("  2. Restart your AI coding tool to pick up the new config")
