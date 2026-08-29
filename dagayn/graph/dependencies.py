"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.dependencies``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import dependencies as _impl

sys.modules[__name__] = _impl
