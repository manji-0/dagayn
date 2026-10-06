"""VCS edge cases of :mod:`dagayn.incremental_files`.

SVN working copies are driven through a fake ``svn`` executable that prints
the output format of the real client: no ``svn`` binary or server is needed,
and the parsing rules (status columns, ``--summarize`` records, branch
extraction from the URL) are pinned exactly. The git cases use real
repositories and a ``PATH`` without git to pin graceful degradation.

Kept apart from ``test_incremental.py``, which mostly covers the build and
update pipelines.
"""

from __future__ import annotations

import os
import shlex
import sys
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.incremental_files import (
    _git_branch_info,
    _relativize_parsed_entities,
    _store_vcs_metadata,
    _svn_revision_info,
    collect_all_files,
    detect_vcs,
    ensure_repo_gitignore_excludes_crg,
    find_repo_root,
    get_changed_file_sources,
    get_vcs_indexable_files,
    resolve_commit_sha,
)
from dagayn.parser import EdgeInfo, NodeInfo

posix_only = pytest.mark.skipif(sys.platform == "win32", reason="fake svn is a POSIX shell script")

_SVN_INFO = """\
Path: .
Working Copy Root Path: {root}
URL: {url}
Relative URL: ^/branches/feature-x
Repository Root: https://svn.example.com/repo
Revision: 1234
Node Kind: directory
"""

# ``svn status``: seven status columns, a space, then the path at column 8.
_SVN_STATUS = """\
M       src/app.py
A  +    src/new.py
D       old/gone.py
?       scratch.py
!       vanished.py
 M      props_only.py
R       src/replaced.py
C       src/conflicted.py

--- Changelist 'review':
M       src/in_changelist.py
"""

_SVN_SUMMARIZE = """\
M       src/app.py
A       src/new.py
D       old/gone.py
 M      props_only.py
"""

_SVN_LIST = """\
src/
src/app.py
README.md
vendor/
vendor/lib.py
deleted.py
link.py
blob.py
"""


def _write_fake_svn(bin_dir: Path, *, url: str, fail_diff: bool = False) -> Path:
    """Install an ``svn`` script that answers like the real client and logs argv."""
    bin_dir.mkdir(parents=True, exist_ok=True)
    log = bin_dir / "svn-calls.log"
    outputs = {
        "info": _SVN_INFO.format(root=bin_dir, url=url),
        "status": _SVN_STATUS,
        "diff": _SVN_SUMMARIZE,
        "list": _SVN_LIST,
    }
    for name, text in outputs.items():
        (bin_dir / f"{name}.out").write_text(text, encoding="utf-8")
    diff_branch = (
        'echo "svn: E160006: No such revision" >&2; exit 1'
        if fail_diff
        else f"cat {shlex.quote(str(bin_dir / 'diff.out'))}"
    )
    script = f"""#!/bin/sh
echo "$*" >> {shlex.quote(str(log))}
case "$1" in
  info) cat {shlex.quote(str(bin_dir / "info.out"))} ;;
  status) cat {shlex.quote(str(bin_dir / "status.out"))} ;;
  diff) {diff_branch} ;;
  list) cat {shlex.quote(str(bin_dir / "list.out"))} ;;
  *) echo "svn: unknown command $1" >&2; exit 1 ;;
esac
"""
    svn = bin_dir / "svn"
    svn.write_text(script, encoding="utf-8")
    svn.chmod(0o755)
    return log


def _prepend_path(monkeypatch: pytest.MonkeyPatch, directory: Path) -> None:
    monkeypatch.setenv("PATH", f"{directory}{os.pathsep}{os.environ.get('PATH', '')}")


def _svn_calls(log: Path) -> list[str]:
    if not log.exists():
        return []
    return log.read_text(encoding="utf-8").splitlines()


@pytest.fixture()
def svn_wc(tmp_path: Path) -> Path:
    """An SVN 1.7+ working copy: a single ``.svn`` at its root."""
    wc = tmp_path / "wc"
    (wc / ".svn").mkdir(parents=True)
    return wc


