"""Allow running as: python -m dagayn"""

from importlib import import_module


def main() -> None:
    """Run the CLI entry point without a package-level dagayn -> cli import."""
    cli_main = import_module("dagayn.cli").main
    cli_main()


main()
