"""FTS text normalization helpers (backward-compatible re-export).

Implementation lives in ``dagayn.graph._fts_tokenize`` to avoid import
cycles between ``dagayn.search`` and ``dagayn.graph``.
"""

from __future__ import annotations

from dagayn.graph._fts_tokenize import contains_japanese, segment_japanese_fts_text

__all__ = ["contains_japanese", "segment_japanese_fts_text"]
