"""Python facade over the native ``GraphStore``.

PyO3 methods are read-only. The facade forwards lease/pin state to the rust
object and keeps other attributes assignable so ``close`` can be wrapped for
read-lock unbind and tests.
"""

from __future__ import annotations

from typing import Any

_INNER_SETATTR = frozenset({"_leases", "_pinned"})


class NativeGraphStore:
    """Assignable wrapper around ``dagayn._core.GraphStore``."""

    def __init__(self, inner: Any) -> None:
        object.__setattr__(self, "_inner", inner)

    def __getattr__(self, name: str) -> Any:
        return getattr(object.__getattribute__(self, "_inner"), name)

    def __setattr__(self, name: str, value: Any) -> None:
        if name == "_inner":
            object.__setattr__(self, name, value)
            return
        if name in _INNER_SETATTR:
            setattr(object.__getattribute__(self, "_inner"), name, value)
            return
        object.__setattr__(self, name, value)
