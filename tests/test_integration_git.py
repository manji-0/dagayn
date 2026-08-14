"""Integration tests exercising git-dependent code with real temporary repos.

Tests cover:
- get_changed_files with real git history
- parse_git_diff_ranges with real diffs
- incremental_update detecting real file modifications
- base ref injection rejection
- wiki page path traversal protection
"""

from __future__ import annotations

import subprocess
import tempfile
from pathlib import Path

import pytest

from dagayn.changes import analyze_changes, parse_git_diff_ranges
from dagayn.graph import GraphStore
from dagayn.incremental import (
    collect_all_files,
    full_build,
    get_all_tracked_files,
    get_changed_file_sources,
    get_changed_files,
    incremental_update,
)
from dagayn.wiki import get_wiki_page


def _git(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    """Run a git command inside *repo* and return the result."""
    return subprocess.run(
        ["git", *args],
        capture_output=True,
        text=True,
        cwd=str(repo),
        timeout=10,
    )


@pytest.fixture()
def git_repo(tmp_path: Path) -> Path:
    """Create a real git repo with two commits.

    Commit 1 adds ``hello.py`` with a single function.
    Commit 2 modifies ``hello.py`` (adds a second function).
    """
    repo = tmp_path / "repo"
    repo.mkdir()

    _git(repo, "init")
    _git(repo, "config", "user.email", "test@test.com")
    _git(repo, "config", "user.name", "Test")

    # First commit
    py_file = repo / "hello.py"
    py_file.write_text("def greet():\n    return 'hello'\n")
    _git(repo, "add", "hello.py")
    _git(repo, "commit", "-m", "initial commit")

    # Second commit — modify the file
    py_file.write_text(
        "def greet():\n    return 'hello'\n\ndef farewell():\n    return 'goodbye'\n"
    )
    _git(repo, "add", "hello.py")
    _git(repo, "commit", "-m", "add farewell function")

    return repo


# ------------------------------------------------------------------
# 1. get_changed_files with a real git repo
# ------------------------------------------------------------------


def test_get_changed_files_real_git(git_repo: Path) -> None:
    """get_changed_files should list hello.py as changed between HEAD~1..HEAD."""
    changed = get_changed_files(git_repo, base="HEAD~1")
    assert "hello.py" in changed


def test_get_changed_files_real_git_includes_untracked_with_tracked(
    git_repo: Path,
) -> None:
    """Mixed tracked and untracked worktree changes are returned together."""
    (git_repo / "hello.py").write_text("def greet():\n    return 'hi now'\n")
    (git_repo / "new_file.py").write_text("def fresh():\n    return 'new'\n")

    changed = get_changed_files(git_repo, base="HEAD")
    assert "hello.py" in changed
    assert "new_file.py" in changed


def test_get_changed_file_sources_real_git_separates_committed_from_worktree(
    git_repo: Path,
) -> None:
    """Committed base diffs are distinct from local worktree and untracked files."""
    (git_repo / "hello.py").write_text("def greet():\n    return 'hi now'\n")
    (git_repo / "new_file.py").write_text("def fresh():\n    return 'new'\n")

    sources = get_changed_file_sources(git_repo, base="HEAD")

    assert sources["base_diff"] == []
    assert "hello.py" in sources["unstaged"]
    assert sources["untracked"] == ["new_file.py"]
    assert set(sources["files"]) == {"hello.py", "new_file.py"}


# ------------------------------------------------------------------
# 2. parse_git_diff_ranges with a real git repo
# ------------------------------------------------------------------


def test_parse_git_diff_ranges_real_git(git_repo: Path) -> None:
    """parse_git_diff_ranges should return non-empty line ranges for hello.py."""
    ranges = parse_git_diff_ranges(str(git_repo), base="HEAD~1")
    assert "hello.py" in ranges
    assert len(ranges["hello.py"]) > 0
    # Each entry is a (start, end) tuple with positive line numbers
    for start, end in ranges["hello.py"]:
        assert start >= 1
        assert end >= start


def test_analyze_changes_real_git_marks_added_and_existing_nodes(git_repo: Path) -> None:
    """Base-ref parsing distinguishes existing functions from added functions."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name

    try:
        store = GraphStore(db_path)
        (git_repo / "hello.py").write_text(
            "def greet():\n    return 'hello now'\n\ndef farewell():\n    return 'goodbye'\n"
        )
        full_build(git_repo, store)

        result = analyze_changes(
            store,
            changed_files=["hello.py"],
            changed_ranges=parse_git_diff_ranges(str(git_repo), base="HEAD~1"),
            repo_root=str(git_repo),
            base="HEAD~1",
        )

        statuses = {node["name"]: node["change_status"] for node in result["changed_functions"]}
        assert statuses["greet"] == "existing"
        assert statuses["farewell"] == "added"
        assert result["change_entity_summary"]["nodes"]["existing"] >= 1
        assert result["change_entity_summary"]["nodes"]["added"] >= 1
        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)


# ------------------------------------------------------------------
# 3. incremental_update detects real modifications
# ------------------------------------------------------------------


def test_incremental_update_real_git(git_repo: Path) -> None:
    """Full build then incremental update should detect the second commit."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name

    try:
        store = GraphStore(db_path)

        # Reset to first commit, do a full build
        _git(git_repo, "checkout", "HEAD~1", "--detach")
        full_build(git_repo, store)
        initial_nodes = store.get_stats().total_nodes
        assert initial_nodes > 0, "full_build should create at least one node"

        # Move back to tip (second commit) and do incremental update
        _git(git_repo, "checkout", "-")
        result = incremental_update(git_repo, store, changed_files=["hello.py"])
        assert result["files_updated"] >= 1
        assert "hello.py" in result["changed_files"]

        # The graph should now contain more nodes (farewell function added)
        assert store.get_stats().total_nodes >= initial_nodes

        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)


