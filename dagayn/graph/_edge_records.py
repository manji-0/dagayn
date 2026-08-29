"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph._edge_records``."""
from __future__ import annotations

import sys

from dagayn.legacy_py.graph import _edge_records as _impl

sys.modules[__name__] = _impl
