"""Dead code detection for graph-powered refactoring.

The analysis itself is ``dagayn_tools::dead_code`` in Rust, reached through
the store's ``find_dead_code_json``: the native ``refactor_tool`` and this
module report the same candidates from one implementation.
"""

from __future__ import annotations

import json
import logging
from typing import Any, Optional

from ..graph import GraphStore

logger = logging.getLogger(__name__)

type DeadValue = Any
type DeadPayload = dict[str, DeadValue]


def _source_line(lines: list[str], line_number: int | None) -> str:
    if line_number is None or line_number <= 0 or line_number > len(lines):
        return ""
    return lines[line_number - 1].strip()


def find_dead_code(
    store: GraphStore,
    kind: Optional[str] = None,
    file_pattern: Optional[str] = None,
) -> list[DeadPayload]:
    """Find functions/classes with no callers, no test refs, no importers, and no references.

    Entry points (functions matching framework decorators or conventional name
    patterns like ``main``, ``test_*``, ``handle_*``) are excluded.

    .. note::

        **Caveats — dynamic dispatch patterns.**  Static analysis cannot track
        all runtime-determined call patterns.  Functions registered via fully
        dynamic keys (``map[computedKey()] = fn``), ``Reflect.apply``, or
        runtime ``require()`` may still appear as dead code.  Treat results as
        hints, especially for TypeScript projects that use map-based dispatch,
        plugin registries, or dynamic requires.

    Args:
        store: The GraphStore instance.
        kind: Optional filter (e.g. ``"Function"`` or ``"Class"``).
        file_pattern: Optional file-path substring filter.

    Returns:
        List of dead-code dicts with name, qualified_name, kind, file, line,
        and a top-level ``caveats`` note. Only candidates that pass
        :func:`dead_code_report`'s checks are listed.
    """
    dead: list[DeadPayload] = dead_code_report(store, kind, file_pattern)["dead"]
    logger.info("find_dead_code: found %d dead symbols", len(dead))
    return dead


def dead_code_report(
    store: GraphStore,
    kind: Optional[str] = None,
    file_pattern: Optional[str] = None,
) -> DeadPayload:
    """:func:`find_dead_code` with what it left out and how the check went.

    A graph candidate (no callers, tests, importers, references, or
    subclasses) is reported only when nothing can still reach it: it is not
    an FFI export, registered by a decorator or Rust attribute, or a trait
    method, and its name appears nowhere in the repository's files outside
    its own definition. The rest are counted under ``suppressed`` by reason;
    ``verification`` says whether every file could be searched.

    Returns:
        ``{"dead": [...], "suppressed": {reason: count}, "verification":
        {"status", "files_scanned", "files_skipped"}}``.
    """
    report: DeadPayload = json.loads(store.find_dead_code_json(kind, file_pattern))
    return report
