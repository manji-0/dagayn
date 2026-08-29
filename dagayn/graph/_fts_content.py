"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph._fts_content``."""

from __future__ import annotations

import sys

from dagayn.legacy_py.graph import _fts_content as _impl

sys.modules[__name__] = _impl
