"""Default ignore patterns and ``.dagaynignore`` matching."""

from __future__ import annotations

import fnmatch
from pathlib import Path, PurePosixPath

# Default ignore patterns applied after git's indexable set (tracked +
# untracked, excluding gitignored). ``.dagaynignore`` is an extra restriction.
#
# `<dir>/**` patterns are matched at any depth by _should_ignore, so
# `node_modules/**` also excludes `packages/app/node_modules/react/index.js`
# inside monorepos. See: #91
DEFAULT_IGNORE_PATTERNS = [
    ".dagayn/**",
    "node_modules/**",
    # Git worktrees checked out inside the repository are additional copies of
    # the same history, so indexing them multiplies the whole graph by the
    # number of worktrees. `git ls-files --others` stops at the nested-repo
    # boundary and reports the directory, which the is_file() checks already
    # drop -- but the directory-walk fallback used when git returns nothing has
    # no such boundary, and one such graph reached 1.6M nodes across 39
    # worktrees against 30k for the repository itself.
    ".worktrees/**",
    ".claude/worktrees/**",
    ".git/**",
    ".svn/**",
    "__pycache__/**",
    "*.pyc",
    ".venv/**",
    "venv/**",
    "dist/**",
    "build/**",
    ".next/**",
    "target/**",
    "dagayn/_vendor_grammars/**",
    ".hatch-vendor-grammars/**",
    # PHP / Laravel / Composer
    "vendor/**",
    "bootstrap/cache/**",
    "public/build/**",
    # Ruby / Bundler
    ".bundle/**",
    # Java / Kotlin / Gradle
    ".gradle/**",
    "*.jar",
    # Dart / Flutter
    ".dart_tool/**",
    ".pub-cache/**",
    # General
    "coverage/**",
    ".cache/**",
    "*.min.js",
    "*.min.css",
    "*.map",
    "*.lock",
    "package-lock.json",
    "yarn.lock",
    "*.db",
    "*.sqlite",
    "*.db-journal",
    "*.db-wal",
]


def _load_ignore_patterns(repo_root: Path) -> list[str]:
    """Load ignore patterns from .dagaynignore file."""
    patterns = list(DEFAULT_IGNORE_PATTERNS)
    ignore_file = repo_root / ".dagaynignore"
    if ignore_file.exists():
        for line in ignore_file.read_text(encoding="utf-8", errors="replace").splitlines():
            line = line.strip()
            if line and not line.startswith("#"):
                patterns.append(line)
    return patterns


def _should_ignore(path: str, patterns: list[str]) -> bool:
    """Check if a path matches any ignore pattern.

    Handles nested occurrences of ``<dir>/**`` patterns: for example,
    ``node_modules/**`` also matches ``packages/app/node_modules/foo.js``
    inside monorepos. ``fnmatch`` alone treats ``*`` as not crossing ``/``
    and only matches the prefix, so we additionally test each path segment
    against the bare prefix of ``<dir>/**`` patterns. See: #91
    """
    # Direct fnmatch first (cheap)
    if any(fnmatch.fnmatch(path, p) for p in patterns):
        return True
    # Then: treat simple single-segment "dir/**" patterns as
    # "this directory at any depth".
    parts = PurePosixPath(path).parts
    for p in patterns:
        if not p.endswith("/**"):
            continue
        prefix = p[:-3]
        # Only single-segment dir patterns (no "/" inside the prefix)
        # qualify for nested matching.
        if "/" in prefix or not prefix:
            continue
        if prefix in parts:
            return True
    return False
