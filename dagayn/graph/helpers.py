"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.helpers``."""
from __future__ import annotations

import sys

from dagayn.legacy_py.graph import helpers as _impl

sys.modules[__name__] = _impl
