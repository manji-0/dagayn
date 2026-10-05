"""The Rust front end answers jj workspaces and SVN working copies as Python does.

A jj workspace is real (jj is on PATH where these run; skipped otherwise): a
colocated main checkout with a ``jj workspace add`` workspace under
``.worktrees/``, as ``track`` lays it out. SVN is not installed, so an
``svn`` stand-in on PATH plays back recorded command output; both sides run
the same ``svn`` binary, so one recording drives both.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import sys
import textwrap
from pathlib import Path
from typing import Any

import pytest
from test_mcp_frontend import DAGAYN, _call_both, _env, _session_both

pytestmark = pytest.mark.skipif(not DAGAYN.exists(), reason="dagayn console script not installed")

real_jj = pytest.mark.skipif(shutil.which("jj") is None, reason="jj is not installed")

APP = "def main():\n    return helper()\n\n\ndef helper():\n    pass\n"
TEST_APP = "from app import main\n\n\ndef test_main():\n    main()\n"
IDENTITY = {
    "GIT_AUTHOR_NAME": "t",
    "GIT_AUTHOR_EMAIL": "t@example.invalid",
    "GIT_COMMITTER_NAME": "t",
    "GIT_COMMITTER_EMAIL": "t@example.invalid",
}

#: Read tools whose freshness fields are the VCS-sensitive part.
READS: list[tuple[str, dict[str, Any]]] = [
    ("query_graph_tool", {"pattern": "callers_of", "target": "app.py::helper"}),
    ("query_graph_tool", {"pattern": "callees_of", "target": "main", "detail_level": "minimal"}),
    ("semantic_search_nodes_tool", {"query": "helper"}),
    ("traverse_graph_tool", {"query": "main", "depth": 2}),
]

REVIEWS: list[tuple[str, dict[str, Any]]] = [
    ("review_tool", {}),
    ("review_tool", {"mode": "changes", "base": "HEAD", "detail_level": "minimal"}),
    ("review_tool", {"mode": "changes", "base": "main"}),
    ("review_tool", {"mode": "changes", "changed_files": ["app.py"]}),
    ("review_tool", {"mode": "changes", "base": "HEAD~9"}),
    ("review_tool", {"mode": "impact"}),
    ("review_tool", {"mode": "impact", "base": "HEAD", "detail_level": "minimal"}),
    ("review_tool", {"mode": "context", "base": "HEAD~1"}),
    ("review_tool", {"mode": "context", "detail_level": "minimal"}),
    ("review_tool", {"mode": "affected_flows", "base": "HEAD"}),
    ("review_tool", {"mode": "affected_flows", "base": "bad ref"}),
]


def _run(argv: list[str], cwd: Path) -> None:
    subprocess.run(argv, cwd=cwd, env={**_env(), **IDENTITY}, check=True, capture_output=True)


def _jj(cwd: Path, *args: str) -> None:
    _run(["jj", "--no-pager", "--color=never", *args], cwd)


def _dagayn(command: str, repo: Path) -> None:
    _run([str(DAGAYN), command, "--repo", str(repo)], repo)


def _structured(results: list[dict[str, Any]]) -> list[Any]:
    return [result.get("structuredContent") for result in results]


@pytest.fixture
def jj_workspace(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """A ``track``-style jj workspace two commits deep, with a built graph."""
    config = tmp_path / "jj-config.toml"
    config.write_text('[user]\nname = "T"\nemail = "t@example.invalid"\n', encoding="utf-8")
    monkeypatch.setenv("JJ_CONFIG", str(config))
    main = tmp_path / "main"
    main.mkdir()
    git = ["git", "-c", "commit.gpgsign=false"]
    _run([*git, "init", "-q", "-b", "main"], main)
    (main / ".gitignore").write_text(".dagayn/\n", encoding="utf-8")
    (main / "app.py").write_text(APP, encoding="utf-8")
    _run([*git, "add", "-A"], main)
    _run([*git, "commit", "-q", "-m", "app"], main)
    (main / "test_app.py").write_text(TEST_APP, encoding="utf-8")
    _run([*git, "add", "-A"], main)
    _run([*git, "commit", "-q", "-m", "tests"], main)
    (main / ".git" / "info").mkdir(exist_ok=True)
    (main / ".git" / "info" / "exclude").write_text(".worktrees/\n", encoding="utf-8")
    _jj(main, "git", "init", "--colocate")
    workspace = main / ".worktrees" / "task"
    workspace.parent.mkdir()
    _jj(main, "workspace", "add", str(workspace), "-r", "main")
    _dagayn("build", workspace)
    return workspace


def _edit(workspace: Path) -> None:
    (workspace / "app.py").write_text(
        "def main():\n    return helper()\n\n\ndef helper():\n    return 1\n\n\n"
        "def auth_token():\n    return 2\n",
        encoding="utf-8",
    )


@real_jj
@pytest.mark.parametrize("change", ["clean", "dirty", "indexed", "committed", "updated"])
def test_a_jj_workspace_is_answered_in_rust_as_python_does(jj_workspace: Path, change: str) -> None:
    if change != "clean":
        _edit(jj_workspace)
    if change == "indexed":
        _dagayn("update", jj_workspace)
    if change in {"committed", "updated"}:
        _jj(jj_workspace, "commit", "-m", "edit")
    if change == "updated":
        _dagayn("update", jj_workspace)
    # A graph behind HEAD makes get_minimal_context queue a prepare, which is
    # Python's (and would rebuild the graph under the calls that follow).
    context = [] if change == "committed" else [("get_minimal_context_tool", {"task": "review"})]
    calls = [*context, *READS, *REVIEWS]
    rust, python, stderr = _session_both(jj_workspace, calls)
    assert _structured(rust) == _structured(python)
    assert stderr.count(" in Rust") == len(calls), stderr
    if context:
        expected = {
            "clean": ("commit_synced", "synced"),
            "dirty": ("worktree_behind", "dirty_worktree"),
            "indexed": ("worktree_ahead", "dirty_worktree"),
            "updated": ("commit_synced", "synced"),
        }[change]
        sync = rust[0]["structuredContent"]["sync"]
        assert sync == {"state": expected[0], "status": expected[1], "vcs": "jj"}
    reads = rust[len(context) : len(context) + len(READS)]
    # traverse_graph_tool carries no answerability.
    for result in reads[:3]:
        answerability = result["structuredContent"]["answerability"]
        codes = answerability["reason_codes"]
        assert ("graph_describes_another_commit" in codes) is (change == "committed"), codes
        assert ("uncommitted_changes_may_be_unindexed" in codes) is (
            change in {"dirty", "indexed"}
        ), codes
    reviews = _structured(rust[len(context) + len(READS) :])
    # HEAD~1 is rebased onto @-: the diff is @-'s own commit.
    last_commit = "app.py" if change in {"committed", "updated"} else "test_app.py"
    assert reviews[0]["change_file_sources"]["base_diff"] == [last_commit]
    if change in {"dirty", "indexed"}:
        assert reviews[4]["diff_parse_status"] == "base_unresolved"
    else:
        assert reviews[4]["summary"] == "No changed files detected."
    if change in {"dirty", "indexed"}:
        sources = reviews[0]["change_file_sources"]
        assert sources["unstaged"] == ["app.py"]
        assert sources["staged"] == []
        # The edit to helper; auth_token only once an update indexed it.
        names = {f["name"] for f in reviews[2]["changed_functions"]}
        assert ("auth_token" in names) is (change == "indexed"), names
        assert "helper" in names


def test_a_jj_workspace_is_auto_detected_in_rust_as_python_does(jj_workspace: Path) -> None:
    """From inside a workspace with no --repo or repo_root, the walk stops at
    the workspace (not the main checkout above it), as find_repo_root does."""
    inside = jj_workspace / "sub"
    inside.mkdir()
    arguments = {"pattern": "callers_of", "target": "app.py::helper"}
    rust, python, stderr = _call_both(None, "query_graph_tool", arguments, cwd=inside)
    assert "answered query_graph_tool in Rust" in stderr
    assert rust["structuredContent"] == python["structuredContent"]
    repo = rust["structuredContent"]["_repo"]
    assert repo == {**repo, "repo_root": str(jj_workspace.resolve()), "source": "auto"}


def _make_stale(workspace: Path) -> None:
    """Rewrite the workspace's commit from the main workspace, as a rebase would."""
    main = workspace.parent.parent
    (main / "moved.py").write_text("MOVED = 1\n", encoding="utf-8")
    _jj(main, "commit", "-m", "main moves")
    _jj(main, "rebase", "-s", "task@", "-d", "@-")


