"""The Python SCIP indexer dagayn ships, as a command.

``python -P -m dagayn.scip_python index --output OUT [--python ENV]``, run
from the project directory, indexes the project with the ty checker in
``dagayn._core`` and writes the SCIP index to ``OUT``. ``ENV`` is the
project's virtual environment (or interpreter), where calls into installed
packages resolve. Without it ty looks for one itself (``VIRTUAL_ENV``, a
``.venv``, a ``python3`` on ``PATH``) and, finding none, uses the
site-packages of the interpreter running dagayn.
:func:`dagayn.scip_overlay.run_scip_overlay` runs it as a child process like
any other indexer, so a checker panic (the release build aborts) ends only
the indexer, its memory returns when it exits, and ``DAGAYN_SCIP_TIMEOUT``
bounds it. ``-P`` keeps a ``dagayn/`` directory of the project off
``sys.path``; ``PYTHONPATH`` (:func:`dagayn.child_env.child_env`) names the
dagayn to import, and that entry is dropped before ty, which reads
``PYTHONPATH`` as search paths, starts.

The report (document and occurrence counts, the environment) is printed as
one JSON line; a failure is ``error: ...`` on stderr and exit status 1.
"""

from __future__ import annotations

import os
import signal
import sys

from dagayn import _core
from dagayn.child_env import drop_own_pythonpath
from dagayn.scip_overlay import parse_python_arguments


def main(argv: list[str]) -> int:
    drop_own_pythonpath()
    try:
        output, python = parse_python_arguments(argv)
        report = _core.scip_index_python(os.getcwd(), python, str(output))
    except (ValueError, RuntimeError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    print(report)
    return 0


if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    sys.exit(main(sys.argv[1:]))