@posix_only
class TestSvnWorkingCopy:
    def test_old_per_directory_svn_resolves_to_the_topmost_copy(self, tmp_path):
        """Pre-1.7 SVN puts ``.svn`` in every directory; the root is the topmost."""
        wc = tmp_path / "old-wc"
        deep = wc / "pkg" / "mod"
        for directory in (wc, wc / "pkg", deep):
            (directory / ".svn").mkdir(parents=True)

        assert find_repo_root(deep) == wc
        assert detect_vcs(wc) == "svn"

    @pytest.mark.parametrize(
        ("url", "branch"),
        [
            ("https://svn.example.com/repo/branches/feature-x", "branches/feature-x"),
            ("https://svn.example.com/repo/tags/v1.2/sub", "tags/v1.2/sub"),
            ("https://svn.example.com/repo/trunk", "trunk"),
            ("https://svn.example.com/standalone/", "standalone"),
        ],
    )
    def test_metadata_records_branch_and_revision(self, svn_wc, tmp_path, monkeypatch, url, branch):
        _write_fake_svn(tmp_path / "bin", url=url)
        _prepend_path(monkeypatch, tmp_path / "bin")

        assert _svn_revision_info(svn_wc) == (branch, "1234")

        store = GraphStore(tmp_path / "graph.db")
        try:
            _store_vcs_metadata(svn_wc, store)
            assert store.get_metadata("svn_branch") == branch
            assert store.get_metadata("svn_revision") == "1234"
            # Not a git checkout: no git commit metadata is invented.
            assert store.get_metadata("git_head_sha") is None
        finally:
            store.close()

    def test_status_reports_modified_added_deleted_replaced_conflicted(
        self, svn_wc, tmp_path, monkeypatch
    ):
        """Untracked (``?``), missing (``!``) and property-only rows are not changes;
        files grouped under a changelist header still are."""
        log = _write_fake_svn(tmp_path / "bin", url="https://h/repo/trunk")
        _prepend_path(monkeypatch, tmp_path / "bin")

        sources = get_changed_file_sources(svn_wc)

        expected = [
            "src/app.py",
            "src/new.py",
            "old/gone.py",
            "src/replaced.py",
            "src/conflicted.py",
            "src/in_changelist.py",
        ]
        assert sources["files"] == expected
        assert sources["worktree"] == expected
        assert sources["unstaged"] == expected
        assert sources["base_diff"] == []
        assert sources["staged"] == []
        assert sources["untracked"] == []
        # "HEAD~1" is a git spelling: SVN falls back to the working-copy status.
        assert _svn_calls(log) == ["status --non-interactive"]

    def test_revision_range_lists_files_changed_between_revisions(
        self, svn_wc, tmp_path, monkeypatch
    ):
        log = _write_fake_svn(tmp_path / "bin", url="https://h/repo/trunk")
        _prepend_path(monkeypatch, tmp_path / "bin")

        sources = get_changed_file_sources(svn_wc, "r100:HEAD")

        assert sources["files"] == ["src/app.py", "src/new.py", "old/gone.py"]
        assert _svn_calls(log) == ["diff --summarize --non-interactive -r r100:HEAD"]

    def test_unsafe_revision_range_never_reaches_svn(self, svn_wc, tmp_path, monkeypatch):
        """A base that is not a revision range is not passed to ``svn diff -r``."""
        log = _write_fake_svn(tmp_path / "bin", url="https://h/repo/trunk")
        _prepend_path(monkeypatch, tmp_path / "bin")

        get_changed_file_sources(svn_wc, "r1:HEAD --config-option=x")

        calls = _svn_calls(log)
        assert calls == ["status --non-interactive"]
        assert not any("--config-option" in call for call in calls)

    def test_failed_summarize_reports_no_files(self, svn_wc, tmp_path, monkeypatch):
        _write_fake_svn(tmp_path / "bin", url="https://h/repo/trunk", fail_diff=True)
        _prepend_path(monkeypatch, tmp_path / "bin")

        assert get_changed_file_sources(svn_wc, "r1:r2")["files"] == []

    def test_collect_all_files_uses_versioned_files_and_ignore_rules(
        self, svn_wc, tmp_path, monkeypatch
    ):
        """SVN discovery stays tracked-only, then drops ignored, missing,
        symlinked, binary and unparseable entries."""
        log = _write_fake_svn(tmp_path / "bin", url="https://h/repo/trunk")
        _prepend_path(monkeypatch, tmp_path / "bin")
        (svn_wc / "src").mkdir()
        (svn_wc / "src" / "app.py").write_text("def run():\n    return 1\n", encoding="utf-8")
        (svn_wc / "README.md").write_text("# Readme\n", encoding="utf-8")
        (svn_wc / "vendor").mkdir()
        (svn_wc / "vendor" / "lib.py").write_text("x = 1\n", encoding="utf-8")
        (svn_wc / ".dagaynignore").write_text("vendor/**\n", encoding="utf-8")
        (svn_wc / "link.py").symlink_to(svn_wc / "src" / "app.py")
        (svn_wc / "blob.py").write_bytes(b"x = 1\n\x00\x01")
        # On disk but not versioned: SVN discovery must not pick it up.
        (svn_wc / "untracked.py").write_text("y = 2\n", encoding="utf-8")

        assert get_vcs_indexable_files(svn_wc) == [
            "src/app.py",
            "README.md",
            "vendor/lib.py",
            "deleted.py",
            "link.py",
            "blob.py",
        ]
        assert collect_all_files(svn_wc) == ["src/app.py", "README.md"]
        assert "list --recursive --non-interactive" in _svn_calls(log)

    def test_without_an_svn_client_discovery_walks_the_working_copy(
        self, svn_wc, tmp_path, monkeypatch
    ):
        """No ``svn`` on PATH: report no changes and fall back to a directory walk."""
        empty_bin = tmp_path / "empty-bin"
        empty_bin.mkdir()
        monkeypatch.setenv("PATH", str(empty_bin))
        (svn_wc / "app.py").write_text("def run():\n    return 1\n", encoding="utf-8")
        (svn_wc / "notes.txt").write_text("not source\n", encoding="utf-8")
        (svn_wc / ".svn" / "pristine.py").write_text("x = 1\n", encoding="utf-8")

        assert _svn_revision_info(svn_wc) == ("", "")
        assert get_changed_file_sources(svn_wc)["files"] == []
        assert get_changed_file_sources(svn_wc, "r1:HEAD")["files"] == []
        assert get_vcs_indexable_files(svn_wc) == []
        assert collect_all_files(svn_wc) == ["app.py"]


