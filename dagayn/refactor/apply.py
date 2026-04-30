"""Apply previewed refactoring edits to source files."""

from __future__ import annotations

import difflib
import logging
import time
from collections import defaultdict
from pathlib import Path
from typing import Any

from .pending import (
    REFACTOR_EXPIRY_SECONDS,
    _cleanup_expired,
    _pending_refactors,
    _refactor_lock,
)

logger = logging.getLogger(__name__)


def _empty_result(*, dry_run: bool) -> dict[str, Any]:
    if dry_run:
        return {
            "status": "ok",
            "dry_run": True,
            "applied": 0,
            "files_modified": [],
            "edits_applied": 0,
            "would_modify": [],
            "diffs": {},
        }
    return {"status": "ok", "applied": 0, "files_modified": [], "edits_applied": 0}


def _resolve_repo_path(path_str: str, repo_root: Path) -> Path:
    path = Path(path_str)
    if not path.is_absolute():
        path = repo_root / path
    return path.resolve()


def _get_valid_preview(refactor_id: str) -> tuple[dict[str, Any] | None, dict[str, str] | None]:
    with _refactor_lock:
        _cleanup_expired()
        preview = _pending_refactors.get(refactor_id)

    if preview is None:
        logger.warning("apply_refactor: unknown or expired refactor_id %s", refactor_id)
        return None, {"status": "error", "error": f"Refactor '{refactor_id}' not found or expired."}

    age = time.time() - preview["created_at"]
    if age > REFACTOR_EXPIRY_SECONDS:
        with _refactor_lock:
            _pending_refactors.pop(refactor_id, None)
        logger.warning("apply_refactor: refactor %s expired (%.0fs old)", refactor_id, age)
        return None, {"status": "error", "error": f"Refactor '{refactor_id}' has expired."}

    return preview, None


def _validate_edit_paths(edits: list[dict[str, Any]], repo_root: Path) -> dict[str, str] | None:
    for edit in edits:
        edit_path = _resolve_repo_path(edit["file"], repo_root)
        try:
            edit_path.relative_to(repo_root)
        except ValueError:
            logger.error(
                "apply_refactor: path traversal blocked for %s (repo_root=%s)",
                edit_path,
                repo_root,
            )
            return {
                "status": "error",
                "error": f"Edit path '{edit['file']}' is outside repo root.",
            }
    return None


def _apply_edit(content: str, file_path: Path, edit: dict[str, Any]) -> tuple[str, bool]:
    old_text = edit["old"]
    new_text = edit["new"]
    if old_text not in content:
        logger.warning("apply_refactor: old text %r not found in %s", old_text, file_path)
        return content, False

    target_line = edit.get("line")
    if target_line is None:
        return content.replace(old_text, new_text, 1), True

    lines = content.splitlines(keepends=True)
    idx = target_line - 1
    if 0 <= idx < len(lines) and old_text in lines[idx]:
        lines[idx] = lines[idx].replace(old_text, new_text, 1)
        return "".join(lines), True

    return content.replace(old_text, new_text, 1), True


def _plan_edits(
    edits: list[dict[str, Any]],
    repo_root: Path,
) -> dict[str, tuple[Path, str, str, int]]:
    edits_by_file: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for edit in edits:
        edits_by_file[edit["file"]].append(edit)

    planned: dict[str, tuple[Path, str, str, int]] = {}
    for file_str, file_edits in edits_by_file.items():
        file_path = _resolve_repo_path(file_str, repo_root)
        if not file_path.is_file():
            logger.warning("apply_refactor: file not found: %s", file_path)
            continue

        try:
            original = file_path.read_text(encoding="utf-8", errors="replace")
        except (OSError, UnicodeDecodeError) as exc:
            logger.warning("apply_refactor: could not read %s: %s", file_path, exc)
            continue

        content = original
        file_edits_applied = 0
        for edit in file_edits:
            content, applied = _apply_edit(content, file_path, edit)
            if applied:
                file_edits_applied += 1

        if file_edits_applied > 0:
            planned[file_str] = (file_path, original, content, file_edits_applied)

    return planned


def _build_dry_run_result(
    refactor_id: str,
    planned: dict[str, tuple[Path, str, str, int]],
) -> dict[str, Any]:
    diffs: dict[str, str] = {}
    for file_str, (_file_path, original, new_content, _count) in planned.items():
        diff_lines = list(
            difflib.unified_diff(
                original.splitlines(keepends=True),
                new_content.splitlines(keepends=True),
                fromfile=f"a/{file_str}",
                tofile=f"b/{file_str}",
                n=3,
            )
        )
        diffs[file_str] = "".join(diff_lines)

    total_edits = sum(count for _file_path, _o, _n, count in planned.values())
    result = {
        "status": "ok",
        "dry_run": True,
        "applied": 0,
        "edits_applied": total_edits,
        "would_modify": sorted(planned.keys()),
        "files_modified": [],
        "diffs": diffs,
    }
    logger.info(
        "apply_refactor: dry-run %s — %d edits would be applied to %d files",
        refactor_id,
        total_edits,
        len(planned),
    )
    return result


def _write_planned_edits(planned: dict[str, tuple[Path, str, str, int]]) -> tuple[int, list[str]]:
    files_modified: set[str] = set()
    edits_applied = 0
    for _file_str, (file_path, _original, new_content, count) in planned.items():
        try:
            file_path.write_text(new_content, encoding="utf-8")
            edits_applied += count
            files_modified.add(str(file_path))
            logger.info("apply_refactor: applied %d edit(s) to %s", count, file_path)
        except OSError as exc:
            logger.error("apply_refactor: could not write %s: %s", file_path, exc)

    return edits_applied, sorted(files_modified)


def apply_refactor(
    refactor_id: str,
    repo_root: Path,
    dry_run: bool = False,
) -> dict[str, Any]:
    """Apply a previously previewed refactoring to source files.

    Validates the refactor_id, checks expiry, ensures all edit paths are
    within the repo root, then performs exact string replacements on the
    target files.

    Args:
        refactor_id: ID from a prior ``rename_preview`` call.
        repo_root: Validated repository root path.
        dry_run: If True, compute the would-be changes and return a
            unified-diff representation per affected file, but do NOT
            write anything to disk. The ``refactor_id`` is preserved so
            the same preview can be committed afterwards via a second
            call without ``dry_run``. See: #176

    Returns:
        Status dict with applied count and modified files. When
        ``dry_run=True`` the dict additionally contains:

        - ``dry_run``: ``True``
        - ``would_modify``: list of file paths that would be changed
        - ``diffs``: map of file path → unified diff string showing the
          proposed change
    """
    repo_root = repo_root.resolve()
    preview, error = _get_valid_preview(refactor_id)
    if error is not None:
        return error
    assert preview is not None

    edits = preview.get("edits", [])
    if not edits:
        return _empty_result(dry_run=dry_run)

    path_error = _validate_edit_paths(edits, repo_root)
    if path_error is not None:
        return path_error

    planned = _plan_edits(edits, repo_root)

    if dry_run:
        return _build_dry_run_result(refactor_id, planned)

    edits_applied, files_modified = _write_planned_edits(planned)

    with _refactor_lock:
        _pending_refactors.pop(refactor_id, None)

    result = {
        "status": "ok",
        "applied": edits_applied,
        "files_modified": sorted(files_modified),
        "edits_applied": edits_applied,
    }
    logger.info("apply_refactor: completed %s — %d edits applied", refactor_id, edits_applied)
    return result
