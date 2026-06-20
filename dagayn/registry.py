"""Multi-repo registry.

Manages a registry of multiple repositories at ``~/.dagayn/registry.json``.
"""

from __future__ import annotations

import json
import logging
import threading
from pathlib import Path

from dagayn.connection_pool import ConnectionPool

logger = logging.getLogger(__name__)

__all__ = ["ConnectionPool", "Registry", "resolve_repo"]

# Default registry path
_REGISTRY_DIR = Path.home() / ".dagayn"
_REGISTRY_PATH = _REGISTRY_DIR / "registry.json"


class Registry:
    """Manages a JSON-based registry of dagayn repositories.

    Each entry stores the repo path and an optional alias.
    The registry lives at ``~/.dagayn/registry.json``.
    """

    def __init__(self, path: Path | None = None) -> None:
        self._path = path or _REGISTRY_PATH
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = threading.Lock()
        self._repos: list[dict[str, str]] = []
        self._load()

    def _load(self) -> None:
        """Load registry from disk."""
        if self._path.exists():
            try:
                data = json.loads(self._path.read_text(encoding="utf-8", errors="replace"))
                self._repos = data.get("repos", [])
            except (json.JSONDecodeError, KeyError, TypeError):
                logger.warning("Invalid registry file, starting fresh: %s", self._path)
                self._repos = []
        else:
            self._repos = []

    def _save(self) -> None:
        """Write registry to disk."""
        self._path.parent.mkdir(parents=True, exist_ok=True)
        data = {"repos": self._repos}
        self._path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")

    def register(self, path: str, alias: str | None = None) -> dict[str, str]:
        """Register a repository path.

        Validates that the path contains a ``.git`` or ``.dagayn``
        directory.

        Args:
            path: Absolute or relative path to the repository root.
            alias: Optional short alias for the repository.

        Returns:
            The registered entry dict.

        Raises:
            ValueError: If the path is not a valid repository.
        """
        resolved = Path(path).resolve()
        if not resolved.is_dir():
            raise ValueError(f"Path is not a directory: {resolved}")
        if not (resolved / ".git").exists() and not (resolved / ".dagayn").exists():
            raise ValueError(
                f"Path does not look like a repository (no .git or .dagayn): {resolved}"
            )

        with self._lock:
            # Check for duplicate path
            str_path = str(resolved)
            for entry in self._repos:
                if entry["path"] == str_path:
                    # Update alias if provided
                    if alias:
                        entry["alias"] = alias
                        self._save()
                    return entry

            new_entry: dict[str, str] = {"path": str_path}
            if alias:
                new_entry["alias"] = alias
            self._repos.append(new_entry)
            self._save()
            return new_entry

    def unregister(self, path_or_alias: str) -> bool:
        """Remove a repository by path or alias.

        Args:
            path_or_alias: Either the absolute path or the alias.

        Returns:
            True if an entry was removed, False otherwise.
        """
        with self._lock:
            resolved = str(Path(path_or_alias).resolve())
            original_len = len(self._repos)
            self._repos = [
                entry
                for entry in self._repos
                if entry["path"] != resolved and entry.get("alias") != path_or_alias
            ]
            if len(self._repos) < original_len:
                self._save()
                return True
            return False

    def list_repos(self) -> list[dict[str, str]]:
        """Return list of all registered repositories.

        Returns:
            List of dicts with 'path' and optional 'alias' keys.
        """
        with self._lock:
            return list(self._repos)

    def find_by_alias(self, alias: str) -> dict[str, str] | None:
        """Look up a repository by its alias.

        Args:
            alias: The alias to search for.

        Returns:
            The matching entry, or None.
        """
        with self._lock:
            for entry in self._repos:
                if entry.get("alias") == alias:
                    return dict(entry)
            return None

    def find_by_path(self, path: str) -> dict[str, str] | None:
        """Look up a repository by its path.

        Args:
            path: The path to search for.

        Returns:
            The matching entry, or None.
        """
        resolved = str(Path(path).resolve())
        with self._lock:
            for entry in self._repos:
                if entry["path"] == resolved:
                    return dict(entry)
            return None


def resolve_repo(
    registry: Registry,
    repo: str | None,
    cwd: str | None = None,
) -> str | None:
    """Resolve a repo parameter to an absolute path.

    Resolution order:
    1. If repo is given, try as alias first.
    2. If repo is given and not an alias, try as a direct path.
    3. If repo is None, use cwd.

    Args:
        registry: The Registry instance.
        repo: Alias or path string, or None.
        cwd: Current working directory fallback.

    Returns:
        Resolved absolute path string, or None if unresolvable.
    """
    if repo:
        # Try alias first
        entry = registry.find_by_alias(repo)
        if entry:
            return entry["path"]

        # Try as direct path
        path = Path(repo).resolve()
        if path.is_dir():
            return str(path)

    # Fall back to CWD
    if cwd:
        return str(Path(cwd).resolve())

    return None
