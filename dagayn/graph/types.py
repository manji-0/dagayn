"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.types``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import types as _impl

sys.modules[__name__] = _impl
