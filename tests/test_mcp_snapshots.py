"""MCP response snapshots: the black-box parity oracle for the pure Rust port.

Each parity fixture is built with the CLI and queried through ``dagayn serve``
over stdio; payloads must match ``tests/fixtures/parity/__mcp_snapshots__/``.
Set ``DAGAYN_CLI_CMD`` and ``DAGAYN_MCP_SERVER_CMD`` to run the same checks
against another implementation. See ``tools/mcp_snapshot.py``.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "tools"))
import mcp_snapshot  # noqa: E402

REGENERATE = "uv run python tools/mcp_snapshot.py --regenerate"


@pytest.mark.parametrize("name", sorted(mcp_snapshot.FIXTURE_CASES))
def test_tool_responses_match_snapshots(name: str) -> None:
    snapshot_dir = mcp_snapshot.SNAPSHOT_DIR / name
    if not snapshot_dir.is_dir():
        pytest.skip(f"No snapshots for '{name}'. Run:\n  {REGENERATE} {name}")  # ty: ignore[too-many-positional-arguments]

    actual = mcp_snapshot.snapshot_fixture(name)
    expected = {path.stem: path.read_text(encoding="utf-8") for path in snapshot_dir.glob("*.json")}
    assert sorted(actual) == sorted(expected), f"Case list changed. Run:\n  {REGENERATE} {name}"
    mismatched = [case for case in sorted(actual) if actual[case] != expected[case]]
    assert not mismatched, (
        f"{name}: {', '.join(mismatched)} differ from the snapshots. If the change is "
        f"intended, run:\n  {REGENERATE} {name}"
    )


def test_tool_and_prompt_listing_matches_snapshot() -> None:
    path = mcp_snapshot.SNAPSHOT_DIR / "tools_list.json"
    if not path.exists():
        pytest.skip(f"No listing snapshot. Run:\n  {REGENERATE}")  # ty: ignore[too-many-positional-arguments]
    assert mcp_snapshot.snapshot_tools_list() == path.read_text(encoding="utf-8"), (
        f"Tool or prompt surface changed. If intended, run:\n  {REGENERATE}"
    )


class TestNormalize:
    def test_repo_paths_and_volatile_keys_become_placeholders(self, tmp_path: Path) -> None:
        payload = {
            "file": f"{tmp_path}/src/a.py",
            "last_updated": "2026-01-01T00:00:00",
            "refactor_id": "abc123",
            "summary": "apply refactor_id='abc123'",
        }
        assert mcp_snapshot.normalize(payload, tmp_path) == {
            "file": "<REPO>/src/a.py",
            "last_updated": "<VOLATILE>",
            "refactor_id": "<REFACTOR_ID>",
            "summary": "apply refactor_id='<REFACTOR_ID>'",
        }

    @pytest.mark.parametrize(
        "text",
        ["/opt/elsewhere/a.py", "failed to open /tmp/x.db", "built 2026-01-01 00:00:00"],
    )
    def test_unknown_run_specific_values_fail_loudly(self, tmp_path: Path, text: str) -> None:
        with pytest.raises(ValueError, match="unnormalized"):
            mcp_snapshot.normalize({"summary": text}, tmp_path)
