"""Atomic config writes.

Regression cover for ``~/.cursor/hooks.json`` ending up as invalid JSON (a
trailing comma with later hook events missing) after ``dagayn install`` raced
Cursor's own write of the same file.
"""

from __future__ import annotations

import json
import os
import stat
from pathlib import Path

import pytest

from dagayn.atomic_write import write_text_atomic


def test_creates_a_new_file(tmp_path) -> None:
    dest = tmp_path / "hooks.json"
    write_text_atomic(dest, '{"hooks": {}}\n')
    assert json.loads(dest.read_text(encoding="utf-8")) == {"hooks": {}}


def test_replaces_an_existing_file_wholesale(tmp_path) -> None:
    dest = tmp_path / "hooks.json"
    dest.write_text("old" * 5000, encoding="utf-8")
    write_text_atomic(dest, "new\n")
    assert dest.read_text(encoding="utf-8") == "new\n"


def test_leaves_no_temporary_files_behind(tmp_path) -> None:
    dest = tmp_path / "settings.json"
    write_text_atomic(dest, "{}\n")
    assert [p.name for p in tmp_path.iterdir()] == ["settings.json"]


def test_preserves_the_executable_bit(tmp_path) -> None:
    """Generated hook scripts must stay executable across a re-install."""
    script = tmp_path / "crg-update.sh"
    script.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    script.chmod(0o755)

    write_text_atomic(script, "#!/bin/sh\nexit 1\n")

    assert script.stat().st_mode & stat.S_IXUSR
    assert script.read_text(encoding="utf-8") == "#!/bin/sh\nexit 1\n"


def test_reader_never_sees_a_truncated_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The old content stays readable and valid right up to the swap."""
    dest = tmp_path / "hooks.json"
    original = json.dumps({"hooks": {"afterFileEdit": [{"command": "old"}]}}, indent=2)
    dest.write_text(original, encoding="utf-8")

    observed: list[dict] = []
    real_replace = os.replace

    def _observing_replace(src, dst):
        # Mid-write: a concurrent reader must still parse the previous version,
        # which is exactly what a truncating write breaks.
        observed.append(json.loads(dest.read_text(encoding="utf-8")))
        real_replace(src, dst)

    import dagayn.atomic_write as module

    monkeypatch.setattr(module.os, "replace", _observing_replace)

    write_text_atomic(dest, json.dumps({"hooks": {"afterFileEdit": [{"command": "new"}]}}))

    assert observed == [{"hooks": {"afterFileEdit": [{"command": "old"}]}}]
    assert json.loads(dest.read_text(encoding="utf-8")) == {
        "hooks": {"afterFileEdit": [{"command": "new"}]}
    }


def test_falls_back_when_replace_is_unavailable(tmp_path, monkeypatch) -> None:
    dest = tmp_path / "hooks.json"
    dest.write_text("old\n", encoding="utf-8")

    import dagayn.atomic_write as module

    def _failing_replace(_src, _dst):
        raise OSError("cross-device link")

    monkeypatch.setattr(module.os, "replace", _failing_replace)

    write_text_atomic(dest, "new\n")

    # Degrading to a direct write is no worse than the behaviour it replaced.
    assert dest.read_text(encoding="utf-8") == "new\n"
    assert [p.name for p in tmp_path.iterdir()] == ["hooks.json"]


def test_missing_parent_directory_raises(tmp_path) -> None:
    with pytest.raises(OSError):
        write_text_atomic(tmp_path / "absent" / "hooks.json", "{}\n")


class TestRefusesExternallyManagedFiles:
    """A rename needs only a writable directory, so refuse explicitly.

    ``~/.claude/CLAUDE.md`` is a read-only symlink into the nix store on
    home-manager setups. A plain write fails there and install reports the file
    as skipped; an unguarded ``os.replace`` would instead replace the symlink
    and silently detach the file from the tool that manages it.
    """

    def test_read_only_file_is_not_replaced(self, tmp_path) -> None:
        dest = tmp_path / "CLAUDE.md"
        dest.write_text("managed elsewhere\n", encoding="utf-8")
        dest.chmod(0o444)

        try:
            with pytest.raises(PermissionError):
                write_text_atomic(dest, "dagayn instructions\n")
            assert dest.read_text(encoding="utf-8") == "managed elsewhere\n"
            assert not list(tmp_path.glob(".*dagayn-tmp*"))
        finally:
            dest.chmod(0o644)

    def test_symlink_to_a_read_only_target_is_not_replaced(self, tmp_path) -> None:
        store = tmp_path / "store"
        store.mkdir()
        target = store / "CLAUDE.md"
        target.write_text("nix managed\n", encoding="utf-8")
        target.chmod(0o444)
        link = tmp_path / "CLAUDE.md"
        link.symlink_to(target)

        try:
            with pytest.raises(PermissionError):
                write_text_atomic(link, "dagayn instructions\n")
            assert link.is_symlink()
            assert target.read_text(encoding="utf-8") == "nix managed\n"
        finally:
            target.chmod(0o644)

    def test_writable_file_is_still_replaced(self, tmp_path) -> None:
        dest = tmp_path / "hooks.json"
        dest.write_text("{}\n", encoding="utf-8")
        dest.chmod(0o644)

        write_text_atomic(dest, '{"hooks": {}}\n')

        assert dest.read_text(encoding="utf-8") == '{"hooks": {}}\n'