def test_incremental_update_noop_advances_head_sha(git_repo: Path) -> None:
    """A no-op update over a resolvable base records the new HEAD.

    The content is already indexed, so nothing is re-parsed — but the graph
    does describe HEAD, and leaving ``git_head_sha`` at the base commit would
    report ``git_drift`` on every future session.
    """
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name

    try:
        store = GraphStore(db_path)
        full_build(git_repo, store)
        base = _git(git_repo, "rev-parse", "HEAD").stdout.strip()

        # Commit a file the graph does not track, so the diff is non-empty but
        # nothing needs re-parsing.
        (git_repo / "notes.txt").write_text("no code here\n")
        _git(git_repo, "add", "notes.txt")
        _git(git_repo, "commit", "-m", "add notes")
        head = _git(git_repo, "rev-parse", "HEAD").stdout.strip()
        assert head != base

        result = incremental_update(git_repo, store, base=base)
        assert result["files_updated"] == 0
        assert store.get_metadata("git_head_sha") == head
        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)


def test_incremental_update_narrow_base_does_not_claim_head(git_repo: Path) -> None:
    """A base that never reaches the graph's commit leaves the drift visible.

    ``diff HEAD~1..HEAD`` after two new commits parses the last one only. If
    that stamped HEAD, the graph would report itself synced while silently
    missing the middle commit's files, with no trigger left to fix it.
    """
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name

    try:
        store = GraphStore(db_path)
        full_build(git_repo, store)
        built_at = _git(git_repo, "rev-parse", "HEAD").stdout.strip()

        (git_repo / "middle.py").write_text("def middle():\n    return 'middle'\n")
        _git(git_repo, "add", "middle.py")
        _git(git_repo, "commit", "-m", "add middle.py")
        (git_repo / "last.py").write_text("def last():\n    return 'last'\n")
        _git(git_repo, "add", "last.py")
        _git(git_repo, "commit", "-m", "add last.py")

        incremental_update(git_repo, store, base="HEAD~1")
        indexed = set(store.get_file_meta_map())
        assert "last.py" in indexed
        assert "middle.py" not in indexed
        assert store.get_metadata("git_head_sha") == built_at

        # Diffing from the graph's own commit covers the gap and then stamps.
        incremental_update(git_repo, store, base=built_at)
        assert "middle.py" in set(store.get_file_meta_map())
        head = _git(git_repo, "rev-parse", "HEAD").stdout.strip()
        assert store.get_metadata("git_head_sha") == head
        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)


def test_incremental_update_noop_keeps_head_sha_when_base_unresolvable(
    git_repo: Path,
) -> None:
    """An unresolvable base yields an empty diff that must stay drift."""
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name

    try:
        store = GraphStore(db_path)
        full_build(git_repo, store)
        store.set_metadata("git_head_sha", "0" * 40)
        store.commit()

        result = incremental_update(git_repo, store, base="0" * 40)
        assert result["files_updated"] == 0
        assert store.get_metadata("git_head_sha") == "0" * 40
        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)


# ------------------------------------------------------------------
# 4. base ref injection is rejected
# ------------------------------------------------------------------


def test_base_validation_rejects_injection(git_repo: Path) -> None:
    """Passing a malicious --flag as base should be rejected (empty list)."""
    result = get_changed_files(git_repo, base="--output=/tmp/evil")
    assert result == []


# ------------------------------------------------------------------
# 5. wiki page path traversal is blocked
# ------------------------------------------------------------------


