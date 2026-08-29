"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.subgraph``."""
from __future__ import annotations

import sys

from dagayn.legacy_py.graph import subgraph as _impl

sys.modules[__name__] = _impl
