"""Language dispatch tables and file-extension / shebang detection."""

from __future__ import annotations

from pathlib import Path
from typing import Optional

EXTENSION_TO_LANGUAGE: dict[str, str] = {
    ".py": "python",
    ".js": "javascript",
    ".jsx": "javascript",
    ".ts": "typescript",
    ".tsx": "tsx",
    ".go": "go",
    ".rs": "rust",
    ".java": "java",
    ".cs": "csharp",
    ".rb": "ruby",
    ".cpp": "cpp",
    ".cc": "cpp",
    ".cxx": "cpp",
    ".c": "c",
    ".h": "c",
    ".hpp": "cpp",
    ".kt": "kotlin",
    ".kts": "kotlin",
    ".swift": "swift",
    ".php": "php",
    ".scala": "scala",
    ".sol": "solidity",
    ".vue": "vue",
    ".dart": "dart",
    ".r": "r",  # .lower() in detect_language handles .R → .r
    ".mjs": "javascript",
    ".astro": "typescript",
    ".pl": "perl",
    ".pm": "perl",
    ".t": "perl",
    ".xs": "c",  # Perl XS: parsed as C to capture functions/structs/includes
    ".lua": "lua",
    ".luau": "luau",
    ".m": "objc",  # Objective-C (.h still maps to C; .mm defers to C++ for simplicity)
    ".sh": "bash",
    ".bash": "bash",
    ".zsh": "bash",
    ".ksh": "bash",  # Korn shell — close enough to bash for tree-sitter-bash (#235)
    ".ex": "elixir",
    ".exs": "elixir",
    ".ipynb": "notebook",
    ".zig": "zig",
    ".ps1": "powershell",
    ".psm1": "powershell",
    ".psd1": "powershell",
    ".svelte": "svelte",
    ".jl": "julia",
    ".gd": "gdscript",
    ".tf": "terraform",
    ".tfvars": "terraform",
    ".md": "markdown",
    ".markdown": "markdown",
}

SHEBANG_INTERPRETER_TO_LANGUAGE: dict[str, str] = {
    "bash": "bash",
    "sh": "bash",
    "zsh": "bash",
    "ksh": "bash",
    "dash": "bash",
    "ash": "bash",
    "python": "python",
    "python2": "python",
    "python3": "python",
    "pypy": "python",
    "pypy3": "python",
    "node": "javascript",
    "nodejs": "javascript",
    "ruby": "ruby",
    "perl": "perl",
    "lua": "lua",
    "Rscript": "r",
    "php": "php",
}

_SHEBANG_PROBE_BYTES = 256


_COMPOUND_EXTENSIONS: tuple[tuple[str, str], ...] = (
    (".tftest.hcl", "terraform"),
    (".tfcomponent.hcl", "terraform"),
    (".tfdeploy.hcl", "terraform"),
    (".tfquery.hcl", "terraform"),
    (".tf.json", "terraform"),
    (".tfvars.json", "terraform"),
)


def detect_language(path: Path) -> Optional[str]:
    name_lower = path.name.lower()
    for compound_ext, lang in _COMPOUND_EXTENSIONS:
        if name_lower.endswith(compound_ext):
            return lang
    suffix = path.suffix.lower()
    lang = EXTENSION_TO_LANGUAGE.get(suffix)
    if lang is not None:
        return lang
    if suffix == "":
        return _detect_language_from_shebang(path)
    return None


def _detect_language_from_shebang(path: Path) -> Optional[str]:
    try:
        with path.open("rb") as fh:
            head = fh.read(_SHEBANG_PROBE_BYTES)
    except (OSError, PermissionError):
        return None
    if not head.startswith(b"#!"):
        return None

    first_line = head.split(b"\n", 1)[0].split(b"\0", 1)[0]
    try:
        line = first_line[2:].decode("utf-8", errors="strict").strip()
    except UnicodeDecodeError:
        return None
    if not line:
        return None

    tokens = line.split()
    if not tokens:
        return None

    first = tokens[0]
    if first.endswith("/env") or first == "env":
        interpreter_token: Optional[str] = None
        for tok in tokens[1:]:
            if tok.startswith("-"):
                continue
            interpreter_token = tok
            break
        if interpreter_token is None:
            return None
        interpreter = interpreter_token.rsplit("/", 1)[-1]
    else:
        interpreter = first.rsplit("/", 1)[-1]

    return SHEBANG_INTERPRETER_TO_LANGUAGE.get(interpreter)
