"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph._protocol``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import _protocol as _impl

sys.modules[__name__] = _impl
