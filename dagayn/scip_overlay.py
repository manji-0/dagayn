"""Call targets from SCIP indexers (``docs/plans/SCIP-CALL-RESOLUTION.md``).

``dagayn build --scip`` runs, for every project of the repository a
registered indexer covers, that indexer, then lets the graph settle its
``CALLS`` edges by the language's indexes together
(``GraphStore.apply_scip_overlays_json``), so a symbol one project
references and another defines is the definition's node. A project is a
directory holding the indexer's marker file (``Cargo.toml``,
``tsconfig.json``, ``go.mod``, ...). TypeScript's indexer ships with dagayn
(:mod:`dagayn.scip_typescript`, run as a child process); the others are
programs this machine has to provide. When an indexer is not installed, the
build prints a hint saying how to install it and the language keeps the
targets resolution gives; a failed run is a warning and does the same. A
project missing what its indexer ``wants`` (``node_modules``) is still
indexed, with a hint, and calls into what is missing stay unsettled.

Rust and TypeScript indexes are authoritative: their answer replaces the
extractor's. The others, not yet measured against dagayn's own resolution,
keep what the extractor resolved and settle the rest, before the inference
passes run. scip-java runs the project's build (`gradle clean ...` /
`mvn clean verify`), so it runs only with ``DAGAYN_SCIP_ALLOW_BUILD=1``.

``DAGAYN_SCIP_<LANGUAGE>`` (``DAGAYN_SCIP_RUST``, ``DAGAYN_SCIP_GO``, ...)
replaces an indexer's command; ``DAGAYN_SCIP_TIMEOUT`` bounds each run in
seconds (default 900).
"""

from __future__ import annotations

import fnmatch
import json
import logging
import os
import shlex
import shutil
import signal
import subprocess
import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from dagayn.child_env import child_env

logger = logging.getLogger(__name__)

_DEFAULT_TIMEOUT = 900
_SKIPPED_DIRS = {
    "node_modules",
    "vendor",
    "target",
    "dist",
    "build",
    "out",
    "bin",
    "obj",
    "__pycache__",
}

# Prints the distributions of the interpreter that runs it, in scip-python's
# `--environment` format.
_PYTHON_ENVIRONMENT_SCRIPT = """\
import importlib.metadata as md, json
print(json.dumps([
    {"name": d.metadata["Name"], "version": d.version,
     "files": [str(f) for f in (d.files or []) if str(f).endswith((".py", ".pyi"))]}
    for d in md.distributions()
]))
"""


@dataclass(frozen=True)
class Indexer:
    """A SCIP indexer: the projects it covers, how to run it, and how to
    install it.

    ``markers`` are file-name patterns (or paths, `build/compile_commands.json`)
    whose directory is a project;
    ``requires`` are paths that must exist next to the marker (installed
    dependencies); ``wants`` are paths whose absence (in the project or an
    ancestor up to the repository root) still lets the indexer run but costs
    answers, so it is a hint. ``topmost`` keeps only the outermost project (a
    Cargo workspace, not its member crates). ``commands`` are the default
    command prefixes, tried in order: the first whose program is on ``PATH``
    runs. ``install`` says how to get the program; ``None`` means dagayn
    ships the indexer, so ``commands[0]`` runs as given. ``arguments`` gives
    the rest of the command for a project and an output path; an indexer
    that always writes ``fixed_output`` in its working directory has it moved
    to the output path.
    """

    language: str
    markers: tuple[str, ...]
    commands: tuple[tuple[str, ...], ...]
    arguments: Callable[[Path, Path, Path], list[str]]
    install: str | None
    authoritative: bool = False
    requires: tuple[str, ...] = ()
    wants: tuple[str, ...] = ()
    topmost: bool = False
    fixed_output: str | None = None
    # Programs installed in the project rather than on PATH
    # (`vendor/bin/scip-php`).
    project_commands: tuple[str, ...] = ()
    # Why the indexer must be allowed to run (it builds the project).
    builds_project: str | None = None

    @property
    def env_var(self) -> str:
        return f"DAGAYN_SCIP_{self.language.upper()}"


def _rust_arguments(project: Path, output: Path, _: Path) -> list[str]:
    return ["scip", str(project), "--output", str(output)]


def _typescript_arguments(project: Path, output: Path, _: Path) -> list[str]:
    tsconfigs = sorted(p.name for p in project.glob("tsconfig*.json"))
    return ["index", "--output", str(output), *tsconfigs]


