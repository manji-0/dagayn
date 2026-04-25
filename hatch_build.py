from __future__ import annotations

import importlib
import shutil
import sys
from pathlib import Path
from typing import Any

try:
    from hatchling.builders.hooks.plugin.interface import BuildHookInterface
except ImportError:  # pragma: no cover - hatchling is only required during packaging

    class BuildHookInterface:  # type: ignore[override]
        pass


ROOT = Path(__file__).resolve().parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

vendor_grammars = importlib.import_module("dagayn.vendor_grammars")
stage_packaged_vendor_grammar_sources = vendor_grammars.stage_packaged_vendor_grammar_sources


def build_force_include_map(staging_root: Path) -> dict[str, str]:
    staged = stage_packaged_vendor_grammar_sources(staging_root)
    return {str(path): f"dagayn/_vendor_grammars/{language}" for language, path in staged.items()}


class CustomBuildHook(BuildHookInterface):
    def initialize(self, version: str, build_data: dict[str, Any]) -> None:
        if version == "editable":
            return
        staging_root = ROOT / ".hatch-vendor-grammars"
        if staging_root.exists():
            shutil.rmtree(staging_root)
        force_include = build_data.setdefault("force_include", {})
        force_include.update(build_force_include_map(staging_root))
