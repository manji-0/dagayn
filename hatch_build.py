from __future__ import annotations

import importlib
import sys
from pathlib import Path
from typing import Any

from hatchling.builders.hooks.plugin.interface import BuildHookInterface

ROOT = Path(__file__).resolve().parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

ensure_all_vendor_grammar_sources = importlib.import_module(
    "dagayn.vendor_grammars"
).ensure_all_vendor_grammar_sources


class CustomBuildHook(BuildHookInterface):
    def initialize(self, version: str, build_data: dict[str, Any]) -> None:
        ensure_all_vendor_grammar_sources()
