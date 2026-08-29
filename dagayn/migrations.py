"""Schema version of the native graph store.

Migrations run inside ``dagayn._core.GraphStore`` on open. This module only
exposes the current version so tools can compare against a built database.
"""

from __future__ import annotations

LATEST_VERSION = 16

__all__ = ["LATEST_VERSION"]
