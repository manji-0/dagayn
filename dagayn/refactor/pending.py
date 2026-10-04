"""Thread-safe pending refactors storage.

With the native extension the previews live in ``dagayn._core`` (JSON text in
``dagayn-tools``), so a ``rename`` preview that ``dagayn serve`` answers in
Rust is one ``apply_refactor_tool`` can apply here. Without it, a plain dict.
"""

from __future__ import annotations

import json
import threading
import time
from collections.abc import Iterator, MutableMapping
from typing import Any

_refactor_lock = threading.Lock()
type PendingRefactorPayload = dict[str, Any]
REFACTOR_EXPIRY_SECONDS = 600  # 10 minutes


class _SharedPendingRefactors(MutableMapping[str, PendingRefactorPayload]):
    """``_core``'s pending store as a dict of previews."""

    def __init__(self, core: Any) -> None:
        self._core = core

    def __getitem__(self, refactor_id: str) -> PendingRefactorPayload:
        raw = self._core.pending_refactor_get(refactor_id)
        if raw is None:
            raise KeyError(refactor_id)
        return json.loads(raw)

    def __setitem__(self, refactor_id: str, payload: PendingRefactorPayload) -> None:
        self._core.pending_refactor_set(refactor_id, json.dumps(payload))

    def __delitem__(self, refactor_id: str) -> None:
        if self._core.pending_refactor_remove(refactor_id) is None:
            raise KeyError(refactor_id)

    def __iter__(self) -> Iterator[str]:
        return iter(self._core.pending_refactor_keys())

    def __len__(self) -> int:
        return len(self._core.pending_refactor_keys())

    def clear(self) -> None:
        self._core.pending_refactor_clear()


def _make_store() -> MutableMapping[str, PendingRefactorPayload]:
    try:
        from .. import _core
    except ImportError:
        return {}
    return _SharedPendingRefactors(_core)


_pending_refactors: MutableMapping[str, PendingRefactorPayload] = _make_store()


def _cleanup_expired() -> int:
    """Remove expired refactors from the pending dict.  Returns count removed."""
    now = time.time()
    expired = [
        rid
        for rid, r in list(_pending_refactors.items())
        if now - r["created_at"] > REFACTOR_EXPIRY_SECONDS
    ]
    for rid in expired:
        _pending_refactors.pop(rid, None)
    return len(expired)