def parse_typescript_arguments(argv: list[str]) -> tuple[Path, list[str]]:
    """The inverse of :func:`_typescript_arguments`, for the bundled indexer:
    the output path and the tsconfigs (``tsconfig.json`` when none is named)."""
    if len(argv) < 3 or argv[:2] != ["index", "--output"]:
        raise ValueError(f"expected `index --output OUT [tsconfig...]`, got {argv}")
    return Path(argv[2]), argv[3:] or ["tsconfig.json"]


def _go_arguments(_: Path, output: Path, __: Path) -> list[str]:
    return ["--output", str(output)]


def _python_arguments(project: Path, output: Path, scratch: Path) -> list[str]:
    arguments = [
        "index",
        "--cwd",
        str(project),
        "--project-name",
        project.name,
        "--output",
        str(output),
    ]
    environment = _python_environment(project, scratch)
    if environment is not None:
        arguments += ["--environment", str(environment)]
    return arguments


def _java_arguments(_: Path, output: Path, __: Path) -> list[str]:
    return ["index", "--output", str(output)]


def _clang_arguments(project: Path, output: Path, _: Path) -> list[str]:
    compdb = next(
        p
        for p in (project / "compile_commands.json", project / "build" / "compile_commands.json")
        if p.is_file()
    )
    return [f"--compdb-path={compdb}", f"--index-output-path={output}"]


def _dotnet_arguments(project: Path, output: Path, _: Path) -> list[str]:
    return ["index", "--output", str(output), "--working-directory", str(project)]


def _ruby_arguments(project: Path, output: Path, _: Path) -> list[str]:
    arguments = ["--index-file", str(output)]
    # Without a Sorbet config, scip-ruby is told the directory to index.
    return arguments if (project / "sorbet" / "config").is_file() else [*arguments, "."]


def _dart_arguments(_: Path, output: Path, __: Path) -> list[str]:
    return ["./", "--output", str(output)]


def _php_arguments(_: Path, __: Path, ___: Path) -> list[str]:
    return []


def _r_arguments(project: Path, output: Path, _: Path) -> list[str]:
    return ["index", str(project), "-o", str(output)]


INDEXERS: tuple[Indexer, ...] = (
    Indexer(
        language="rust",
        markers=("Cargo.toml",),
        commands=(("rust-analyzer",),),
        arguments=_rust_arguments,
        install="rustup component add rust-analyzer",
        authoritative=True,
        topmost=True,
    ),
    Indexer(
        language="typescript",
        markers=("tsconfig.json",),
        commands=((sys.executable, "-P", "-m", "dagayn.scip_typescript"),),
        arguments=_typescript_arguments,
        install=None,
        authoritative=True,
        wants=("node_modules",),
    ),
    Indexer(
        language="go",
        markers=("go.mod",),
        commands=(("scip-go",),),
        arguments=_go_arguments,
        install="go install github.com/scip-code/scip-go/cmd/scip-go@latest",
    ),
    Indexer(
        language="python",
        markers=("pyproject.toml", "setup.py", "setup.cfg"),
        commands=(("scip-python",), ("npx", "-y", "@sourcegraph/scip-python")),
        arguments=_python_arguments,
        install="npm install -g @sourcegraph/scip-python (or install Node.js for npx)",
        topmost=True,
    ),
    Indexer(
        language="java",
        markers=(
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ),
        commands=(
            ("scip-java",),
            ("coursier", "launch", "org.scip-code:scip-java:0.13.1", "--"),
            ("cs", "launch", "org.scip-code:scip-java:0.13.1", "--"),
        ),
        arguments=_java_arguments,
        install=(
            "coursier bootstrap --standalone -o scip-java "
            "org.scip-code:scip-java:0.13.1 --main org.scip_code.scip_java.ScipJava"
        ),
        topmost=True,
        builds_project="scip-java runs the project's build with `clean` (Gradle / Maven)",
    ),
    Indexer(
        language="cpp",
        markers=("compile_commands.json", "build/compile_commands.json"),
        commands=(("scip-clang",),),
        arguments=_clang_arguments,
        install="download scip-clang from https://github.com/sourcegraph/scip-clang/releases",
        topmost=True,
    ),
    Indexer(
        language="csharp",
        markers=("*.sln", "*.slnx", "*.csproj", "*.vbproj"),
        commands=(("scip-dotnet",),),
        arguments=_dotnet_arguments,
        install="dotnet tool install --global scip-dotnet",
        topmost=True,
    ),
    Indexer(
        language="ruby",
        markers=("Gemfile",),
        commands=(("scip-ruby",),),
        arguments=_ruby_arguments,
        install="add `gem 'scip-ruby', require: false` to the Gemfile, or download it from https://github.com/sourcegraph/scip-ruby/releases",
        topmost=True,
        project_commands=("bin/scip-ruby",),
    ),
    Indexer(
        language="dart",
        markers=("pubspec.yaml",),
        requires=(".dart_tool/package_config.json",),
        commands=(("scip_dart",), ("dart", "pub", "global", "run", "scip_dart")),
        arguments=_dart_arguments,
        install="dart pub global activate scip_dart",
        topmost=True,
    ),
    Indexer(
        language="php",
        markers=("composer.json",),
        requires=("composer.lock", "vendor"),
        commands=(),
        project_commands=("vendor/bin/scip-php",),
        arguments=_php_arguments,
        install="composer require --dev davidrjenni/scip-php",
        topmost=True,
        fixed_output="index.scip",
    ),
    Indexer(
        language="r",
        markers=("DESCRIPTION",),
        requires=("R",),
        commands=(("scip-r",),),
        arguments=_r_arguments,
        install=(
            'uv tool install "scip-r[export] @ git+https://github.com/seandavi/scip-r" '
            '--with "tree-sitter-r @ git+https://github.com/r-lib/tree-sitter-r@v1.3.0"'
        ),
        topmost=True,
    ),
)


