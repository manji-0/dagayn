from __future__ import annotations

from pathlib import Path

import hatch_build


def test_build_force_include_map_targets_packaged_vendor_dir(monkeypatch, tmp_path: Path):
    staged = {
        "markdown": tmp_path / "bundle" / "markdown",
        "terraform": tmp_path / "bundle" / "terraform",
    }

    def fake_stage(destination_root: Path):
        assert destination_root == tmp_path / "bundle"
        return staged

    monkeypatch.setattr(hatch_build, "stage_packaged_vendor_grammar_sources", fake_stage)

    mapping = hatch_build.build_force_include_map(tmp_path / "bundle")

    assert mapping == {
        str(staged["markdown"]): "dagayn/_vendor_grammars/markdown",
        str(staged["terraform"]): "dagayn/_vendor_grammars/terraform",
    }


def test_custom_build_hook_populates_force_include(monkeypatch, tmp_path: Path):
    monkeypatch.setattr(hatch_build, "ROOT", tmp_path)
    monkeypatch.setattr(
        hatch_build,
        "build_force_include_map",
        lambda staging_root: {str(staging_root / "markdown"): "dagayn/_vendor_grammars/markdown"},
    )

    hook = hatch_build.CustomBuildHook()
    build_data: dict[str, object] = {}

    hook.initialize("standard", build_data)

    assert build_data["force_include"] == {
        str(tmp_path / ".hatch-vendor-grammars" / "markdown"): "dagayn/_vendor_grammars/markdown"
    }
