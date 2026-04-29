"""Backward-compatibility re-export from parser._base.test_detection."""

from ._base.test_detection import (
    _TEST_ANNOTATIONS,
    _TEST_FILE_PATTERNS,
    _TEST_PATTERNS,
    _TEST_RUNNER_NAMES,
    is_test_file,
    is_test_function,
)

__all__ = [
    "_TEST_ANNOTATIONS",
    "_TEST_FILE_PATTERNS",
    "_TEST_PATTERNS",
    "_TEST_RUNNER_NAMES",
    "is_test_file",
    "is_test_function",
]
