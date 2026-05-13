"""Shared utilities used only within CLI command modules."""

from __future__ import annotations

import argparse
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


def _add_local_embedding_args(cmd: argparse.ArgumentParser) -> None:
    """Add --local-embedding* flags to a subcommand parser."""
    cmd.add_argument(
        "--local-embedding",
        choices=["none", "low", "high"],
        default="none",
        help="Use a local Qwen GGUF embedding server (default: none)",
    )
    cmd.add_argument(
        "--local-embedding-port",
        type=int,
        default=18080,
        help="Local llama-server port (default: 18080)",
    )
    cmd.add_argument(
        "--local-embedding-bin",
        default="llama-server",
        help="llama-server executable name or path (default: llama-server)",
    )
    cmd.add_argument(
        "--keep-local-embedding-server",
        action="store_true",
        help="Leave a dagayn-started local embedding server running after the command finishes",
    )
    cmd.add_argument(
        "--local-embedding-timeout",
        type=int,
        default=300,
        help="Seconds to wait for llama-server readiness (default: 300)",
    )


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