@dataclass
class ScipJob:
    """One indexer run: the language, its command, where it runs, the prefix
    that turns its document paths into the graph's, and whether its answer
    replaces resolution's. ``env`` is set for a bundled indexer (the dagayn
    to import); the others inherit the environment."""

    language: str
    command: list[str]
    cwd: Path
    prefix: str
    output: Path
    authoritative: bool = False
    fixed_output: str | None = None
    env: dict[str, str] | None = None


@dataclass
class ScipOverlayReport:
    """What the overlay did per index, what to install for the languages it
    could not index, and what failed."""

    runs: list[dict[str, Any]] = field(default_factory=list)
    hints: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)


def _timeout() -> int:
    try:
        return int(os.environ.get("DAGAYN_SCIP_TIMEOUT", _DEFAULT_TIMEOUT))
    except ValueError:
        return _DEFAULT_TIMEOUT


def _command(indexer: Indexer, project: Path) -> list[str] | None:
    """The indexer's command prefix: the override, the program the project
    installed, the bundled command, or the first default whose program is on
    ``PATH``."""
    override = os.environ.get(indexer.env_var)
    if override:
        return shlex.split(override)
    for local in indexer.project_commands:
        if (project / local).is_file():
            return [str(project / local)]
    if indexer.install is None:
        return list(indexer.commands[0])
    for command in indexer.commands:
        program = shutil.which(command[0])
        if program:
            return [program, *command[1:]]
    return None


def _projects(repo_root: Path, indexer: Indexer) -> list[Path]:
    """Directories holding one of the indexer's markers and what it requires."""
    projects: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(repo_root):
        dirnames[:] = sorted(
            d for d in dirnames if d not in _SKIPPED_DIRS and not d.startswith(".")
        )
        here = Path(dirpath)
        if indexer.topmost and any(here.is_relative_to(p) for p in projects):
            continue
        marked = any(
            (here / m).is_file() if "/" in m else any(fnmatch.fnmatch(n, m) for n in filenames)
            for m in indexer.markers
        )
        if marked and all((here / required).exists() for required in indexer.requires):
            projects.append(here)
    return projects


def _missing_wants(repo_root: Path, project: Path, indexer: Indexer) -> list[str]:
    """What the indexer wants that neither the project nor an ancestor up to
    the repository root has (a workspace hoists ``node_modules`` to its root)."""
    candidates = [p for p in (project, *project.parents) if p.is_relative_to(repo_root)]
    return [
        wanted
        for wanted in indexer.wants
        if not any((candidate / wanted).exists() for candidate in candidates)
    ]


