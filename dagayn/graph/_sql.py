"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph._sql``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import _sql as _impl

sys.modules[__name__] = _impl
