"""Call targets from SCIP indexers (``docs/plans/SCIP-CALL-RESOLUTION.md``).

``dagayn build --scip`` runs the indexers this machine has for the
repository's languages, then lets the graph settle its ``CALLS`` edges by
their answers (``GraphStore.apply_scip_overlay_json``). Rust is indexed by
``rust-analyzer scip`` at a Cargo workspace root, TypeScript by
``scip-typescript`` in each directory holding a ``tsconfig.json`` and its
``node_modules``. A missing indexer, a failed run, or a project without its
prerequisites skips that language with a warning; the graph keeps what
resolution gave it.

``DAGAYN_SCIP_RUST`` and ``DAGAYN_SCIP_TYPESCRIPT`` replace the indexer
commands (``rust-analyzer`` and ``npx -y @sourcegraph/scip-typescript``);
``DAGAYN_SCIP_TIMEOUT`` bounds each run in seconds (default 900).
"""

from __future__ import annotations

import json
import logging
import os
import shlex
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

_DEFAULT_TIMEOUT = 900
_SKIPPED_DIRS = {"node_modules", ".git", ".dagayn", "target", "dist", "build", "out"}


@dataclass
class ScipJob:
    """One indexer run: the language, its command, where it runs, and the
    prefix that turns its document paths into the graph's."""

    language: str
    command: list[str]
    cwd: Path
    prefix: str
    output: Path


@dataclass
class ScipOverlayReport:
    """What the overlay did per index, and why any language was skipped."""

    runs: list[dict[str, Any]] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)


def _timeout() -> int:
    try:
        return int(os.environ.get("DAGAYN_SCIP_TIMEOUT", _DEFAULT_TIMEOUT))
    except ValueError:
        return _DEFAULT_TIMEOUT


def _command(env_var: str, default: list[str] | None) -> list[str] | None:
    override = os.environ.get(env_var)
    if override:
        return shlex.split(override)
    return default


def _typescript_projects(repo_root: Path) -> list[Path]:
    """Directories with a ``tsconfig.json`` and installed ``node_modules``."""
    projects = []
    for dirpath, dirnames, filenames in os.walk(repo_root):
        dirnames[:] = [d for d in dirnames if d not in _SKIPPED_DIRS and not d.startswith(".")]
        here = Path(dirpath)
        if "tsconfig.json" in filenames and (here / "node_modules").is_dir():
            projects.append(here)
    return projects


def plan_jobs(repo_root: Path, output_dir: Path) -> tuple[list[ScipJob], list[str]]:
    """The indexer runs this repository and machine allow, and why others
    were skipped."""
    jobs: list[ScipJob] = []
    skipped: list[str] = []
    if (repo_root / "Cargo.toml").is_file():
        analyzer = shutil.which("rust-analyzer")
        command = _command("DAGAYN_SCIP_RUST", [analyzer] if analyzer else None)
        if command is None:
            skipped.append(
                "SCIP overlay: rust-analyzer not found; Rust calls keep resolution's targets"
            )
        else:
            output = output_dir / "rust.scip"
            jobs.append(
                ScipJob(
                    language="rust",
                    command=[*command, "scip", str(repo_root), "--output", str(output)],
                    cwd=repo_root,
                    prefix="",
                    output=output,
                )
            )
    projects = _typescript_projects(repo_root)
    if projects:
        npx = shutil.which("npx")
        default = [npx, "-y", "@sourcegraph/scip-typescript"] if npx else None
        command = _command("DAGAYN_SCIP_TYPESCRIPT", default)
        if command is None:
            skipped.append(
                "SCIP overlay: npx not found; TypeScript calls keep resolution's targets"
            )
        else:
            for number, project in enumerate(projects):
                relative = project.relative_to(repo_root).as_posix()
                prefix = "" if relative == "." else f"{relative}/"
                tsconfigs = sorted(p.name for p in project.glob("tsconfig*.json"))
                output = output_dir / f"typescript-{number}.scip"
                jobs.append(
                    ScipJob(
                        language="typescript",
                        command=[*command, "index", "--output", str(output), *tsconfigs],
                        cwd=project,
                        prefix=prefix,
                        output=output,
                    )
                )
    return jobs, skipped


def run_scip_overlay(store: Any, repo_root: Path, output_dir: Path) -> ScipOverlayReport:
    """Index the repository with every available SCIP indexer and settle the
    graph's ``CALLS`` edges by the indexes."""
    report = ScipOverlayReport()
    output_dir.mkdir(parents=True, exist_ok=True)
    jobs, skipped = plan_jobs(repo_root, output_dir)
    report.warnings.extend(skipped)
    for job in jobs:
        label = f"SCIP overlay ({job.language}{', ' + job.prefix if job.prefix else ''})"
        job.output.unlink(missing_ok=True)
        try:
            completed = subprocess.run(  # noqa: S603 - indexer command, not shell
                job.command,
                cwd=job.cwd,
                capture_output=True,
                text=True,
                timeout=_timeout(),
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as e:
            report.warnings.append(f"{label}: indexer did not run: {type(e).__name__}: {e}")
            continue
        if completed.returncode != 0 or not job.output.is_file():
            tail = (completed.stderr or completed.stdout or "").strip().splitlines()[-3:]
            report.warnings.append(
                f"{label}: indexer exited {completed.returncode}: {' | '.join(tail)}"
            )
            continue
        try:
            raw = store.apply_scip_overlay_json(str(job.output), job.prefix, str(repo_root))
        except (AttributeError, OSError, RuntimeError, ValueError) as e:
            report.warnings.append(f"{label}: index not applied: {type(e).__name__}: {e}")
            continue
        stats = json.loads(raw)
        report.runs.append({"language": job.language, "prefix": job.prefix, **stats})
        logger.info("%s: %s", label, stats)
    return report
