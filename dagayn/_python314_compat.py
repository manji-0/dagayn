"""Compatibility shims for early Python 3.14 builds."""

from __future__ import annotations

import collections.abc
import inspect
import typing
from typing import Any


def patch_typing_eval_type_for_python314_beta() -> None:
    """Keep Pydantic importable on early Python 3.14 builds."""
    eval_type = getattr(typing, "_eval_type", None)
    if eval_type is None:
        return
    try:
        parameters = inspect.signature(eval_type).parameters
    except (TypeError, ValueError):
        return
    if "prefer_fwd_module" in parameters:
        return

    def _eval_type_compat(*args: Any, **kwargs: Any) -> Any:
        kwargs.pop("prefer_fwd_module", None)
        return eval_type(*args, **kwargs)

    setattr(typing, "_eval_type", _eval_type_compat)


def patch_collections_abc_bytestring_for_python314() -> None:
    """Restore the removed ``collections.abc.ByteString`` for old deps."""
    if hasattr(collections.abc, "ByteString"):
        return

    class ByteString(collections.abc.Sequence):
        pass

    ByteString.register(bytes)
    ByteString.register(bytearray)
    ByteString.register(memoryview)
    collections.abc.ByteString = ByteString  # type: ignore[attr-defined]


patch_typing_eval_type_for_python314_beta()
patch_collections_abc_bytestring_for_python314()
