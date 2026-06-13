"""Git helpers for reproducible evaluation checkouts."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any


def run_git(args: list[str], cwd: Path | str | None = None) -> subprocess.CompletedProcess[str]:
    """Run git with checked errors and readable diagnostics."""
    cmd = ["git", *args]
    try:
        return subprocess.run(
            cmd,
            cwd=str(cwd) if cwd is not None else None,
            check=True,
            text=True,
            capture_output=True,
        )
    except subprocess.CalledProcessError as exc:
        stderr = (exc.stderr or "").strip()
        stdout = (exc.stdout or "").strip()
        detail = stderr or stdout or f"exit code {exc.returncode}"
        location = f" in {cwd}" if cwd is not None else ""
        raise RuntimeError(f"{' '.join(cmd)} failed{location}: {detail}") from exc


def resolve_ref(repo_path: Path, ref: str) -> str:
    """Resolve *ref* to an immutable commit SHA."""
    return run_git(["rev-parse", "--verify", f"{ref}^{{commit}}"], cwd=repo_path).stdout.strip()


def _remote_default_ref(repo_path: Path) -> str:
    symbolic = run_git(["symbolic-ref", "refs/remotes/origin/HEAD"], cwd=repo_path).stdout.strip()
    if symbolic.startswith("refs/remotes/origin/"):
        return symbolic.removeprefix("refs/remotes/")
    return "origin/HEAD"


def checkout_config_ref(config: dict[str, Any], repo_path: Path) -> str:
    """Checkout the configured ref and return its resolved commit SHA."""
    ref = str(config.get("commit", "HEAD"))
    moving_ref = ref == "HEAD"
    checkout_ref = _remote_default_ref(repo_path) if moving_ref else ref
    resolved = resolve_ref(repo_path, checkout_ref)
    run_git(["checkout", "--force", "--detach", resolved], cwd=repo_path)
    config["resolved_commit"] = resolved
    config["moving_ref"] = moving_ref
    if moving_ref and not config.get("allow_moving_ref"):
        config["moving_ref_warning"] = (
            "commit HEAD resolved from remote default branch; set allow_moving_ref: true "
            "or pin an immutable commit for reproducible evals"
        )
    return resolved


def ensure_parent_available(repo_path: Path, sha: str) -> str:
    """Resolve ``sha~1`` or raise a RuntimeError when history is unavailable."""
    return resolve_ref(repo_path, f"{sha}~1")
