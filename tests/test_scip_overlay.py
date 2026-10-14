"""The SCIP overlay orchestrator: which indexers run, and how failures are reported."""

from __future__ import annotations

import json
import shlex
import sys
from pathlib import Path

import pytest

from dagayn import scip_overlay
from dagayn.scip_overlay import INDEXERS, plan_jobs, run_scip_overlay


class _Store:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, bool]] = []

    def apply_scip_overlay_json(
        self, index_path: str, prefix: str, repo_root: str, authoritative: bool = True
    ) -> str:
        self.calls.append((Path(index_path).name, prefix, authoritative))
        return json.dumps({"documents": 1, "calls": 2, "confirmed": 2})


@pytest.fixture(autouse=True)
def _no_indexers(monkeypatch: pytest.MonkeyPatch) -> None:
    """No real indexer runs: every command comes from an override."""
    monkeypatch.setattr(scip_overlay.shutil, "which", lambda _name: None)
    for indexer in INDEXERS:
        monkeypatch.delenv(indexer.env_var, raising=False)


def _repo(tmp_path: Path) -> Path:
    (tmp_path / "Cargo.toml").write_text("[workspace]\n")
    (tmp_path / "crates" / "a").mkdir(parents=True)
    (tmp_path / "crates" / "a" / "Cargo.toml").write_text("[package]\n")
    web = tmp_path / "web"
    (web / "node_modules").mkdir(parents=True)
    (web / "tsconfig.json").write_text("{}")
    (web / "tsconfig.test.json").write_text("{}")
    # A tsconfig without node_modules is not indexed.
    (tmp_path / "docs").mkdir()
    (tmp_path / "docs" / "tsconfig.json").write_text("{}")
    (tmp_path / "svc").mkdir()
    (tmp_path / "svc" / "go.mod").write_text("module example.com/svc\n")
    return tmp_path


def _writer(tmp_path: Path) -> str:
    """A stub indexer: writes a file at the path after ``--output``."""
    script = tmp_path / "stub_indexer.py"
    script.write_text(
        "import sys\nargs = sys.argv[1:]\nopen(args[args.index('--output') + 1], 'wb').write(b'')\n"
    )
    return f"{shlex.quote(sys.executable)} {shlex.quote(str(script))}"


