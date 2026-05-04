"""profile command — wrap any other dagayn subcommand in pyinstrument.

The profiler is an optional dev dependency. When it is missing the
command degrades to a clear error message rather than crashing.
"""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

DEFAULT_OUTPUT_DIR = Path(".dagayn/profiles")


def register_command(sub: argparse._SubParsersAction) -> argparse.ArgumentParser:
    """Register the ``dagayn profile`` subcommand."""
    parser = sub.add_parser(
        "profile",
        help="Run another dagayn subcommand under a CPU profiler.",
        description=(
            "Wrap any other dagayn subcommand in a pyinstrument profile. "
            "The HTML report is written to .dagayn/profiles/ by default."
        ),
    )
    parser.add_argument(
        "--output-dir",
        default=str(DEFAULT_OUTPUT_DIR),
        help=(f"Directory to write the HTML profile report to (default: {DEFAULT_OUTPUT_DIR})."),
    )
    parser.add_argument(
        "--interval",
        type=float,
        default=0.001,
        help="Sampling interval in seconds (default: 0.001).",
    )
    parser.add_argument(
        "--open",
        action="store_true",
        dest="open_browser",
        help="Open the HTML report in a browser when finished.",
    )
    parser.add_argument(
        "profile_command",
        nargs=argparse.REMAINDER,
        help="The dagayn subcommand and its arguments to profile.",
        metavar="command",
    )
    return parser


def _import_profiler() -> type | None:
    try:
        from pyinstrument import Profiler  # ty: ignore[unresolved-import]
    except ImportError:
        return None
    return Profiler


def handle(args: argparse.Namespace) -> int:
    """Run *args.command* under pyinstrument and write an HTML report."""
    profiler_cls = _import_profiler()
    if profiler_cls is None:
        print(
            "pyinstrument is not installed.\n"
            "Install dev dependencies: uv sync --extra dev "
            "(or pip install pyinstrument).",
            file=sys.stderr,
        )
        return 2

    sub_argv = list(getattr(args, "profile_command", None) or [])
    if not sub_argv:
        print(
            "Specify a subcommand to profile, e.g. `dagayn profile build`.",
            file=sys.stderr,
        )
        return 2

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    if sub_argv and sub_argv[0] == "--":
        sub_argv = sub_argv[1:]

    label = sub_argv[0] if sub_argv else "command"
    safe_label = "".join(c if c.isalnum() or c in "-_" else "_" for c in label)
    timestamp = time.strftime("%Y%m%dT%H%M%S")
    out_path = output_dir / f"profile_{safe_label}_{timestamp}.html"

    # Re-enter the dagayn CLI under the profiler. Replace argv so the
    # nested ``main()`` call sees the wrapped subcommand.
    profiler = profiler_cls(interval=args.interval)
    saved_argv = sys.argv[:]
    sys.argv = [saved_argv[0], *sub_argv]
    rc = 0
    try:
        from .. import app

        profiler.start()
        try:
            app.main()
        except SystemExit as exc:
            # Match Python's interpreter semantics: None -> 0,
            # int -> int, anything else -> 1 (Python prints the value
            # to stderr and exits non-zero). Forcing 0 here would
            # silently mask failures in scripted profiling runs.
            code = exc.code
            if code is None:
                rc = 0
            elif isinstance(code, int):
                rc = code
            else:
                rc = 1
    finally:
        profiler.stop()
        sys.argv = saved_argv

    out_path.write_text(profiler.output_html(), encoding="utf-8")
    print(f"Profile written to {out_path}")

    if args.open_browser:
        import webbrowser

        webbrowser.open(out_path.resolve().as_uri())

    return rc
