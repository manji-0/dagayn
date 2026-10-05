"""Dead code detection for graph-powered refactoring.

The analysis itself is ``dagayn_tools::dead_code`` in Rust, reached through
the store's ``find_dead_code_json``: the native ``refactor_tool`` and this
module report the same candidates from one implementation. The helpers here
read source lines for the callers that still need them.
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any, Optional

from ..graph import GraphStore

logger = logging.getLogger(__name__)

type DeadValue = Any
type DeadPayload = dict[str, DeadValue]


def _load_source_lines(store: GraphStore, file_path: str) -> list[str]:
    try:
        path = store.resolve_file_path(file_path)
    except (AttributeError, TypeError):
        path = Path(file_path)
    try:
        return path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError):
        return []


def _source_line(lines: list[str], line_number: int | None) -> str:
    if line_number is None or line_number <= 0 or line_number > len(lines):
        return ""
    return lines[line_number - 1].strip()


def _is_source_public_api_candidate(node: Any, lines: list[str]) -> bool:
    line = _source_line(lines, node.line_start)
    if not line:
        return False
    public_markers = (
        "pub ",
        "pub(",
        "public ",
        "export ",
        "export default ",
        "export async ",
        "export function ",
        "export class ",
        "export interface ",
        "export const ",
        "export let ",
        "export var ",
    )
    if line.startswith(public_markers):
        return True
    if node.language in {"typescript", "tsx", "javascript", "vue", "svelte"}:
        return " export " in f" {line} " or line.startswith("exports.")
    return False


def _is_bridge_export_candidate(node: Any, lines: list[str]) -> bool:
    if node.language != "rust":
        return False
    line_number = node.line_start
    if not isinstance(line_number, int) or line_number <= 0 or not lines:
        return False

    target_idx = min(line_number - 1, len(lines) - 1)
    for idx in range(target_idx, -1, -1):
        line = lines[idx]
        if not line.lstrip().startswith("impl "):
            continue
        window = "\n".join(lines[max(0, idx - 5) : idx + 1])
        if "#[pymethods]" not in window:
            continue
        depth = 0
        for scoped_line in lines[idx : target_idx + 1]:
            depth += scoped_line.count("{")
            depth -= scoped_line.count("}")
        if depth > 0:
            return True
    return False


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
