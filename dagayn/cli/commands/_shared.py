"""Shared utilities used only within CLI command modules."""

from __future__ import annotations

import argparse
import sys

from ...local_embeddings import DEFAULT_LOCAL_EMBEDDING_BIN

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


def _add_local_embedding_args(
    cmd: argparse.ArgumentParser,
    *,
    include_mode_alias: bool = True,
) -> None:
    """Add --local-embedding* flags to a subcommand parser."""
    cmd.add_argument(
        "--local-embedding",
        nargs="?",
        const="bge-m3",
        choices=["none", "bge-m3", "low", "llama-qwen3"],
        default="none",
        help=(
            "Generate/search with local embeddings. With no value, use in-process "
            "BAAI/bge-m3. Use 'low' or --mode llama-qwen3 for the managed Qwen3 "
            "llama.cpp sidecar (default: none)."
        ),
    )
    if include_mode_alias:
        cmd.add_argument(
            "--mode",
            dest="local_embedding_mode",
            choices=["bge-m3", "llama-qwen3"],
            default=None,
            help=(
                "Execution mode for --local-embedding: bge-m3 in-process or "
                "llama-qwen3 managed sidecar."
            ),
        )
    cmd.add_argument(
        "--local-embedding-port",
        type=int,
        default=18080,
        help="Local embedding server port (default: 18080)",
    )
    cmd.add_argument(
        "--local-embedding-bin",
        default=DEFAULT_LOCAL_EMBEDDING_BIN,
        help="Local embedding server executable name/path, or 'auto' (default: auto)",
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
        help="Seconds to wait for local embedding server readiness (default: 300)",
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
# AI coding tool when ``--mode remote-embedding`` is selected. Kept in sync by hand
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
    """Interactive menu for selecting the install mode and its sub-option.

    Returns ``(mode, preset, provider)``.  ``preset`` is set only for
    ``local-embedding-llama``, ``provider`` is set only for
    ``remote-embedding``.
    """
    print("Which embedding mode would you like?")
    print("  1) fts-only              — FTS only (no embeddings, fastest)")
    print("  2) local-embedding       — In-process BGE-M3 local embeddings")
    print("  3) local-embedding-llama — Managed Qwen3 llama.cpp sidecar")
    print("  4) remote-embedding      — OpenAI-compatible / Google / MiniMax cloud embeddings")
    choice = _read_choice(
        "Choose [1-4]: ",
        {
            "1": "fts-only",
            "2": "local-embedding",
            "3": "local-embedding-llama",
            "4": "remote-embedding",
        },
    )
    if choice == "fts-only":
        return "fts-only", None, None
    if choice == "local-embedding":
        print("Using local embeddings: BAAI/bge-m3 in-process")
        return "local-embedding", None, None
    if choice == "local-embedding-llama":
        print("Using managed Qwen3 sidecar: Qwen3-Embedding-0.6B (~1 GB)")
        return "local-embedding-llama", "low", None
    # remote
    print("Which provider?")
    print("  1) openai  — OpenAI-compatible API")
    print("  2) google  — Google Gemini")
    print("  3) minimax — MiniMax embo-01")
    provider = _read_choice(
        "Choose [1-3]: ",
        {"1": "openai", "2": "google", "3": "minimax"},
    )
    return "remote-embedding", None, provider


def _normalize_install_mode(mode: str) -> str:
    """Normalize legacy install mode names to the current explicit surface."""
    aliases = {
        "fts": "fts-only",
        "local": "local-embedding",
        "llama-qwen3": "local-embedding-llama",
        "remote": "remote-embedding",
    }
    return aliases.get(mode, mode)


def _resolve_install_mode(args: argparse.Namespace) -> tuple[str, str | None, str | None]:
    """Resolve the install mode from CLI flags or by prompting the user.

    Precedence:
    1. Explicit ``--mode`` flag (with paired ``--preset`` / ``--provider``
       validation).
    2. Legacy ``--local-embedding low`` implies ``local-embedding-llama`` mode.
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
        raw_mode = mode
        mode = _normalize_install_mode(mode)
        if mode == "local-embedding":
            if preset not in (None, "low"):
                raise SystemExit("--mode local-embedding does not accept --preset")
            if preset == "low":
                # Backwards compatibility for the old Qwen spelling.
                if raw_mode == "local":
                    return "local-embedding-llama", "low", provider
                raise SystemExit("--mode local-embedding does not accept --preset")
            return mode, None, provider
        if mode == "local-embedding-llama":
            if preset not in (None, "low"):
                raise SystemExit("--mode local-embedding-llama only supports --preset low")
            return mode, preset or "low", provider
        if mode == "remote-embedding" and not provider:
            raise SystemExit(
                "--mode remote-embedding requires --provider {openai,google,minimax}"
            )
        return mode, preset, provider

    if legacy_le in ("bge-m3", "local"):
        return "local-embedding", None, None
    if legacy_le in ("low", "llama-qwen3"):
        return "local-embedding-llama", "low", None
    if legacy_le not in ("none", ""):
        raise SystemExit("--local-embedding only supports bge-m3, low, or llama-qwen3")

    if auto_yes or not sys.stdin.isatty():
        raise SystemExit(
            "--mode is required when stdin is not a TTY or --yes is set.\n"
            "  Use --mode fts-only | --mode local-embedding | "
            "--mode local-embedding-llama [--preset low] | "
            "--mode remote-embedding --provider {openai,google,minimax}."
        )

    return _prompt_install_mode()
