"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph._mixin_protocol``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import _mixin_protocol as _impl

sys.modules[__name__] = _impl