def test_jobs_follow_the_projects_and_the_overrides(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    monkeypatch.setenv("DAGAYN_SCIP_RUST", "ra")
    monkeypatch.setenv("DAGAYN_SCIP_TYPESCRIPT", "scip-ts")
    monkeypatch.setenv("DAGAYN_SCIP_GO", "scip-go")
    jobs, hints = plan_jobs(repo, tmp_path / "out")
    assert hints == []
    assert [(job.language, job.prefix, job.cwd, job.authoritative) for job in jobs] == [
        # The workspace, not its member crate.
        ("rust", "", repo, True),
        ("typescript", "web/", repo / "web", True),
        ("go", "svc/", repo / "svc", False),
    ]
    assert jobs[0].command[:2] == ["ra", "scip"]
    assert jobs[1].command[:3] == ["scip-ts", "index", "--output"]
    assert jobs[1].command[-2:] == ["tsconfig.json", "tsconfig.test.json"]
    assert jobs[2].command[:2] == ["scip-go", "--output"]


def test_a_missing_indexer_is_a_hint_not_a_failure(tmp_path: Path) -> None:
    repo = _repo(tmp_path)
    jobs, hints = plan_jobs(repo, tmp_path / "out")
    assert jobs == []
    assert [hint.split(":")[0] for hint in hints] == ["rust", "typescript", "go"]
    assert "go install github.com/scip-code/scip-go/cmd/scip-go@latest" in hints[2]


def test_indexes_are_applied_and_failures_reported(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    monkeypatch.setenv("DAGAYN_SCIP_RUST", _writer(tmp_path))
    monkeypatch.setenv("DAGAYN_SCIP_GO", _writer(tmp_path))
    monkeypatch.setenv(
        "DAGAYN_SCIP_TYPESCRIPT", f"{shlex.quote(sys.executable)} -c 'raise SystemExit(3)'"
    )
    store = _Store()
    report = run_scip_overlay(store, repo, tmp_path / "out")
    assert store.calls == [("rust-0.scip", "", True), ("go-0.scip", "svc/", False)]
    assert [run["language"] for run in report.runs] == ["rust", "go"]
    assert report.hints == []
    assert len(report.warnings) == 1
    assert "typescript" in report.warnings[0] and "exited 3" in report.warnings[0]


def test_a_store_without_the_overlay_is_a_warning(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "Cargo.toml").write_text("[workspace]\n")
    monkeypatch.setenv("DAGAYN_SCIP_RUST", _writer(tmp_path))
    report = run_scip_overlay(object(), tmp_path, tmp_path / "out")
    assert report.runs == []
    assert "index not applied" in report.warnings[0]


def _polyglot(tmp_path: Path) -> Path:
    (tmp_path / "app").mkdir()
    (tmp_path / "app" / "pom.xml").write_text("<project/>")
    (tmp_path / "native" / "build").mkdir(parents=True)
    (tmp_path / "native" / "build" / "compile_commands.json").write_text("[]")
    (tmp_path / "dotnet").mkdir()
    (tmp_path / "dotnet" / "App.sln").write_text("")
    (tmp_path / "rb").mkdir()
    (tmp_path / "rb" / "Gemfile").write_text("")
    (tmp_path / "flutter" / ".dart_tool").mkdir(parents=True)
    (tmp_path / "flutter" / "pubspec.yaml").write_text("")
    (tmp_path / "flutter" / ".dart_tool" / "package_config.json").write_text("{}")
    (tmp_path / "web" / "vendor" / "bin").mkdir(parents=True)
    (tmp_path / "web" / "composer.json").write_text("{}")
    (tmp_path / "web" / "composer.lock").write_text("{}")
    (tmp_path / "rpkg" / "R").mkdir(parents=True)
    (tmp_path / "rpkg" / "DESCRIPTION").write_text("Package: x\n")
    return tmp_path


def test_every_language_without_its_indexer_gets_a_hint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("DAGAYN_SCIP_ALLOW_BUILD", raising=False)
    jobs, hints = plan_jobs(_polyglot(tmp_path), tmp_path / "out")
    assert jobs == []
    by_language = {hint.split(":")[0]: hint for hint in hints}
    assert sorted(by_language) == ["cpp", "csharp", "dart", "java", "php", "r", "ruby"]
    # scip-java builds the project: it needs consent, not just installing.
    assert "DAGAYN_SCIP_ALLOW_BUILD=1" in by_language["java"]
    assert "composer require --dev davidrjenni/scip-php" in by_language["php"]


def test_indexers_found_in_the_project_or_on_path_run_there(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _polyglot(tmp_path)
    php = repo / "web" / "vendor" / "bin" / "scip-php"
    php.write_text("")
    monkeypatch.setenv("DAGAYN_SCIP_ALLOW_BUILD", "1")
    for language in ("JAVA", "CPP", "CSHARP", "RUBY", "DART", "R"):
        monkeypatch.setenv(f"DAGAYN_SCIP_{language}", language.lower())
    jobs, hints = plan_jobs(repo, tmp_path / "out")
    assert hints == []
    commands = {job.language: (job.command, job.cwd, job.fixed_output) for job in jobs}
    assert commands["java"][0][:2] == ["java", "index"]
    assert commands["cpp"][0][1] == (
        f"--compdb-path={repo / 'native' / 'build' / 'compile_commands.json'}"
    )
    assert commands["cpp"][1] == repo / "native"
    assert commands["csharp"][0][-2:] == ["--working-directory", str(repo / "dotnet")]
    # Without `sorbet/config`, scip-ruby is told the directory.
    assert commands["ruby"][0][-1] == "."
    assert commands["php"] == ([str(php)], repo / "web", "index.scip")
    assert commands["r"][0][:3] == ["r", "index", str(repo / "rpkg")]


def test_an_indexer_writing_a_fixed_file_has_it_moved(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _polyglot(tmp_path)
    php = repo / "web" / "vendor" / "bin" / "scip-php"
    php.write_text(f"#!{sys.executable}\nopen('index.scip', 'wb').write(b'')\n")
    php.chmod(0o755)
    store = _Store()
    report = run_scip_overlay(store, repo, tmp_path / "out")
    assert store.calls == [("php-0.scip", "web/", False)]
    assert not (repo / "web" / "index.scip").exists()
    assert {hint.split(":")[0] for hint in report.hints} >= {"java", "ruby"}


def _typescript_package(directory: Path, version: str) -> Path:
    package = directory / "node_modules" / "typescript"
    (package / "dist" / "api" / "sync").mkdir(parents=True)
    (package / "dist" / "api" / "sync" / "api.js").write_text("")
    (package / "package.json").write_text(json.dumps({"name": "typescript", "version": version}))
    return package


def _node_only(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        scip_overlay.shutil, "which", lambda name: "/usr/bin/node" if name == "node" else None
    )


def test_typescript_7_in_the_project_is_indexed_by_the_tsgo_resolver(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    package = _typescript_package(repo / "web", "7.0.2")
    _node_only(monkeypatch)
    monkeypatch.delenv("DAGAYN_TSGO", raising=False)
    monkeypatch.delenv("DAGAYN_TSGO_TYPESCRIPT", raising=False)
    jobs, _hints = plan_jobs(repo, tmp_path / "out")
    (job,) = [job for job in jobs if job.language == "typescript"]
    assert job.command[:4] == ["/usr/bin/node", str(scip_overlay._TSGO_SCRIPT), "index", "--output"]
    assert job.command[-2:] == ["tsconfig.json", "tsconfig.test.json"]
    assert job.env == {"DAGAYN_TSGO_TYPESCRIPT": str(package)}
    assert job.indexer == "tsgo"
    assert job.authoritative


def test_older_typescript_an_override_or_dagayn_tsgo_0_keep_scip_typescript(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    _node_only(monkeypatch)
    monkeypatch.delenv("DAGAYN_TSGO_TYPESCRIPT", raising=False)

    _typescript_package(repo / "web", "5.9.3")
    jobs, hints = plan_jobs(repo, tmp_path / "out")
    assert [job for job in jobs if job.language == "typescript"] == []
    assert any(hint.startswith("typescript: no SCIP indexer found") for hint in hints)

    newer = _typescript_package(tmp_path / "tsgo", "7.0.2")
    monkeypatch.setenv("DAGAYN_TSGO_TYPESCRIPT", str(newer))
    monkeypatch.setenv("DAGAYN_TSGO", "0")
    jobs, _hints = plan_jobs(repo, tmp_path / "out")
    assert [job for job in jobs if job.language == "typescript"] == []

    monkeypatch.delenv("DAGAYN_TSGO")
    monkeypatch.setenv("DAGAYN_SCIP_TYPESCRIPT", "scip-ts")
    jobs, _hints = plan_jobs(repo, tmp_path / "out")
    (job,) = [job for job in jobs if job.language == "typescript"]
    assert job.command[0] == "scip-ts"
    assert job.indexer is None


def test_the_tsgo_resolver_settles_calls_in_a_real_build(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Runs only where TypeScript 7 and Node.js are installed
    (``DAGAYN_TSGO_TYPESCRIPT``)."""
    import os
    import shutil
    import sqlite3

    monkeypatch.undo()  # the real `which`
    typescript = os.environ.get("DAGAYN_TSGO_TYPESCRIPT")
    if not typescript or shutil.which("node") is None:
        pytest.skip("needs DAGAYN_TSGO_TYPESCRIPT and node")
    from dagayn.tools.build import build_or_update_graph

    (tmp_path / ".git").mkdir()
    (tmp_path / "node_modules").mkdir()
    (tmp_path / "tsconfig.json").write_text('{"compilerOptions": {"strict": true}}')
    (tmp_path / "store.ts").write_text(
        "export class Store {\n  save(): void {}\n}\n"
        "export function make(): Store { return new Store(); }\n"
    )
    (tmp_path / "main.ts").write_text(
        "import { make } from './store';\n"
        "export function run(): void {\n  const s = make();\n  s.save();\n  [1].forEach(make);\n}\n"
    )
    result = build_or_update_graph(repo_root=str(tmp_path), full_rebuild=True, scip=True)
    (run,) = result["scip_overlay"]
    assert run["indexer"] == "tsgo"
    db = sqlite3.connect(tmp_path / ".dagayn" / "graph.db")
    rows = set(
        db.execute(
            "SELECT kind, target_qualified, line FROM edges WHERE file_path = 'main.ts' "
            "AND json_extract(extra, '$.resolved_by') = 'scip'"
        )
    )
    assert ("CALLS", "store.ts::Store.save", 4) in rows
    assert ("CALLS", "store.ts::make", 3) in rows
    assert ("REFERENCES", "store.ts::make", 5) in rows


def test_watch_keeps_typescript_calls_settled_by_a_resident_tsgo_resolver(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """After a `build --scip`, an edit re-parsed by an incremental update has
    its calls settled by the resident resolver. Runs only where TypeScript 7
    and Node.js are installed (``DAGAYN_TSGO_TYPESCRIPT``)."""
    import os
    import shutil
    import sqlite3

    monkeypatch.undo()  # the real `which`
    if not os.environ.get("DAGAYN_TSGO_TYPESCRIPT") or shutil.which("node") is None:
        pytest.skip("needs DAGAYN_TSGO_TYPESCRIPT and node")
    from dagayn.graph import GraphStore
    from dagayn.incremental_update_pipeline import incremental_update
    from dagayn.scip_overlay import ResidentTsgo
    from dagayn.tools.build import build_or_update_graph

    (tmp_path / ".git").mkdir()
    (tmp_path / "node_modules").mkdir()
    (tmp_path / "tsconfig.json").write_text('{"compilerOptions": {"strict": true}}')
    (tmp_path / "store.ts").write_text(
        "export class Store {\n  save(): void {}\n}\n"
        "export function make(): Store { return new Store(); }\n"
    )
    main = tmp_path / "main.ts"
    main.write_text("import { make } from './store';\nexport function run(): void {}\n")
    build_or_update_graph(repo_root=str(tmp_path), full_rebuild=True, scip=True)

    store = GraphStore(str(tmp_path / ".dagayn" / "graph.db"))
    resident = ResidentTsgo.for_store(store, tmp_path, tmp_path / ".dagayn" / "scip")
    assert resident is not None
    try:
        main.write_text(
            "import { make } from './store';\n"
            "export function run(): void {\n  const s = make();\n  s.save();\n}\n"
        )
        result = incremental_update(tmp_path, store, changed_files=["main.ts"])
        changed = list(result.changed_files or [])
        (run,) = resident.apply(store, changed, [*changed, *(result.dependent_files or [])])
        assert run["rewritten_to_node"] + run["confirmed"] >= 1
    finally:
        resident.close()
    db = sqlite3.connect(tmp_path / ".dagayn" / "graph.db")
    rows = set(
        db.execute(
            "SELECT target_qualified, line FROM edges WHERE file_path = 'main.ts' "
            "AND kind = 'CALLS' AND json_extract(extra, '$.resolved_by') = 'scip'"
        )
    )
    assert ("store.ts::Store.save", 4) in rows
