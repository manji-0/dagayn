"""Tests for CLI helpers."""

import logging
from importlib.metadata import PackageNotFoundError

from dagayn import cli


def test_get_version_logs_and_falls_back_to_dev(monkeypatch, caplog):
    def _raise_package_not_found(_dist_name: str) -> str:
        raise PackageNotFoundError("dagayn")

    monkeypatch.setattr(cli, "pkg_version", _raise_package_not_found)

    with caplog.at_level(logging.DEBUG, logger="dagayn.cli"):
        version = cli._get_version()

    assert version == "dev"
    assert "Package metadata unavailable" in caplog.text
