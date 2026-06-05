"""FTS text normalization helpers."""

from __future__ import annotations

import importlib
import re
from collections.abc import Callable
from typing import Any

_JAPANESE_CHAR_RE = re.compile(r"[\u3040-\u30ff\u3400-\u9fff\uf900-\ufaff]")
_FALLBACK_CHUNK_RE = re.compile(
    r"[A-Za-z0-9_]+|[\u3040-\u309f]+|[\u30a0-\u30ff]+|[\u3400-\u9fff\uf900-\ufaff]+"
)

_wakati: Callable[[str], str] | None | bool = None


def contains_japanese(text: str) -> bool:
    """Return True when *text* contains Japanese kana or CJK ideographs."""
    return bool(_JAPANESE_CHAR_RE.search(text))


def _load_wakati() -> Callable[[str], str] | None:
    """Load an optional Japanese tokenizer without making it a hard dependency."""
    global _wakati
    if _wakati is not None:
        return None if _wakati is False else _wakati

    for module_name, factory in (
        ("fugashi", _fugashi_wakati),
        ("MeCab", _mecab_wakati),
        ("janome.tokenizer", _janome_wakati),
    ):
        try:
            module = importlib.import_module(module_name)
            _wakati = factory(module)
            return _wakati
        except Exception:  # noqa: BLE001  # optional dependency probing
            continue

    _wakati = False
    return None


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


def segment_japanese_fts_text(text: str) -> str:
    """Return text suitable for unicode61 FTS when Japanese appears.

    If a MeCab-compatible tokenizer is installed, use it for real word
    segmentation. Otherwise fall back to ASCII-preserving Japanese bigrams so
    Japanese search remains useful without adding a required dependency.
    """
    if not text or not contains_japanese(text):
        return text

    wakati = _load_wakati()
    if wakati is not None:
        segmented = wakati(text)
        if segmented:
            return segmented

    tokens: list[str] = []
    for match in _FALLBACK_CHUNK_RE.finditer(text):
        chunk = match.group(0)
        if chunk.isascii():
            tokens.append(chunk)
            continue
        chars = [ch for ch in chunk if not ch.isspace()]
        if len(chars) <= 2:
            tokens.append("".join(chars))
        else:
            tokens.extend("".join(chars[idx : idx + 2]) for idx in range(len(chars) - 1))
    return " ".join(tokens)
