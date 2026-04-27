"""Test-function and test-file detection heuristics."""

from __future__ import annotations

import re

_TEST_PATTERNS = [
    re.compile(r"^test_"),
    re.compile(r"^Test"),
    re.compile(r"_test$"),
    re.compile(r"\.test\."),
    re.compile(r"\.spec\."),
    re.compile(r"_spec$"),
]

_TEST_FILE_PATTERNS = [
    re.compile(r"test_.*\.py$"),
    re.compile(r".*_test\.py$"),
    re.compile(r".*\.test\.[jt]sx?$"),
    re.compile(r".*\.spec\.[jt]sx?$"),
    re.compile(r".*_test\.go$"),
    re.compile(r"tests?/"),
    re.compile(r".*_test\.dart$"),
    re.compile(r"test[_-].*\.[rR]$"),
    re.compile(r"tests/testthat/"),
    re.compile(r".*Test\.kt$"),
    re.compile(r".*Test\.java$"),
    re.compile(r".*_test\.resi?$"),
    re.compile(r".*\.test\.resi?$"),
]

_TEST_RUNNER_NAMES = frozenset(
    {
        "describe",
        "it",
        "test",
        "beforeEach",
        "afterEach",
        "beforeAll",
        "afterAll",
    }
)

# Annotations/decorators that mark test methods (JUnit, TestNG, etc.)
_TEST_ANNOTATIONS = frozenset(
    {
        "Test",
        "ParameterizedTest",
        "RepeatedTest",
        "TestFactory",
        "org.junit.Test",
        "org.junit.jupiter.api.Test",
    }
)


def is_test_file(path: str) -> bool:
    return any(p.search(path) for p in _TEST_FILE_PATTERNS)


def is_test_function(
    name: str,
    file_path: str,
    decorators: tuple[str, ...] = (),
) -> bool:
    """A function is a test if its name matches test patterns, it lives
    in a test file and has a test-runner name, or it has a @Test annotation.
    """
    if any(p.search(name) for p in _TEST_PATTERNS):
        return True
    if is_test_file(file_path) and name in _TEST_RUNNER_NAMES:
        return True
    if decorators and any(d in _TEST_ANNOTATIONS for d in decorators):
        return True
    return False
