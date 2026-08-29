"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.core``."""
from __future__ import annotations

import sys

from dagayn.legacy_py.graph import core as _impl

sys.modules[__name__] = _impl
