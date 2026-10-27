from __future__ import annotations

import os
from pathlib import Path


def _package_parent() -> str:
    return str(Path(__file__).resolve().parent.parent)


def child_env() -> dict[str, str]:
    """``os.environ`` with this package's parent directory first on ``PYTHONPATH``.

    Paired with ``-P`` on the child's command line, this makes ``python -P -m
    dagayn...`` import the same dagayn as the process spawning it, whether that
    is site-packages or an uninstalled checkout, and never a ``dagayn/``
    directory of the project the child runs in.
    """
    env = dict(os.environ)
    package_parent = _package_parent()
    existing = env.get("PYTHONPATH")
    env["PYTHONPATH"] = f"{package_parent}{os.pathsep}{existing}" if existing else package_parent
    return env


def drop_own_pythonpath() -> None:
    """Undo :func:`child_env` in the child once dagayn is imported, so what the
    child runs sees the caller's ``PYTHONPATH``: ty reads it as search paths,
    and dagayn's own entry would make the project's ``import dagayn`` resolve
    to the dagayn running the indexer."""
    parts = os.environ.get("PYTHONPATH", "").split(os.pathsep)
    if parts[0] != _package_parent():
        return
    if len(parts) == 1:
        del os.environ["PYTHONPATH"]
    else:
        os.environ["PYTHONPATH"] = os.pathsep.join(parts[1:])
