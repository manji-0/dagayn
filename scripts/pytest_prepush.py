#!/usr/bin/env python3
"""Run pytest for files in a push, not the whole suite.

The pre-push hook used to invoke ``uv run pytest`` with no arguments, so every
``git push`` waited on the full suite. CI still does that. Locally we only need
the tests that can see the files being pushed.
"""

from __future__ import annotations

import subprocess
import sys
from collections.abc import Iterable, Sequence
from pathlib import Path

_GENERIC_STEMS = frozenset(
    {
        "base",
        "common",
        "core",
        "helpers",
        "lib",
        "main",
        "types",
        "utils",
    }
)
_BENCHMARK_TESTS = frozenset({"test_embedding_material_model_benchmark.py"})
_RUST_SMOKE = Path("tests/test_rust_backend_parity.py")
_CODE_PREFIXES = ("dagayn/", "crates/", "tests/", "scripts/")


def select_test_files(
    changed: Sequence[str],
    *,
    tests_dir: Path,
    repo_root: Path,
) -> list[Path]:
    """Return test paths that should run for *changed* repo-relative files."""
    rels = [_relative(path, repo_root) for path in changed]
    test_files = sorted(tests_dir.glob("test_*.py"))
    haystacks = {tf: tf.read_text(encoding="utf-8", errors="replace") for tf in test_files}
    selected: set[Path] = set()
    code_changed = False

    for rel in rels:
        posix = rel.as_posix()
        if posix.startswith("tests/fixtures/"):
            continue
        if _is_test_module(rel):
            selected.add(rel)
            continue
        if not posix.startswith(_CODE_PREFIXES) and posix not in {
            "pyproject.toml",
            "hatch_build.py",
        }:
            continue
        code_changed = True
        if posix.startswith("crates/") and (repo_root / _RUST_SMOKE).is_file():
            selected.add(_RUST_SMOKE)
        for candidate in _name_candidates(rel):
            path = Path("tests") / candidate
            if (repo_root / path).is_file() or (tests_dir / candidate).is_file():
                selected.add(path)
        needles = _import_needles(rel)
        for tf, text in haystacks.items():
            if any(needle in text for needle in needles):
                selected.add(Path("tests") / tf.name)

    changed_test_names = {path.name for path in rels if _is_test_module(path)}
    selected = {
        path
        for path in selected
        if path.name not in _BENCHMARK_TESTS or path.name in changed_test_names
    }
    if code_changed and not selected:
        smoke = Path("tests/test_tools.py")
        if (repo_root / smoke).is_file():
            selected.add(smoke)
    return sorted(selected)


def _relative(path: str, repo_root: Path) -> Path:
    raw = Path(path)
    if raw.is_absolute():
        try:
            return raw.resolve().relative_to(repo_root.resolve())
        except ValueError:
            return raw
    return raw


def _is_test_module(rel: Path) -> bool:
    posix = rel.as_posix()
    return (
        posix.startswith("tests/")
        and rel.suffix == ".py"
        and rel.name.startswith("test_")
        and "fixtures" not in rel.parts
    )


def _name_candidates(rel: Path) -> list[str]:
    parts = list(rel.with_suffix("").parts)
    if parts and parts[0] in {"dagayn", "crates", "scripts"}:
        parts = parts[1:]
    if parts and parts[0] in {"dagayn-graph", "dagayn-py", "src"}:
        parts = parts[1:]
    if not parts:
        return []
    names = [f"test_{parts[-1]}.py"]
    if len(parts) >= 2:
        names.append(f"test_{parts[-2]}_{parts[-1]}.py")
        names.append(f"test_{parts[0]}.py")
        names.append(f"test_{parts[0]}_{parts[-1]}.py")
    if len(parts) >= 3:
        names.append(f"test_{parts[-3]}_{parts[-1]}.py")
    return list(dict.fromkeys(names))


def _import_needles(rel: Path) -> list[str]:
    parts = rel.with_suffix("").parts
    needles: list[str] = [rel.as_posix()]
    if parts and parts[0] == "dagayn":
        needles.append(".".join(parts))
    stem = rel.stem
    if stem not in _GENERIC_STEMS and not stem.startswith("_"):
        needles.append(f"from .{stem} ")
        needles.append(f"from ..{stem} ")
        needles.append(f"from ...{stem} ")
        needles.append(f"import {stem}")
    return needles


def _git_changed_files(repo_root: Path) -> list[str]:
    for spec in ("@{upstream}...HEAD", "origin/main...HEAD", "HEAD~1"):
        result = subprocess.run(
            ["git", "diff", "--name-only", spec],
            cwd=repo_root,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode == 0:
            return [line.strip() for line in result.stdout.splitlines() if line.strip()]
    return []


def main(argv: Sequence[str] | None = None) -> int:
    repo_root = Path(__file__).resolve().parents[1]
    tests_dir = repo_root / "tests"
    changed: Iterable[str] = sys.argv[1:] if argv is None else argv
    if not changed:
        changed = _git_changed_files(repo_root)
    selected = select_test_files(list(changed), tests_dir=tests_dir, repo_root=repo_root)
    if not selected:
        print("pytest-prepush: no related tests")
        return 0
    cmd = [
        sys.executable,
        "-m",
        "pytest",
        "--tb=short",
        "-q",
        *(str(repo_root / path) for path in selected),
    ]
    print("pytest-prepush:", " ".join(path.as_posix() for path in selected))
    return subprocess.call(cmd, cwd=repo_root)


if __name__ == "__main__":
    raise SystemExit(main())
