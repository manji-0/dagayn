"""The TypeScript SCIP indexer dagayn ships, as a command.

``python -P -m dagayn.scip_typescript index --output OUT [tsconfig...]``, run
from the project directory, indexes the named tsconfigs (``tsconfig.json``
when none is named) with the tsrs checker in ``dagayn._core`` and writes the
SCIP index to ``OUT``. :func:`dagayn.scip_overlay.run_scip_overlay` runs it
as a child process like any other indexer, so a checker panic (the release
build aborts) ends only the indexer, its memory returns when it exits, and
``DAGAYN_SCIP_TIMEOUT`` bounds it. ``-P`` keeps a ``dagayn/`` directory of
the project off ``sys.path``; ``PYTHONPATH`` (:func:`dagayn.child_env.child_env`)
names the dagayn to import.

The report (document and occurrence counts, skipped tsconfigs) is printed as
one JSON line; a failure is ``error: ...`` on stderr and exit status 1.
"""

from __future__ import annotations

import os
import signal
import sys

from dagayn import _core
from dagayn.scip_overlay import parse_typescript_arguments


def main(argv: list[str]) -> int:
    try:
        output, tsconfigs = parse_typescript_arguments(argv)
        report = _core.scip_index_typescript(os.getcwd(), tsconfigs, str(output))
    except (ValueError, RuntimeError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    print(report)
    return 0


if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    sys.exit(main(sys.argv[1:]))
