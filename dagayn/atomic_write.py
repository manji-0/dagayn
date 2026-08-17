"""Atomic replacement for the files ``dagayn install`` writes.

``install`` rewrites configuration that its host applications own and read
concurrently — ``~/.cursor/hooks.json``, ``~/.claude/settings.json``,
``.mcp.json``, the generated hook scripts. A plain ``Path.write_text``
truncates first and writes second, so a reader that lands in between sees a
half-written file. That was observed in practice: ``~/.cursor/hooks.json`` was
left as invalid JSON (trailing comma, later events missing) after an install
raced Cursor's own write, which then broke every hook until it was repaired by
hand. A partially written hook *script* is worse still, since bash may already
be executing the truncated version.

Writing a sibling temporary file and ``os.replace``-ing it into place makes the
swap atomic: a concurrent reader sees either the old file or the new one.
"""

from __future__ import annotations

import errno
import logging
import os
from pathlib import Path

logger = logging.getLogger(__name__)


def write_text_atomic(
    path: str | Path,
    content: str,
    *,
    encoding: str = "utf-8",
) -> None:
    """Write *content* to *path*, replacing it atomically.

    The temporary file is created in the destination directory so the rename
    stays on one filesystem. Any existing file's permission bits are preserved,
    which matters for the executable hook scripts.

    Falls back to a direct write when the atomic path is unavailable (an
    unwritable directory, an exotic filesystem); that is no worse than the
    non-atomic behaviour this replaces.
    """
    dest = Path(path)

    # ``os.replace`` only needs a writable *directory*, so it would happily
    # swap out a file the user cannot write — a home-manager/chezmoi symlink
    # into a read-only store, for instance. A plain write fails there, and that
    # refusal is the correct behaviour: install reports the file as skipped
    # instead of clobbering externally managed configuration.
    if dest.exists() and not os.access(dest, os.W_OK):
        raise PermissionError(errno.EACCES, "File is not writable", str(dest))

    tmp = dest.with_name(f".{dest.name}.dagayn-tmp-{os.getpid()}")

    try:
        mode: int | None = None
        try:
            mode = dest.stat().st_mode
        except OSError:
            mode = None

        tmp.write_text(content, encoding=encoding)
        if mode is not None:
            try:
                os.chmod(tmp, mode)
            except OSError:
                logger.debug("could not carry permissions over to %s", tmp, exc_info=True)
        os.replace(tmp, dest)
    except OSError:
        logger.debug("atomic write failed for %s; writing in place", dest, exc_info=True)
        try:
            tmp.unlink()
        except OSError:
            pass
        dest.write_text(content, encoding=encoding)
