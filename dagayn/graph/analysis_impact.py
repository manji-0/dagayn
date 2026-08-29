"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.analysis_impact``."""
from __future__ import annotations

import sys

from dagayn.legacy_py.graph import analysis_impact as _impl

sys.modules[__name__] = _impl
