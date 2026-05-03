"""dagayn CLI package."""

from __future__ import annotations

import logging
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as pkg_version  # noqa: F401
from typing import Any

_LAZY_EXPORTS = {
    "_supports_color": (".utils", "_supports_color"),
    "main": (".app", "main"),
}

logger = logging.getLogger(__name__)

__all__ = ["main", "_get_version", "_supports_color", "pkg_version"]


def __getattr__(name: str) -> Any:
    if name not in _LAZY_EXPORTS:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    module_name, attr_name = _LAZY_EXPORTS[name]
    from importlib import import_module

    value = getattr(import_module(module_name, __name__), attr_name)
    globals()[name] = value
    return value


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
