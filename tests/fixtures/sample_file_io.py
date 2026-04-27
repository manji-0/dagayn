"""Fixture for file-I/O bridge detection tests (Python)."""

from pathlib import Path


def read_config():
    with open("config.yaml") as f:
        return f.read()


def write_output():
    with open("output.json", "w") as f:
        f.write("{}")


def read_bytes():
    return Path("data/model.bin").read_bytes()


def write_report():
    Path("reports/summary.txt").write_text("done")


def read_pathlib():
    return Path("schema.json").read_text()


def write_pathlib_bytes():
    Path("cache/dump.bin").write_bytes(b"\x00")


def dynamic_open(path):
    # Dynamic path — should produce LOW confidence CROSS_ARTIFACT edge
    with open(path) as f:
        return f.read()
