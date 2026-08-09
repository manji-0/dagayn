"""Wrapper side of a reportable subprocess CROSS_ARTIFACT bridge."""

import subprocess


def launch_native():
    """Invoke the native entry binary/script across an artifact boundary."""
    subprocess.run(["./native_entry.py", "--once"])


def main():
    launch_native()
