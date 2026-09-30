"""Command-line and path helpers shared by the build-manifest scanners."""

from __future__ import annotations

import shlex
from pathlib import PurePosixPath


def _split_command(text: str) -> list[str]:
    try:
        return shlex.split(text, comments=False)
    except ValueError:
        return text.split()


def _command_options(tokens: list[str], valued: set[str]) -> tuple[dict[str, str], list[str]]:
    """Split CLI tokens into ``{flag: value}`` and positional arguments."""
    options: dict[str, str] = {}
    positional: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token.startswith("-"):
            flag, eq, value = token.partition("=")
            if eq:
                options[flag] = value
            elif flag in valued and index + 1 < len(tokens):
                options[flag] = tokens[index + 1]
                index += 1
            else:
                options[flag] = ""
        elif "=" not in token or token.startswith("."):
            positional.append(token)
        index += 1
    return options, positional


def _resolve_rel(base_dir: PurePosixPath, declared: str) -> str | None:
    """Resolve *declared* against *base_dir* as a repo-root-relative path.

    Absolute inputs are treated as repo-root-relative by stripping the leading
    slash.  Returns ``None`` when lexical normalization would escape the
    repository root via ``..`` (path traversal).
    """
    raw = declared.strip()
    if not raw:
        return None

    declared_path = PurePosixPath(raw)
    if declared_path.is_absolute() or raw.startswith(("/", "\\")):
        # Treat absolute-looking paths as repo-root-relative by stripping root.
        candidate = PurePosixPath(raw.lstrip("/\\"))
    elif str(base_dir) in ("", "."):
        candidate = declared_path
    else:
        candidate = base_dir / declared_path

    parts: list[str] = []
    for part in candidate.parts:
        if part in ("", ".", "/"):
            continue
        if part == "..":
            if not parts:
                return None
            parts.pop()
            continue
        parts.append(part)
    if not parts:
        return None
    return "/".join(parts)