@real_jj
def test_a_stale_jj_workspace_leaves_its_review_to_python(jj_workspace: Path) -> None:
    """jj cannot read a stale working copy: reads carry no freshness, the
    change listing's error is Python's to explain, and explicit files meet
    the diff base jj cannot resolve."""
    _make_stale(jj_workspace)
    calls = [*READS, ("review_tool", {}), ("review_tool", {"changed_files": ["app.py"]})]
    rust, python, stderr = _session_both(jj_workspace, calls)
    assert _structured(rust) == _structured(python)
    assert stderr.count(" in Rust") == len(READS) + 1
    assert "review_tool in Rust\n" in stderr.split("answered traverse_graph_tool in Rust")[1]
    for result in rust[:3]:
        codes = result["structuredContent"]["answerability"]["reason_codes"]
        assert "graph_describes_another_commit" not in codes, codes
        assert "uncommitted_changes_may_be_unindexed" not in codes, codes
    assert rust[len(READS)]["structuredContent"]["status"] == "error"
    assert rust[len(READS) + 1]["structuredContent"]["diff_parse_status"] == "base_unresolved"


# --------------------------------------------------------------------------
# SVN
# --------------------------------------------------------------------------

SVN_INFO = textwrap.dedent(
    """\
    Path: .
    Working Copy Root Path: {root}
    URL: https://svn.example.invalid/repo/branches/feature-x
    Relative URL: ^/branches/feature-x
    Repository Root: https://svn.example.invalid/repo
    Revision: 42
    Node Kind: directory
    Schedule: normal
    """
)

