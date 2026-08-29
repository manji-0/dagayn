"""Compatibility shim. Implementation: ``dagayn.legacy_py.migrations``."""

from __future__ import annotations

import sys

from dagayn.legacy_py import migrations as _impl

sys.modules[__name__] = _impl
