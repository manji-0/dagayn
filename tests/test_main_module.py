"""Tests for dagayn/__main__.py (python -m dagayn)."""

from __future__ import annotations

import subprocess
import sys


def test_module_help_exits_zero():
    result = subprocess.run(
        [sys.executable, "-m", "dagayn", "--help"],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0


def test_module_help_mentions_dagayn():
    result = subprocess.run(
        [sys.executable, "-m", "dagayn", "--help"],
        capture_output=True,
        text=True,
    )
    output = result.stdout + result.stderr
    assert "dagayn" in output.lower()
