"""Apply previewed refactoring edits to source files."""

from __future__ import annotations

import logging
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
    import time

    repo_root = repo_root.resolve()

    with _refactor_lock:
        _cleanup_expired()
        preview = _pending_refactors.get(refactor_id)

    if preview is None:
        logger.warning("apply_refactor: unknown or expired refactor_id %s", refactor_id)
        return {"status": "error", "error": f"Refactor '{refactor_id}' not found or expired."}

    age = time.time() - preview["created_at"]
    if age > REFACTOR_EXPIRY_SECONDS:
        with _refactor_lock:
            _pending_refactors.pop(refactor_id, None)
        logger.warning("apply_refactor: refactor %s expired (%.0fs old)", refactor_id, age)
        return {"status": "error", "error": f"Refactor '{refactor_id}' has expired."}

    edits = preview.get("edits", [])
    if not edits:
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

    for edit in edits:
        edit_path = Path(edit["file"])
        if not edit_path.is_absolute():
            edit_path = repo_root / edit_path
        edit_path = edit_path.resolve()
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

    edits_by_file: dict[str, list[dict]] = defaultdict(list)
    for edit in edits:
        edits_by_file[edit["file"]].append(edit)

    planned: dict[str, tuple[str, str, int]] = {}
    for file_str, file_edits in edits_by_file.items():
        file_path = Path(file_str)
        if not file_path.is_absolute():
            file_path = repo_root / file_path
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
            old_text = edit["old"]
            new_text = edit["new"]
            if old_text not in content:
                logger.warning(
                    "apply_refactor: old text %r not found in %s",
                    old_text,
                    file_path,
                )
                continue
            target_line = edit.get("line")
            if target_line is not None:
                lines = content.splitlines(keepends=True)
                idx = target_line - 1
                if 0 <= idx < len(lines) and old_text in lines[idx]:
                    lines[idx] = lines[idx].replace(old_text, new_text, 1)
                    content = "".join(lines)
                else:
                    content = content.replace(old_text, new_text, 1)
            else:
                content = content.replace(old_text, new_text, 1)
            file_edits_applied += 1

        if file_edits_applied > 0:
            planned[file_str] = (original, content, file_edits_applied)

    if dry_run:
        import difflib

        diffs: dict[str, str] = {}
        for file_str, (original, new_content, _count) in planned.items():
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
        total_edits = sum(count for _o, _n, count in planned.values())
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

    files_modified: set[str] = set()
    edits_applied = 0
    for file_str, (_original, new_content, count) in planned.items():
        file_path = Path(file_str)
        try:
            file_path.write_text(new_content, encoding="utf-8")
            edits_applied += count
            files_modified.add(str(file_path))
            logger.info("apply_refactor: applied %d edit(s) to %s", count, file_path)
        except OSError as exc:
            logger.error("apply_refactor: could not write %s: %s", file_path, exc)

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
