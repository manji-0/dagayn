"""Token counting helpers for eval benchmarks."""

from __future__ import annotations


def count_tokens(text: str) -> tuple[int, str]:
    """Return ``(count, counter_name)`` using tiktoken when available."""
    try:
        import tiktoken  # type: ignore[import-not-found]

        encoding = tiktoken.get_encoding("cl100k_base")
        return len(encoding.encode(text)), "tiktoken:cl100k_base"
    except Exception:
        return len(text) // 4, "char_div_4"
