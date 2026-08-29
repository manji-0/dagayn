"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.storage_batch``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import storage_batch as _impl

sys.modules[__name__] = _impl
