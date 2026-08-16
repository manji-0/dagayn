"""FTS text normalization helpers."""

from __future__ import annotations

import importlib
import logging
import re
from collections.abc import Callable
from typing import Any, cast

logger = logging.getLogger(__name__)

FTS_SEGMENTER_METADATA_KEY = "fts_segmenter"

_JAPANESE_CHAR_RE = re.compile(r"[\u3040-\u30ff\u3400-\u9fff\uf900-\ufaff\uac00-\ud7af]")
_FALLBACK_CHUNK_RE = re.compile(
    r"[A-Za-z0-9_]+|[\u3040-\u309f]+|[\u30a0-\u30ff]+|[\u3400-\u9fff\uf900-\ufaff]+|[\uac00-\ud7af]+"
)

_WAKATI_MISSING = object()
_wakati_cache: dict[str, Callable[[str], str] | object] = {}
_detected_segmenter: str | None = None

_SEGMENTER_LOADERS: dict[str, tuple[str, Callable[[Any], Callable[[str], str]]]] = {
    "fugashi": ("fugashi", lambda module: _fugashi_wakati(module)),
    "mecab": ("MeCab", lambda module: _mecab_wakati(module)),
    "janome": ("janome.tokenizer", lambda module: _janome_wakati(module)),
}


def contains_japanese(text: str) -> bool:
    """Return True when *text* contains Japanese kana or CJK ideographs."""
    return bool(_JAPANESE_CHAR_RE.search(text))


def detect_fts_segmenter() -> str:
    """Return the segmenter used for doc_text indexing in this environment."""
    global _detected_segmenter
    if _detected_segmenter is not None:
        return _detected_segmenter
    if _probe_wakati() is not None:
        return _detected_segmenter or "bigram"
    _detected_segmenter = "bigram"
    return "bigram"


def segment_cjk_identifier_tokens(text: str, *, segmenter: str | None = None) -> str:
    """Segment CJK runs for ``identifier_tokens`` indexing.

    When a wakati tokenizer is active, index both tokenizer output and bigrams
    so full-token and partial-prefix queries both remain findable.
    """
    if not text or not contains_japanese(text):
        return ""
    resolved = segmenter or detect_fts_segmenter()
    bigram = _segment_bigram(text, cjk_only=True)
    if resolved == "bigram":
        return bigram
    wakati = segment_japanese_fts_text(text, segmenter=resolved)
    tokens: list[str] = []
    for part in (wakati, bigram):
        tokens.extend(part.split())
    return " ".join(dict.fromkeys(tokens))


def segment_japanese_fts_text(text: str, *, segmenter: str | None = None) -> str:
    """Return text suitable for unicode61 FTS when Japanese appears.

    When *segmenter* is omitted, auto-detect the active tokenizer.  When set,
    force the persisted index-time segmenter so query tokenization matches.
    """
    if not text or not contains_japanese(text):
        return text

    resolved = segmenter or detect_fts_segmenter()
    if resolved == "bigram":
        return _segment_bigram(text)

    wakati = _get_wakati(resolved)
    if wakati is not None:
        segmented = wakati(text)
        if segmented:
            return segmented

    if segmenter and segmenter != "bigram":
        logger.warning(
            "FTS segmenter %r unavailable at query time; falling back to bigram",
            segmenter,
        )
    return _segment_bigram(text)


def _probe_wakati() -> Callable[[str], str] | None:
    """Load the first available Japanese tokenizer for auto-detection."""
    global _detected_segmenter
    for name in _SEGMENTER_LOADERS:
        loaded = _get_wakati(name)
        if loaded is not None:
            _detected_segmenter = name
            return loaded
    _detected_segmenter = "bigram"
    return None


def _get_wakati(segmenter: str) -> Callable[[str], str] | None:
    if segmenter == "bigram":
        return None
    if segmenter in _wakati_cache:
        cached = _wakati_cache[segmenter]
        return None if cached is _WAKATI_MISSING else cast(Callable[[str], str], cached)

    module_name, factory = _SEGMENTER_LOADERS.get(segmenter, (None, None))  # type: ignore[misc]
    if module_name is None or factory is None:
        _wakati_cache[segmenter] = _WAKATI_MISSING
        return None
    try:
        module = importlib.import_module(module_name)
        loaded = factory(module)
    except Exception:  # noqa: BLE001  # nosec B110 - optional dependency probing
        _wakati_cache[segmenter] = _WAKATI_MISSING
        return None

    _wakati_cache[segmenter] = loaded
    return loaded


def _segment_bigram(text: str, *, cjk_only: bool = False) -> str:
    tokens: list[str] = []
    for match in _FALLBACK_CHUNK_RE.finditer(text):
        chunk = match.group(0)
        if chunk.isascii():
            if not cjk_only:
                tokens.append(chunk)
            continue
        chars = [ch for ch in chunk if not ch.isspace()]
        if len(chars) <= 2:
            tokens.append("".join(chars))
        else:
            tokens.extend("".join(chars[idx : idx + 2]) for idx in range(len(chars) - 1))
    return " ".join(tokens)


def _fugashi_wakati(module: Any) -> Callable[[str], str]:
    tagger = module.Tagger("-Owakati")

    def wakati(text: str) -> str:
        return tagger.parse(text).strip()

    return wakati


def _mecab_wakati(module: Any) -> Callable[[str], str]:
    tagger = module.Tagger("-Owakati")

    def wakati(text: str) -> str:
        parsed = tagger.parse(text)
        return parsed.strip() if parsed else ""

    return wakati


def _janome_wakati(module: Any) -> Callable[[str], str]:
    tokenizer = module.Tokenizer()

    def wakati(text: str) -> str:
        return " ".join(token.surface for token in tokenizer.tokenize(text))

    return wakati
