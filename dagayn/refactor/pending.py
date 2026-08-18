"""Thread-safe pending refactors storage."""

from __future__ import annotations

import threading
import time
from typing import Any

_refactor_lock = threading.Lock()
type PendingRefactorPayload = dict[str, Any]
_pending_refactors: dict[str, PendingRefactorPayload] = {}
REFACTOR_EXPIRY_SECONDS = 600  # 10 minutes


def _cleanup_expired() -> int:
    """Remove expired refactors from the pending dict.  Returns count removed."""
    now = time.time()
    expired = [
        rid
        for rid, r in _pending_refactors.items()
        if now - r["created_at"] > REFACTOR_EXPIRY_SECONDS
    ]
    for rid in expired:
        del _pending_refactors[rid]
    return len(expired)
