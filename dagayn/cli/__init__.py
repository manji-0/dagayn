"""dagayn CLI package."""

from __future__ import annotations

import logging
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as pkg_version  # noqa: F401

from .app import main
from .utils import _supports_color

logger = logging.getLogger(__name__)

__all__ = ["main", "_get_version", "_supports_color", "pkg_version"]


def _get_version() -> str:
    """Get the installed package version.

    Defined here (not in utils.py) so that test_cli.py can monkeypatch
    ``cli.pkg_version`` and have this function pick up the patched version.
    """
    try:
        return pkg_version("dagayn")
    except PackageNotFoundError:
        try:
            return pkg_version("dagayn")
        except PackageNotFoundError as exc:
            logger.debug("Package metadata unavailable, falling back to 'dev': %s", exc)
            return "dev"
