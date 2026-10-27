from __future__ import annotations

import os
from pathlib import Path


def child_env() -> dict[str, str]:
    """``os.environ`` with this package's parent directory first on ``PYTHONPATH``.

    Paired with ``-P`` on the child's command line, this makes ``python -P -m
    dagayn...`` import the same dagayn as the process spawning it, whether that
    is site-packages or an uninstalled checkout, and never a ``dagayn/``
    directory of the project the child runs in.
    """
    env = dict(os.environ)
    package_parent = str(Path(__file__).resolve().parent.parent)
    existing = env.get("PYTHONPATH")
    env["PYTHONPATH"] = f"{package_parent}{os.pathsep}{existing}" if existing else package_parent
    return env
