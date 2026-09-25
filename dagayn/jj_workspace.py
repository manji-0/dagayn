"""Jujutsu (jj) secondary workspace awareness.

``jj workspace add`` (used by ``track`` and ``jj-task``) creates a working copy
that has a ``.jj/`` directory but no ``.git``. Git commands run from inside one
walk up to the enclosing repository instead, so every git-derived answer —
repository root, file list, HEAD, working-tree changes — describes the main
checkout rather than the workspace the agent is editing.

A git-backed workspace still stores its commits in the shared git object
database. This module reads the two jj facts that matter — the working-copy
commit (``@``) and its first parent (``@-``) — and exposes the backing git
directory, so callers can answer everything else with commit-to-commit git
commands that never consult git's own working tree or index:

* ``@-`` plays the role of ``HEAD``;
* ``@-`` → ``@`` is the uncommitted working-tree change;
* the tree of ``@`` is the indexable file set (jj snapshots honor
  ``.gitignore``).

A colocated main workspace has both ``.jj`` and ``.git`` and keeps being
handled as a plain git checkout.
"""

from __future__ import annotations

import logging
import os
import re
import subprocess  # nosec B404 — jj/git metadata queries with fixed argv
from dataclasses import dataclass
from pathlib import Path

logger = logging.getLogger(__name__)

_JJ_TIMEOUT = int(os.environ.get("CRG_GIT_TIMEOUT", "30"))

#: ``HEAD``, ``HEAD~2``, ``HEAD^``, ``HEAD^2~1`` — git spellings of "relative
#: to the current commit", which in a jj workspace means relative to ``@-``.
_HEAD_RELATIVE = re.compile(r"^HEAD((?:[~^]\d*)*)$")


def _read_link(path: Path) -> Path | None:
    """Resolve a jj pointer file holding an absolute or relative path."""
    try:
        text = path.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    if not text:
        return None
    target = Path(text)
    return target if target.is_absolute() else (path.parent / target)


def jj_git_dir(root: Path) -> Path | None:
    """Return the git directory backing the jj workspace at *root*.

    ``.jj/repo`` is a directory in the main workspace and a file pointing at
    it in every secondary workspace. ``store/git_target`` then names the git
    directory relative to the store. Returns ``None`` for directories without
    ``.jj`` and for jj repositories that are not git-backed.
    """
    jj_dir = root / ".jj"
    if not jj_dir.is_dir():
        return None
    repo = jj_dir / "repo"
    if repo.is_file():
        repo_dir = _read_link(repo)
    elif repo.is_dir():
        repo_dir = repo
    else:
        return None
    if repo_dir is None:
        return None
    git_dir = _read_link(repo_dir / "store" / "git_target")
    if git_dir is None:
        return None
    try:
        git_dir = git_dir.resolve()
    except (OSError, RuntimeError):
        return None
    return git_dir if git_dir.is_dir() else None


def is_jj_workspace(root: Path) -> bool:
    """True for a git-backed jj workspace root that has no ``.git`` of its own."""
    return not (root / ".git").exists() and jj_git_dir(root) is not None


def main_workspace_root(root: Path) -> Path | None:
    """Return the colocated main checkout that owns the jj workspace *root*."""
    git_dir = jj_git_dir(root)
    if git_dir is None or git_dir.name != ".git":
        return None
    return git_dir.parent


def _run(argv: list[str], cwd: Path) -> str | None:
    try:
        result = subprocess.run(  # nosec B603 B607 — fixed argv, no shell
            argv,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=str(cwd),
            timeout=_JJ_TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        logger.debug("%s failed (rc=%d): %s", argv[:3], result.returncode, result.stderr[:200])
        return None
    return result.stdout


def _jj(root: Path, *args: str) -> str | None:
    return _run(["jj", "--no-pager", "--color=never", "-R", str(root), *args], root)


def git_argv(root: Path, *args: str) -> list[str] | None:
    """Return a git argv bound to the workspace's git dir and work tree."""
    git_dir = jj_git_dir(root)
    if git_dir is None:
        return None
    return ["git", f"--git-dir={git_dir}", f"--work-tree={root}", *args]


def run_git(root: Path, *args: str) -> str | None:
    """Run git against the workspace's backing repository; ``None`` on failure."""
    argv = git_argv(root, *args)
    if argv is None:
        return None
    return _run(argv, root)


@dataclass(frozen=True)
class WorkingCopy:
    """Commit ids of ``@`` and its first parent, plus the nearest bookmark."""

    commit: str
    parent: str
    bookmark: str


_WC_TEMPLATE = (
    'commit_id ++ " " ++ parents.map(|c| c.commit_id()).join(",") ++ " "'
    ' ++ local_bookmarks.map(|b| b.name()).join(",") ++ " "'
    ' ++ parents.map(|c| c.local_bookmarks().map(|b| b.name()).join(",")).join(",")'
    ' ++ "\\n"'
)


def working_copy(root: Path) -> WorkingCopy | None:
    """Snapshot the workspace and return its ``@`` / ``@-`` commit ids.

    Running ``jj log`` snapshots the working copy first, so ``@`` already
    contains the files on disk. The bookmark is the first local bookmark on
    ``@``, else on ``@-``; ``track`` names its PR head that way.
    """
    out = _jj(root, "log", "--no-graph", "-r", "@", "-T", _WC_TEMPLATE)
    if not out:
        return None
    fields = out.strip("\n").split(" ")
    if len(fields) < 2 or not fields[0]:
        return None
    parents = [sha for sha in fields[1].split(",") if sha]
    if not parents:
        return None
    own = [name for name in (fields[2] if len(fields) > 2 else "").split(",") if name]
    inherited = [name for name in (fields[3] if len(fields) > 3 else "").split(",") if name]
    bookmark = (own or inherited or [""])[0]
    return WorkingCopy(commit=fields[0], parent=parents[0], bookmark=bookmark)


def resolve_commit(root: Path, ref: str, wc: WorkingCopy | None = None) -> str | None:
    """Resolve a git-style *ref* inside the jj workspace *root* to a full sha.

    ``HEAD``-relative refs are rebased onto ``@-``; everything else (shas,
    branches, ``origin/main``) resolves against the shared git refs.
    """
    match = _HEAD_RELATIVE.match(ref)
    if match:
        wc = wc or working_copy(root)
        if wc is None:
            return None
        ref = wc.parent + match.group(1)
    out = run_git(root, "rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}")
    return out.strip() if out and out.strip() else None


def tree_files(root: Path, commit: str) -> list[str]:
    """Return every path in *commit*'s tree (NUL-safe)."""
    out = run_git(root, "ls-tree", "-r", "-z", "--name-only", "--full-tree", commit)
    if not out:
        return []
    return [path for path in out.split("\0") if path]


def is_ignored(root: Path, relative: str) -> bool:
    """True when the workspace's ``.gitignore`` rules ignore *relative*.

    ``--no-index`` keeps the main checkout's index out of the answer: the
    workspace's files are not in it, and the main checkout lists
    ``.worktrees/`` as excluded.
    """
    argv = git_argv(root, "check-ignore", "-q", "--no-index", "--", relative)
    if argv is None:
        return False
    try:
        result = subprocess.run(  # nosec B603 B607 — fixed argv, no shell
            argv,
            capture_output=True,
            cwd=str(root),
            timeout=_JJ_TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return result.returncode == 0
