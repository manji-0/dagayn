"""FTS text normalization helpers (backward-compatible re-export).

Implementation lives in ``dagayn.graph._fts_tokenize`` to avoid import
cycles between ``dagayn.search`` and ``dagayn.graph``.
"""

from __future__ import annotations

from dagayn.graph._fts_tokenize import (
    FTS_SEGMENTER_METADATA_KEY,
    contains_japanese,
    detect_fts_segmenter,
    segment_cjk_identifier_tokens,
    segment_japanese_fts_text,
)

__all__ = [
    "FTS_SEGMENTER_METADATA_KEY",
    "contains_japanese",
    "detect_fts_segmenter",
    "segment_cjk_identifier_tokens",
    "segment_japanese_fts_text",
]