@pytest.fixture()
def git_repo_with_submodule(tmp_path: Path) -> Path:
    """Create a parent repo containing a git submodule with a Python file."""
    # Create the "library" repo that will become a submodule
    lib_repo = tmp_path / "lib"
    lib_repo.mkdir()
    _git(lib_repo, "init")
    _git(lib_repo, "config", "user.email", "test@test.com")
    _git(lib_repo, "config", "user.name", "Test")
    (lib_repo / "util.py").write_text("def helper():\n    pass\n")
    _git(lib_repo, "add", "util.py")
    _git(lib_repo, "commit", "-m", "lib initial")

    # Create the parent repo and add lib as a submodule
    parent = tmp_path / "parent"
    parent.mkdir()
    _git(parent, "init")
    _git(parent, "config", "user.email", "test@test.com")
    _git(parent, "config", "user.name", "Test")
    (parent / "main.py").write_text("def main():\n    pass\n")
    _git(parent, "add", "main.py")
    _git(parent, "commit", "-m", "parent initial")
    _git(
        parent,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        str(lib_repo),
        "lib",
    )
    _git(parent, "commit", "-m", "add lib submodule")

    return parent


def test_get_all_tracked_files_without_recurse(
    git_repo_with_submodule: Path,
) -> None:
    """Without recurse_submodules, submodule files are NOT listed."""
    files = get_all_tracked_files(git_repo_with_submodule, recurse_submodules=False)
    assert "main.py" in files
    # Submodule entry appears as a gitlink, not as individual files
    assert not any(f.startswith("lib/") for f in files)


def test_get_all_tracked_files_with_recurse(
    git_repo_with_submodule: Path,
) -> None:
    """With recurse_submodules=True, submodule files ARE listed."""
    files = get_all_tracked_files(git_repo_with_submodule, recurse_submodules=True)
    assert "main.py" in files
    assert "lib/util.py" in files


def test_collect_all_files_with_recurse(
    git_repo_with_submodule: Path,
) -> None:
    """collect_all_files with recurse_submodules includes submodule code."""
    files = collect_all_files(git_repo_with_submodule, recurse_submodules=True)
    assert "main.py" in files
    assert "lib/util.py" in files


def test_full_build_with_recurse_submodules(
    git_repo_with_submodule: Path,
) -> None:
    """full_build with recurse_submodules parses submodule files."""
    db_path = git_repo_with_submodule / ".dagayn" / "graph.db"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    store = GraphStore(db_path)
    try:
        result = full_build(git_repo_with_submodule, store, recurse_submodules=True)
        assert result["files_parsed"] >= 2  # main.py + lib/util.py
        assert result["errors"] == []

        # Verify both parent and submodule nodes exist
        parent_nodes = store.get_nodes_by_file(str(git_repo_with_submodule / "main.py"))
        sub_nodes = store.get_nodes_by_file(str(git_repo_with_submodule / "lib" / "util.py"))
        assert len(parent_nodes) > 0
        assert len(sub_nodes) > 0
    finally:
        store.close()


def test_wiki_page_path_traversal_blocked(tmp_path: Path) -> None:
    """get_wiki_page must not serve files outside the wiki directory."""
    wiki_dir = tmp_path / "wiki"
    wiki_dir.mkdir()

    # Create a legitimate page
    (wiki_dir / "my-module.md").write_text("# My Module\n")

    # Attempt a path traversal — should return None
    result = get_wiki_page(str(wiki_dir), "../../etc/passwd")
    assert result is None


def _indexed_files(store: GraphStore) -> set[str]:
    return {
        row[0]
        for row in store._conn.execute("SELECT DISTINCT file_path FROM nodes WHERE file_path != ''")
    }


def test_committed_rename_prunes_the_old_path(git_repo: Path) -> None:
    """``git diff --name-only`` reports a rename's destination only.

    Rename detection is on by default, so the source path never entered the
    changed set and its nodes were never removed -- the graph served the same
    code under two paths forever.
    """
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        store = GraphStore(db_path)
        full_build(git_repo, store)
        assert "hello.py" in _indexed_files(store)

        base = _git(git_repo, "rev-parse", "HEAD").stdout.strip()
        _git(git_repo, "mv", "hello.py", "renamed.py")
        _git(git_repo, "commit", "-m", "rename hello.py")

        result = incremental_update(git_repo, store, base=base)
        assert "hello.py" in result["changed_files"], result["changed_files"]
        assert "renamed.py" in result["changed_files"], result["changed_files"]

        indexed = _indexed_files(store)
        assert "renamed.py" in indexed
        assert "hello.py" not in indexed, "renamed-away path kept its nodes"
        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)