SVN_STATUS = "M       app.py\n?       scratch.txt\nA  +    test_app.py\n"

SVN_DIFF = textwrap.dedent(
    """\
    Index: app.py
    ===================================================================
    --- app.py\t(revision 42)
    +++ app.py\t(working copy)
    @@ -5,2 +5,2 @@
     def helper():
    -    pass
    +    return 1
    """
)

SVN_SUMMARIZE = "M       app.py\nD       gone.py\n"

#: A recording-playback ``svn``: each subcommand prints its recorded file.
SVN_SHIM = textwrap.dedent(
    """\
    import os, sys
    from pathlib import Path

    record = Path(os.environ["FAKE_SVN_DIR"])
    args = sys.argv[1:]
    with (record / "calls.log").open("a", encoding="utf-8") as log:
        log.write(" ".join(args) + "\\n")
    name = {
        "info": "info_revision.txt" if "--show-item" in args else "info.txt",
        "status": "status.txt",
        "diff": ("summarize.txt" if "--summarize" in args
                 else "diff_range.txt" if "-r" in args else "diff.txt"),
    }.get(args[0] if args else "")
    if name is None or not (record / name).exists():
        sys.stderr.write("svn: E155007: not recorded\\n")
        sys.exit(1)
    sys.stdout.buffer.write((record / name).read_bytes())
    """
)