def _python_environment(project: Path, scratch: Path) -> Path | None:
    """The distributions of the project's interpreter (its ``.venv``, the
    active virtualenv, or dagayn's own), written for ``--environment``: a uv
    venv has no ``pip`` for scip-python to ask."""
    candidates = [project / ".venv" / "bin" / "python"]
    if os.environ.get("VIRTUAL_ENV"):
        candidates.append(Path(os.environ["VIRTUAL_ENV"]) / "bin" / "python")
    candidates.append(Path(sys.executable))
    interpreter = next((c for c in candidates if c.is_file()), None)
    if interpreter is None:
        return None
    try:
        completed = subprocess.run(  # noqa: S603 - the project's own interpreter
            [str(interpreter), "-c", _PYTHON_ENVIRONMENT_SCRIPT],
            capture_output=True,
            text=True,
            timeout=120,
            check=True,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    path = scratch / f"python-environment-{abs(hash(str(project)))}.json"
    path.write_text(completed.stdout)
    return path


def plan_jobs(repo_root: Path, output_dir: Path) -> tuple[list[ScipJob], list[str]]:
    """The indexer runs this repository and machine allow, and hints for the
    indexers its projects need but this machine lacks."""
    jobs: list[ScipJob] = []
    hints: list[str] = []
    for indexer in INDEXERS:
        projects = _projects(repo_root, indexer)
        if not projects:
            continue
        if indexer.builds_project and os.environ.get("DAGAYN_SCIP_ALLOW_BUILD") != "1":
            hints.append(
                f"{indexer.language}: {indexer.builds_project}; set DAGAYN_SCIP_ALLOW_BUILD=1 "
                "to let `build --scip` run it (calls keep dagayn's own resolution)"
            )
            continue
        missing = False
        wanting: list[str] = []
        for number, project in enumerate(projects):
            command = _command(indexer, project)
            if command is None:
                missing = True
                continue
            relative = project.relative_to(repo_root).as_posix()
            if lacks := _missing_wants(repo_root, project, indexer):
                wanting.append(f"{relative} ({', '.join(lacks)})")
            output = output_dir / f"{indexer.language}-{number}.scip"
            jobs.append(
                ScipJob(
                    language=indexer.language,
                    command=[*command, *indexer.arguments(project, output, output_dir)],
                    cwd=project,
                    prefix="" if relative == "." else f"{relative}/",
                    output=output,
                    authoritative=indexer.authoritative,
                    fixed_output=indexer.fixed_output,
                    env=child_env() if indexer.install is None else None,
                )
            )
        if missing:
            hints.append(
                f"{indexer.language}: no SCIP indexer found; install it with "
                f"`{indexer.install}` (calls keep dagayn's own resolution)"
            )
        if wanting:
            hints.append(
                f"{indexer.language}: dependencies not installed in {', '.join(wanting)}; "
                "calls into packages keep dagayn's own resolution"
            )
    return jobs, hints


def run_scip_overlay(store: Any, repo_root: Path, output_dir: Path) -> ScipOverlayReport:
    """Index the repository with every available SCIP indexer and settle the
    graph's ``CALLS`` edges by the indexes."""
    report = ScipOverlayReport()
    output_dir.mkdir(parents=True, exist_ok=True)
    jobs, hints = plan_jobs(repo_root, output_dir)
    report.hints.extend(hints)
    indexed: dict[str, list[ScipJob]] = {}
    for job in jobs:
        label = _label(job)
        job.output.unlink(missing_ok=True)
        try:
            completed = subprocess.run(  # noqa: S603 - indexer command, not shell
                job.command,
                cwd=job.cwd,
                env=job.env,
                capture_output=True,
                text=True,
                timeout=_timeout(),
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as e:
            report.warnings.append(f"{label}: indexer did not run: {type(e).__name__}: {e}")
            continue
        if job.fixed_output is not None and (job.cwd / job.fixed_output).is_file():
            shutil.move(str(job.cwd / job.fixed_output), job.output)
        if completed.returncode != 0 or not job.output.is_file():
            tail = (completed.stderr or completed.stdout or "").strip().splitlines()[-3:]
            ended = (
                f"killed by {signal.Signals(-completed.returncode).name}"
                if completed.returncode < 0
                else f"exited {completed.returncode}"
            )
            report.warnings.append(f"{label}: indexer {ended}: {' | '.join(tail)}")
            continue
        indexed.setdefault(job.language, []).append(job)
    for language, done in indexed.items():
        try:
            raw = store.apply_scip_overlays_json(
                [(str(job.output), job.prefix, job.authoritative) for job in done],
                str(repo_root),
            )
        except (AttributeError, OSError, RuntimeError, TypeError, ValueError) as e:
            report.warnings.append(
                f"SCIP overlay ({language}): index not applied: {type(e).__name__}: {e}"
            )
            continue
        for job, stats in zip(done, json.loads(raw), strict=True):
            report.runs.append({"language": job.language, "prefix": job.prefix, **stats})
            logger.info("%s: %s", _label(job), stats)
    return report


def _label(job: ScipJob) -> str:
    return f"SCIP overlay ({job.language}{', ' + job.prefix if job.prefix else ''})"
