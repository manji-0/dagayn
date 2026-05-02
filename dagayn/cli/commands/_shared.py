"""Shared utilities used only within CLI command modules."""

from __future__ import annotations

import sys

_PLATFORM_CHOICES = [
    "codex",
    "claude",
    "claude-code",
    "cursor",
    "windsurf",
    "zed",
    "continue",
    "opencode",
    "antigravity",
    "qwen",
    "kiro",
    "qoder",
    "qcoder",
    "all",
]


def _confirm_yes_no(prompt: str, default_yes: bool = True) -> bool:
    """Prompt the user [Y/n] and return True for yes.

    Non-interactive environments (no TTY on stdin, e.g. an MCP wrapper
    piping the CLI) return ``default_yes`` without blocking — the
    stdio transport cannot safely read from stdin without corrupting
    the JSON-RPC stream. See: #173, #174
    """
    if not sys.stdin.isatty():
        return default_yes
    suffix = "[Y/n]" if default_yes else "[y/N]"
    try:
        answer = input(f"{prompt} {suffix} ").strip().lower()
    except (EOFError, KeyboardInterrupt):
        print()
        return False
    if not answer:
        return default_yes
    return answer in ("y", "yes")