def _svn_bin(tmp_path: Path) -> Path:
    bin_dir = tmp_path / "svn-bin"
    bin_dir.mkdir()
    shim = bin_dir / "svn"
    shim.write_text(f"#!{sys.executable}\n{SVN_SHIM}", encoding="utf-8")
    shim.chmod(shim.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return bin_dir


@pytest.fixture
def svn_copy(tmp_path: Path) -> tuple[Path, dict[str, str]]:
    """An SVN working copy with a built graph, and the env that puts the
    recorded ``svn`` on PATH."""
    root = tmp_path / "wc"
    (root / ".svn").mkdir(parents=True)
    (root / "app.py").write_text(APP, encoding="utf-8")
    (root / "test_app.py").write_text(TEST_APP, encoding="utf-8")
    record = tmp_path / "svn-record"
    record.mkdir()
    (record / "info.txt").write_text(SVN_INFO.format(root=root), encoding="utf-8")
    (record / "info_revision.txt").write_text("42\n", encoding="utf-8")
    env = {
        "PATH": f"{_svn_bin(tmp_path)}{os.pathsep}{os.environ.get('PATH', '')}",
        "FAKE_SVN_DIR": str(record),
    }
    subprocess.run(
        [str(DAGAYN), "build", "--repo", str(root)],
        env={**_env(), **env},
        check=True,
        capture_output=True,
    )
    return root, env


SVN_REVIEWS: list[tuple[str, dict[str, Any]]] = [
    ("review_tool", {}),
    ("review_tool", {"mode": "changes", "detail_level": "minimal"}),
    ("review_tool", {"mode": "changes", "base": "r40:HEAD"}),
    ("review_tool", {"mode": "changes", "changed_files": ["app.py"]}),
    ("review_tool", {"mode": "impact"}),
    ("review_tool", {"mode": "impact", "base": "r40:HEAD", "detail_level": "minimal"}),
    ("review_tool", {"mode": "context"}),
    ("review_tool", {"mode": "context", "base": "40", "detail_level": "minimal"}),
    ("review_tool", {"mode": "affected_flows"}),
    ("review_tool", {"mode": "affected_flows", "base": "bad ref"}),
]


@pytest.mark.parametrize("recorded", ["changes", "clean", "missing"])
def test_an_svn_working_copy_is_answered_in_rust_as_python_does(
    svn_copy: tuple[Path, dict[str, str]], recorded: str
) -> None:
    root, env = svn_copy
    record = Path(env["FAKE_SVN_DIR"])
    if recorded == "changes":
        (record / "status.txt").write_text(SVN_STATUS, encoding="utf-8")
        (record / "diff.txt").write_text(SVN_DIFF, encoding="utf-8")
        (record / "diff_range.txt").write_text(SVN_DIFF, encoding="utf-8")
        (record / "summarize.txt").write_text(SVN_SUMMARIZE, encoding="utf-8")
    elif recorded == "clean":
        (record / "status.txt").write_text("?       scratch.txt\n", encoding="utf-8")
    else:
        # No svn at all: Python reports no changes, and no ranges.
        env = {"PATH": os.environ.get("PATH", "")}
    context = ("get_minimal_context_tool", {"task": "explore"})
    calls = [context, *READS, *SVN_REVIEWS]
    rust, python, stderr = _session_both(root, calls, **env)
    assert _structured(rust) == _structured(python)
    assert stderr.count(" in Rust") == len(calls), stderr
    assert rust[0]["structuredContent"]["sync"] == {
        "state": "commit_synced",
        "status": "synced",
        "vcs": "svn",
    }
    first = rust[1 + len(READS)]["structuredContent"]
    if recorded == "changes":
        assert first["changed_files"] == ["app.py", "test_app.py"]
        assert first["change_file_sources"]["unstaged"] == ["app.py", "test_app.py"]
        ranged = rust[1 + len(READS) + 2]["structuredContent"]
        assert ranged["changed_files"] == ["app.py", "gone.py"]
        calls_log = (record / "calls.log").read_text(encoding="utf-8").splitlines()
        assert "diff --summarize --non-interactive -r r40:HEAD" in calls_log
        assert "diff --non-interactive -r r40:HEAD" in calls_log
    else:
        assert first["summary"] == "No changed files detected."
