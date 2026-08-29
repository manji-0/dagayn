"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.storage_metadata``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import storage_metadata as _impl

sys.modules[__name__] = _impl
