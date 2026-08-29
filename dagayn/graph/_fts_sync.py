"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph._fts_sync``."""
from __future__ import annotations

import sys

from dagayn.legacy_py.graph import _fts_sync as _impl

sys.modules[__name__] = _impl