class TestGitUnavailable:
    def test_missing_git_binary_degrades_to_empty_answers(self, main_repo, tmp_path, monkeypatch):
        """An MCP server launched with a minimal PATH must not crash on git calls."""
        head = resolve_commit_sha(main_repo, "HEAD")
        assert head is not None and len(head) == 40

        empty_bin = tmp_path / "empty-bin"
        empty_bin.mkdir()
        monkeypatch.setenv("PATH", str(empty_bin))

        assert _git_branch_info(main_repo) == ("", "")
        assert resolve_commit_sha(main_repo, "HEAD") is None
        assert get_vcs_indexable_files(main_repo) == []
        sources = get_changed_file_sources(main_repo, "HEAD")
        assert sources["files"] == []
        assert sources["base_diff"] == []

    @pytest.mark.parametrize("ref", ["HEAD; rm -rf /", "$(id)", "HEAD --output=/tmp/x", ""])
    def test_unsafe_refs_are_not_resolved(self, main_repo, ref):
        assert resolve_commit_sha(main_repo, ref) is None

    def test_missing_ref_is_none(self, main_repo):
        assert resolve_commit_sha(main_repo, "no-such-branch") is None
        assert resolve_commit_sha(main_repo, "HEAD~5") is None

    def test_metadata_records_branch_and_head(self, main_repo, tmp_path):
        store = GraphStore(tmp_path / "graph.db")
        try:
            _store_vcs_metadata(main_repo, store)
            assert store.get_metadata("git_branch") == "main"
            assert store.get_metadata("git_head_sha") == resolve_commit_sha(main_repo, "HEAD")
            assert store.get_metadata("svn_revision") is None
        finally:
            store.close()


