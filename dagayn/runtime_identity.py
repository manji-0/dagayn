"""Runtime identity for tool responses (``_runtime``).

Kept free of heavy imports: ``dagayn.server.proxy`` hands the same record to
the tools answered in Rust before the Python server starts.
"""

from __future__ import annotations

import os
import sys
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as pkg_version
from pathlib import Path
from typing import TypedDict


class RuntimeSummaryRecord(TypedDict):
    package: str
    version: str
    pid: int
    python: str
    package_root: str


def runtime_summary() -> RuntimeSummaryRecord:
    """Return compact runtime identity for comparing CLI and MCP responses."""
    try:
        version = pkg_version("dagayn")
    except PackageNotFoundError:
        version = "dev"
    return {
        "package": "dagayn",
        "version": version,
        "pid": os.getpid(),
        "python": sys.executable,
        "package_root": str(Path(__file__).resolve().parent),
    }
