"""Shared CLI utilities for dagayn."""

from __future__ import annotations

import logging
import os
import sys
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as pkg_version  # noqa: F401

logger = logging.getLogger(__name__)

# Shared platform choices for install and init commands
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
    "all",
]


def _get_version() -> str:
    """Get the installed package version."""
    try:
        return pkg_version("dagayn")
    except PackageNotFoundError:
        try:
            return pkg_version("dagayn")
        except PackageNotFoundError as exc:
            logger.debug("Package metadata unavailable, falling back to 'dev': %s", exc)
            return "dev"


def _supports_color() -> bool:
    """Check if the terminal likely supports ANSI colors."""
    if os.environ.get("NO_COLOR"):
        return False
    if not hasattr(sys.stdout, "isatty"):
        return False
    return sys.stdout.isatty()


def _print_banner() -> None:
    """Print the startup banner with graph art and available commands."""
    color = _supports_color()
    version = _get_version()

    # ANSI escape codes
    c = "\033[36m" if color else ""  # cyan — graph art
    y = "\033[33m" if color else ""  # yellow — center node
    b = "\033[1m" if color else ""  # bold
    d = "\033[2m" if color else ""  # dim
    g = "\033[32m" if color else ""  # green — commands
    r = "\033[0m" if color else ""  # reset

    print(f"""
{c}  ●──●──●{r}
{c}  │╲ │ ╱│{r}       {b}dagayn{r}  {d}v{version}{r}
{c}  ●──{y}◆{c}──●{r}
{c}  │╱ │ ╲│{r}       {d}Structural knowledge graph for{r}
{c}  ●──●──●{r}       {d}smarter code reviews + Terraform{r}

  {b}Commands:{r}
    {g}install{r}     Set up MCP server for AI coding platforms
    {g}init{r}        Alias for install
    {g}build{r}       Full graph build {d}(parse all files){r}
    {g}update{r}      Incremental update {d}(changed files only){r}
    {g}watch{r}       Auto-update on file changes
    {g}status{r}      Show graph statistics
    {g}visualize{r}   Generate graph reports and exports
    {g}wiki{r}        Generate markdown wiki from communities
    {g}detect-changes{r} Analyze change impact {d}(risk-scored review){r}
    {g}detect-adp{r}    Detect cyclic dependencies {d}(ADP violations){r}
    {g}sdp-metrics{r}   Compute instability scores {d}(SDP metrics){r}
    {g}detect-sdp{r}    Detect stability-direction violations {d}(SDP){r}
    {g}sap-metrics{r}  Compute abstractness/instability/distance {d}(SAP metrics){r}
    {g}detect-sap{r}   Detect scopes far from the main sequence {d}(SAP){r}
    {g}register{r}    Register a repository in the multi-repo registry
    {g}unregister{r}  Remove a repository from the registry
    {g}repos{r}       List registered repositories
    {g}postprocess{r} Run post-processing {d}(flows, communities, FTS){r}
    {g}daemon{r}      Multi-repo watch daemon management
    {g}eval{r}        Run evaluation benchmarks
    {g}serve{r}       Start MCP server {d}(stdio, or {g}--http{r} on localhost:5555){r}

  {d}Run{r} {b}dagayn <command> --help{r} {d}for details{r}
""")


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
