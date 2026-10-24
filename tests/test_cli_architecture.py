"""The architecture metric commands are gone: the overview's findings
(``import_cycle``, ``unstable_dependency``) answer them."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

DAGAYN = Path(sys.executable).with_name("dagayn")


@pytest.mark.parametrize(
    "command", ["detect-adp", "sdp-metrics", "detect-sdp", "sap-metrics", "detect-sap"]
)
def test_metric_command_is_gone(tmp_path: Path, command: str) -> None:
    result = subprocess.run(
        [DAGAYN, command, "--repo", str(tmp_path)], capture_output=True, text=True
    )

    assert result.returncode != 0
    assert "invalid choice" in result.stderr
