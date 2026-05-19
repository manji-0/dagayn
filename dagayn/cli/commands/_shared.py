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
    "pi",
    "hermes",
    "all",
]


def _add_local_embedding_args(cmd: argparse.ArgumentParser) -> None:
    """Add --local-embedding* flags to a subcommand parser."""
    cmd.add_argument(
        "--local-embedding",
        choices=["none", "low"],
        default="none",
        help="Use the local Qwen 0.6B GGUF embedding server (default: none)",
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
    cmd.add_argument(
        "--local-embedding-request-timeout",
        type=int,
        default=60,
        help="Seconds to wait for each local embedding request (default: 60)",
    )
    cmd.add_argument(
        "--local-embedding-batch-size",
        type=int,
        default=1,
        help="Texts per local embedding request (default: 1)",
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


# Environment variables a user needs to set in the shell that launches their
# AI coding tool when ``--mode remote`` is selected.  Kept in sync by hand
# with ``dagayn/embeddings.py:get_provider``.  See: test_resolve_install_mode.
_REMOTE_ENV_VARS = {
    "openai": ["CRG_OPENAI_API_KEY", "CRG_OPENAI_BASE_URL", "CRG_OPENAI_MODEL"],
    "google": ["GOOGLE_API_KEY"],
    "minimax": ["MINIMAX_API_KEY"],
}


def _read_choice(prompt: str, mapping: dict[str, str]) -> str:
    """Loop until the user picks a valid menu item.

    ``mapping`` maps the keys the user can type (e.g. ``"1"``) to the
    canonical value to return (e.g. ``"fts"``).  KeyboardInterrupt and
    EOF abort the install with a clear ``SystemExit``.
    """
    while True:
        try:
            ans = input(prompt).strip()
        except (EOFError, KeyboardInterrupt):
            print()
            raise SystemExit("Aborted.")
        if ans in mapping:
            return mapping[ans]
        print(f"  Please enter one of: {', '.join(sorted(mapping))}")


def _prompt_install_mode() -> tuple[str, str | None, str | None]:
    """Interactive 1-2-3 menu for selecting the install mode and its sub-option.

    Returns ``(mode, preset, provider)``.  ``preset`` is set only for
    ``local``, ``provider`` is set only for ``remote``.
    """
    print("Which embedding mode would you like?")
    print("  1) fts    — FTS only (no embeddings, fastest, no model download)")
    print("  2) local  — Managed llama.cpp sidecar with Qwen3 GGUF")
    print("  3) remote — OpenAI-compatible / Google / MiniMax cloud embeddings")
    choice = _read_choice(
        "Choose [1-3]: ",
        {"1": "fts", "2": "local", "3": "remote"},
    )
    if choice == "fts":
        return "fts", None, None
    if choice == "local":
        print("Using local preset: low — Qwen3-Embedding-0.6B (~1 GB)")
        return "local", "low", None
    # remote
    print("Which provider?")
    print("  1) openai  — OpenAI-compatible API")
    print("  2) google  — Google Gemini")
    print("  3) minimax — MiniMax embo-01")
    provider = _read_choice(
        "Choose [1-3]: ",
        {"1": "openai", "2": "google", "3": "minimax"},
    )
    return "remote", None, provider


def _resolve_install_mode(args: argparse.Namespace) -> tuple[str, str | None, str | None]:
    """Resolve the install mode from CLI flags or by prompting the user.

    Precedence:
    1. Explicit ``--mode`` flag (with paired ``--preset`` / ``--provider``
       validation).
    2. Legacy ``--local-embedding low`` implies ``local`` mode.
    3. Otherwise: interactive prompt on a TTY, fail-fast under ``-y`` or
       a non-TTY stdin.

    Returns ``(mode, preset, provider)``.
    """
    mode = getattr(args, "mode", None)
    preset = getattr(args, "preset", None)
    provider = getattr(args, "provider", None)
    legacy_le = getattr(args, "local_embedding", "none") or "none"
    auto_yes = getattr(args, "yes", False)

    if mode is not None:
        if mode == "local":
            if preset not in (None, "low"):
                raise SystemExit("--mode local only supports --preset low")
            return mode, preset or "low", provider
        if mode == "remote" and not provider:
            raise SystemExit("--mode remote requires --provider {openai,google,minimax}")
        return mode, preset, provider

    if legacy_le == "low":
        return "local", legacy_le, None
    if legacy_le not in ("none", ""):
        raise SystemExit("--local-embedding only supports low")

    if auto_yes or not sys.stdin.isatty():
        raise SystemExit(
            "--mode is required when stdin is not a TTY or --yes is set.\n"
            "  Use --mode fts | --mode local [--preset low] | "
            "--mode remote --provider {openai,google,minimax}."
        )

    return _prompt_install_mode()
