"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.community``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import community as _impl

sys.modules[__name__] = _impl