def test_non_ascii_filenames_are_indexed(git_repo: Path) -> None:
    """``core.quotePath`` is on by default, so git C-quotes non-ASCII paths.

    The quoted literal is not a path any ``open()`` resolves, so files with
    Japanese or accented names were silently absent from the graph.
    """
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        (git_repo / "日本語.py").write_text("def greet_ja():\n    return 1\n", encoding="utf-8")
        _git(git_repo, "add", "-A")
        _git(git_repo, "commit", "-m", "add a non-ascii filename")

        store = GraphStore(db_path)
        full_build(git_repo, store)
        assert "日本語.py" in _indexed_files(store)

        # And the incremental path must agree with the full build.
        base = _git(git_repo, "rev-parse", "HEAD").stdout.strip()
        (git_repo / "日本語.py").write_text(
            "def greet_ja():\n    return 2\n\ndef added():\n    return 3\n",
            encoding="utf-8",
        )
        _git(git_repo, "add", "-A")
        _git(git_repo, "commit", "-m", "edit the non-ascii file")
        result = incremental_update(git_repo, store, base=base)
        assert "日本語.py" in result["changed_files"], result["changed_files"]
        names = {row[0] for row in store._conn.execute("SELECT name FROM nodes")}
        assert "added" in names
        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)


def test_content_change_with_restored_mtime_is_detected(git_repo: Path) -> None:
    """An mtime equal to the stored one is not proof the content is unchanged.

    ``cp -p`` / ``rsync -a`` / ``tar x`` restore mtimes, and coarse filesystem
    granularity hides two writes in one tick. Both classifiers short-circuited on
    mtime equality without hashing, so the file was skipped forever -- the stored
    hash stayed stale too, so no later run noticed either.
    """
    import os

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        store = GraphStore(db_path)
        full_build(git_repo, store)
        assert "farewell" in {row[0] for row in store._conn.execute("SELECT name FROM nodes")}

        base = _git(git_repo, "rev-parse", "HEAD").stdout.strip()
        target = git_repo / "hello.py"
        stat_before = target.stat()
        target.write_text(
            target.read_text(encoding="utf-8") + "\n\ndef added_later():\n    return 42\n",
            encoding="utf-8",
        )
        os.utime(target, ns=(stat_before.st_atime_ns, stat_before.st_mtime_ns))
        _git(git_repo, "add", "-A")
        _git(git_repo, "commit", "-m", "edit with a restored mtime")
        assert target.stat().st_mtime_ns == stat_before.st_mtime_ns, "premise: mtime unchanged"

        result = incremental_update(git_repo, store, base=base)
        assert result["files_updated"] >= 1, result
        names = {row[0] for row in store._conn.execute("SELECT name FROM nodes")}
        assert "added_later" in names
        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)


def test_file_that_stops_being_indexable_is_removed(git_repo: Path) -> None:
    """Binary-ization and symlink replacement are removals, not skips.

    Both used to `continue`, leaving the file's previous nodes in the graph
    forever while a full build's stale-file purge dropped them -- so the two
    paths disagreed about the same tree.
    """
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        (git_repo / "target.py").write_text("def target_fn():\n    return 1\n", encoding="utf-8")
        (git_repo / "real.py").write_text("def real_fn():\n    return 1\n", encoding="utf-8")
        _git(git_repo, "add", "-A")
        _git(git_repo, "commit", "-m", "add target and real")

        store = GraphStore(db_path)
        full_build(git_repo, store)
        assert "target.py" in _indexed_files(store)

        base = _git(git_repo, "rev-parse", "HEAD").stdout.strip()
        (git_repo / "target.py").write_bytes(b"def target_fn():\n    return 1\n\x00binary")
        _git(git_repo, "add", "-A")
        _git(git_repo, "commit", "-m", "make target binary")

        incremental_update(git_repo, store, base=base)
        assert "target.py" not in _indexed_files(store), "stale nodes kept for a binary file"
        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)


def test_case_only_rename_does_not_duplicate(git_repo: Path) -> None:
    """On a case-insensitive filesystem is_file() still answers for the old name."""
    import os

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        (git_repo / "mod.py").write_text("def mod_fn():\n    return 1\n", encoding="utf-8")
        _git(git_repo, "add", "-A")
        _git(git_repo, "commit", "-m", "add mod")

        store = GraphStore(db_path)
        full_build(git_repo, store)
        base = _git(git_repo, "rev-parse", "HEAD").stdout.strip()

        _git(git_repo, "mv", "-f", "mod.py", "Mod.py")
        _git(git_repo, "commit", "-m", "case-only rename")
        if not os.path.exists(git_repo / "mod.py"):
            pytest.skip("case-sensitive filesystem: the old path really is gone")

        incremental_update(git_repo, store, base=base)
        indexed = _indexed_files(store)
        assert "Mod.py" in indexed
        assert "mod.py" not in indexed, f"case-only rename left a duplicate: {sorted(indexed)}"
        store.close()
    finally:
        Path(db_path).unlink(missing_ok=True)
