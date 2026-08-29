"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.maintenance``."""
from __future__ import annotations

import sys

from dagayn.legacy_py.graph import maintenance as _impl

sys.modules[__name__] = _impl
