"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph._fts_tokenize``."""
from __future__ import annotations

import sys

from dagayn.legacy_py.graph import _fts_tokenize as _impl

sys.modules[__name__] = _impl
