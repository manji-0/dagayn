"""The Rust store's JSON answers, parsed, for tests that drive the engine directly."""

from __future__ import annotations

import json
from typing import Any

from dagayn import _core
from dagayn.graph import GraphStore

# `dagayn-tools::pending::EXPIRY_SECONDS`.
REFACTOR_EXPIRY_SECONDS = 600


def entry_points(store: GraphStore, include_tests: bool = False) -> list[dict[str, Any]]:
    return json.loads(store.detect_entry_points_json(include_tests))


def dead_code_report(
    store: GraphStore, kind: str | None = None, file_pattern: str | None = None
) -> dict[str, Any]:
    return json.loads(store.find_dead_code_json(kind, file_pattern))


def find_dead_code(
    store: GraphStore, kind: str | None = None, file_pattern: str | None = None
) -> list[dict[str, Any]]:
    return dead_code_report(store, kind, file_pattern)["dead"]


def suggest_refactorings(store: GraphStore) -> list[dict[str, Any]]:
    return json.loads(store.suggest_refactorings_json())


def put_pending_refactor(refactor_id: str, payload: dict[str, Any]) -> None:
    _core.pending_refactor_set(refactor_id, json.dumps(payload))


def pending_refactor(refactor_id: str) -> dict[str, Any] | None:
    raw = _core.pending_refactor_get(refactor_id)
    return None if raw is None else json.loads(raw)


def pending_refactor_ids() -> list[str]:
    return list(_core.pending_refactor_keys())


def drop_pending_refactor(refactor_id: str) -> None:
    _core.pending_refactor_remove(refactor_id)


def clear_pending_refactors() -> None:
    _core.pending_refactor_clear()