def _node(file_path: str, **extra: object) -> NodeInfo:
    return NodeInfo(
        kind="Function",
        name="run",
        file_path=file_path,
        line_start=1,
        line_end=2,
        language="python",
        extra=dict(extra),
    )


@pytest.mark.skipif(sys.platform == "win32", reason="symlinks need privileges on Windows")
class TestRelativizeThroughSymlinks:
    """Parsed paths and the repo root may name the same checkout differently."""

    @pytest.fixture()
    def linked_repo(self, tmp_path: Path) -> tuple[Path, Path]:
        real = tmp_path / "real-repo"
        (real / "pkg").mkdir(parents=True)
        (real / "pkg" / "a.py").write_text("def run():\n    pass\n", encoding="utf-8")
        link = tmp_path / "linked-repo"
        link.symlink_to(real, target_is_directory=True)
        return real, link

    def test_resolved_file_under_symlinked_root(self, linked_repo):
        real, link = linked_repo
        resolved_file = str((real / "pkg" / "a.py").resolve())

        nodes, edges = _relativize_parsed_entities(
            [_node(resolved_file)],
            [
                EdgeInfo(
                    kind="CALLS",
                    source=f"{resolved_file}::run",
                    target="helper",
                    file_path=resolved_file,
                    line=2,
                )
            ],
            link,
        )

        assert nodes[0].file_path == "pkg/a.py"
        assert edges[0].source == "pkg/a.py::run"
        assert edges[0].file_path == "pkg/a.py"
        # A bare (unqualified, relative) target is left as the parser wrote it.
        assert edges[0].target == "helper"

    def test_symlinked_file_under_real_root(self, linked_repo):
        real, link = linked_repo
        via_link = str(link / "pkg" / "a.py")

        nodes, _edges = _relativize_parsed_entities([_node(via_link)], [], real.resolve())

        assert nodes[0].file_path == "pkg/a.py"

    def test_file_outside_the_repo_keeps_its_absolute_path(self, linked_repo, tmp_path):
        real, _link = linked_repo
        outside = tmp_path / "elsewhere" / "b.py"
        outside.parent.mkdir()
        outside.write_text("x = 1\n", encoding="utf-8")

        nodes, edges = _relativize_parsed_entities(
            [_node(str(outside), level=2, tags=["a", 3], parent_section=f"{outside}::top")],
            [
                EdgeInfo(
                    kind="IMPORTS_FROM",
                    source=f"{real / 'pkg' / 'a.py'}::run",
                    target=str(outside),
                    file_path=str(real / "pkg" / "a.py"),
                    line=1,
                )
            ],
            real,
        )

        assert nodes[0].file_path == str(outside)
        assert nodes[0].extra == {
            "level": 2,
            "tags": ["a", 3],
            "parent_section": f"{outside}::top",
        }
        assert edges[0].source == "pkg/a.py::run"
        assert edges[0].target == str(outside)


class TestRepoGitignore:
    def test_rule_after_comments_and_blank_lines_is_found(self, tmp_path):
        gitignore = tmp_path / ".gitignore"
        original = "# build output\n\n   \n.dagayn\n"
        gitignore.write_text(original, encoding="utf-8")

        assert ensure_repo_gitignore_excludes_crg(tmp_path) == "already-present"
        assert gitignore.read_text(encoding="utf-8") == original

    def test_commented_out_rule_does_not_count(self, tmp_path):
        gitignore = tmp_path / ".gitignore"
        gitignore.write_text("# .dagayn/", encoding="utf-8")

        assert ensure_repo_gitignore_excludes_crg(tmp_path) == "updated"
        assert gitignore.read_text(encoding="utf-8") == "# .dagayn/\n# Added by dagayn\n.dagayn/\n"
