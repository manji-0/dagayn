"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.flow``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import flow as _impl

sys.modules[__name__] = _impl
