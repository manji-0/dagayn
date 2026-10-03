"""The ``dagayn`` console script.

``build``, ``update``, and ``status`` run in the Rust CLI compiled into
``dagayn._core``, which answers ``None`` for any command line it does not
handle exactly as the Python CLI would; everything else, and those commands
under ``DAGAYN_PYTHON_CLI=1``, runs the Python CLI. Only ``dagayn._core`` is
imported before that decision, so a hook's ``dagayn update`` does not pay for
loading the Python CLI. ``python -m dagayn`` always runs the Python CLI.
"""

import os
import signal
import sys


def main() -> None:
    if os.environ.get("DAGAYN_PYTHON_CLI", "").strip().lower() not in ("1", "true", "yes"):
        from dagayn import _core

        # Python only acts on SIGINT between bytecodes, which never come while
        # the Rust CLI runs; let Ctrl-C stop the process as it would the binary.
        handler = signal.signal(signal.SIGINT, signal.SIG_DFL)
        status = _core.run_cli(["dagayn", *sys.argv[1:]])
        if status is not None:
            sys.exit(status)
        signal.signal(signal.SIGINT, handler)

    from dagayn.cli import main as cli_main

    cli_main()
