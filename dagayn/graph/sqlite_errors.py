"""Compatibility shim. Implementation: ``dagayn.legacy_py.graph.sqlite_errors``."""
from __future__ import annotations

import sys

from dagayn.legacy_py.graph import sqlite_errors as _impl

sys.modules[__name__] = _impl
