"""Tests for change impact analysis (changes.py)."""

import hashlib
import subprocess
import tempfile
from pathlib import Path
from typing import cast
from unittest.mock import patch

import pytest

from dagayn.changes import (
    ChangeMappingResult,
    DiffParseResult,
    _parse_diff_ranges_cached,
    _parse_unified_diff,
    analyze_changes,
    compute_risk_score,
    map_changes_to_nodes,
    map_changes_with_attribution,
    parse_diff_ranges,
    parse_diff_result,
    parse_git_diff,
    parse_git_diff_ranges,
    resolve_git_renames,
)
from dagayn.flows import store_flows, trace_flows
from dagayn.graph import GraphStore
from dagayn.graph.types import ImpactRadiusResult
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.state_types import ChangeAnalysisResult


class TestChanges:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    # -- helpers --

    def _add_func(
        self,
        name: str,
        path: str = "app.py",
        parent: str | None = None,
        is_test: bool = False,
        line_start: int = 1,
        line_end: int = 10,
        language: str = "python",
        extra: dict | None = None,
    ) -> int:
        node = NodeInfo(
            kind="Test" if is_test else "Function",
            name=name,
            file_path=path,
            line_start=line_start,
            line_end=line_end,
            language=language,
            parent_name=parent,
            is_test=is_test,
            extra=extra or {},
        )
        nid = self.store.upsert_node(node, file_hash="abc")
        self.store.commit()
        return nid

    def _add_call(self, source_qn: str, target_qn: str, path: str = "app.py") -> None:
        edge = EdgeInfo(
            kind="CALLS",
            source=source_qn,
            target=target_qn,
            file_path=path,
            line=5,
        )
        self.store.upsert_edge(edge)
        self.store.commit()

    def _add_tested_by(self, target_qn: str, test_qn: str, path: str = "app.py") -> None:
        edge = EdgeInfo(
            kind="TESTED_BY",
            source=target_qn,
            target=test_qn,
            file_path=path,
            line=1,
        )
        self.store.upsert_edge(edge)
        self.store.commit()

    # ---------------------------------------------------------------
    # parse_git_diff_ranges / _parse_unified_diff
    # ---------------------------------------------------------------

    def test_parse_unified_diff_basic(self):
        """Parses a simple unified diff into file -> range mappings."""
        diff = (
            "diff --git a/foo.py b/foo.py\n"
            "--- a/foo.py\n"
            "+++ b/foo.py\n"
            "@@ -10,3 +10,5 @@ def foo():\n"
            "+    new line\n"
            "+    another\n"
        )
        result = _parse_unified_diff(diff)
        assert "foo.py" in result
        assert len(result["foo.py"]) == 1
        start, end = result["foo.py"][0]
        assert start == 10
        assert end == 14  # 10 + 5 - 1

    def test_parse_unified_diff_multiple_hunks(self):
        """Parses a diff with multiple hunks in one file."""
        diff = (
            "diff --git a/bar.py b/bar.py\n"
            "--- a/bar.py\n"
            "+++ b/bar.py\n"
            "@@ -5,2 +5,3 @@ class Bar:\n"
            "+    x\n"
            "@@ -20,1 +21,4 @@ def method():\n"
            "+    y\n"
        )
        result = _parse_unified_diff(diff)
        assert "bar.py" in result
        assert len(result["bar.py"]) == 2
        assert result["bar.py"][0] == (5, 7)  # 5 + 3 - 1
        assert result["bar.py"][1] == (21, 24)  # 21 + 4 - 1

    def test_parse_unified_diff_single_line(self):
        """Parses a diff where count is omitted (single line change)."""
        diff = "--- a/x.py\n+++ b/x.py\n@@ -1 +1 @@\n+changed\n"
        result = _parse_unified_diff(diff)
        assert "x.py" in result
        assert result["x.py"][0] == (1, 1)

    def test_parse_unified_diff_deletion_only(self):
        """Handles pure deletion hunks (+start,0)."""
        diff = "--- a/del.py\n+++ b/del.py\n@@ -10,3 +10,0 @@ some context\n"
        result = _parse_unified_diff(diff)
        assert "del.py" in result
        # Count=0 means deletion, start=end
        assert result["del.py"][0] == (10, 10)

    def test_parse_unified_diff_multiple_files(self):
        """Parses a diff spanning two files."""
        diff = (
            "--- a/a.py\n"
            "+++ b/a.py\n"
            "@@ -1,2 +1,3 @@\n"
            "+x\n"
            "--- a/b.py\n"
            "+++ b/b.py\n"
            "@@ -5,1 +5,2 @@\n"
            "+y\n"
        )
        result = _parse_unified_diff(diff)
        assert "a.py" in result
        assert "b.py" in result

    def test_parse_unified_diff_quoted_non_ascii_path(self):
        """Quoted ``+++`` headers bind hunks to the correct non-ASCII file."""
        diff = (
            "diff --git a/aaa.py b/aaa.py\n"
            "--- a/aaa.py\n"
            "+++ b/aaa.py\n"
            "@@ -3,1 +3,1 @@\n"
            "+changed\n"
            'diff --git "a/caf\\303\\251.py" "b/caf\\303\\251.py"\n'
            '--- "a/caf\\303\\251.py"\n'
            '+++ "b/caf\\303\\251.py"\n'
            "@@ -2,4 +2,4 @@\n"
            "+café changes\n"
        )
        result = _parse_unified_diff(diff)
        assert result == {"aaa.py": [(3, 3)], "café.py": [(2, 5)]}

    def test_parse_unified_diff_deleted_file_not_misattributed(self):
        """Deletion headers reset attribution so hunks are not stolen."""
        diff = (
            "--- a/keep.py\n"
            "+++ b/keep.py\n"
            "@@ -11,2 +11,2 @@\n"
            "+keep change\n"
            "--- a/deleted.py\n"
            "+++ /dev/null\n"
            "@@ -11,2 +1,1 @@\n"
            "-deleted\n"
        )
        result = _parse_unified_diff(diff)
        assert result == {"keep.py": [(11, 12)]}
        assert "deleted.py" not in result

    def test_parse_unified_diff_unrecognized_plus_header_resets_file(self):
        """Malformed ``+++`` headers drop subsequent hunks instead of reusing the prior file."""
        diff = (
            "--- a/first.py\n"
            "+++ b/first.py\n"
            "@@ -1,1 +1,1 @@\n"
            "+first\n"
            "+++ not-a-valid-path\n"
            "@@ -9,1 +9,1 @@\n"
            "+orphan\n"
            "--- a/second.py\n"
            "+++ b/second.py\n"
            "@@ -2,1 +2,1 @@\n"
            "+second\n"
        )
        result = _parse_unified_diff(diff)
        assert result == {"first.py": [(1, 1)], "second.py": [(2, 2)]}

    def test_parse_git_diff_ranges_error_handling(self):
        """Returns empty dict when git command fails."""
        result = parse_git_diff_ranges("/nonexistent/path", base="HEAD~1")
        assert result == {}

    def test_parse_git_diff_reports_base_unresolved(self):
        parsed = parse_git_diff("/nonexistent/path", base="HEAD~1")
        assert parsed.ranges == {}
        assert parsed.status == "base_unresolved"

    def test_parse_diff_result_caches_git_diff_and_returns_copy(self, tmp_path):
        """Repeated review calls should reuse git diff output without sharing mutable results."""
        repo = tmp_path / "repo"
        repo.mkdir()
        calls = []

        def fake_git_ranges(repo_root: str, base: str = "HEAD~1"):
            calls.append((repo_root, base))
            return DiffParseResult({"app.py": [(1, 2)]}, "ok")

        _parse_diff_ranges_cached.cache_clear()
        try:
            with patch("dagayn.changes.parse_git_diff", side_effect=fake_git_ranges):
                first = parse_diff_ranges(str(repo), "HEAD~1")
                first["app.py"].append((99, 99))
                second = parse_diff_ranges(str(repo), "HEAD~1")
                parsed = parse_diff_result(str(repo), "HEAD~1")

            assert calls == [(str(repo.resolve()), "HEAD~1")]
            assert second == {"app.py": [(1, 2)]}
            assert parsed.ranges == {"app.py": [(1, 2)]}
            assert parsed.status == "ok"
        finally:
            _parse_diff_ranges_cached.cache_clear()

    def test_map_changes_degrades_stale_line_ranges_to_file_granular(self, tmp_path):
        """Stale indexed content falls back to file-granular attribution."""
        repo = tmp_path / "repo"
        repo.mkdir()
        src = repo / "src"
        src.mkdir()
        app_py = src / "app.py"
        app_py.write_text(
            "def alpha():\n    pass\n\n\ndef beta():\n    pass\n",
            encoding="utf-8",
        )
        indexed_hash = hashlib.sha256(app_py.read_bytes()).hexdigest()

        self._add_func("alpha", path="src/app.py", line_start=1, line_end=2)
        self._add_func("beta", path="src/app.py", line_start=4, line_end=5)
        self.store._conn.execute(
            "UPDATE nodes SET file_hash=? WHERE file_path=?",
            (indexed_hash, "src/app.py"),
        )
        self.store.commit()

        app_py.write_text(
            "def gamma():\n    pass\n\n\ndef alpha():\n    pass\n\n\ndef beta():\n    pass\n",
            encoding="utf-8",
        )

        mapping = map_changes_with_attribution(
            self.store,
            {"src/app.py": [(1, 2)]},
            repo_root=str(repo),
        )
        assert isinstance(mapping, ChangeMappingResult)
        assert mapping.stale_line_range_files == ["src/app.py"]
        names = {node.name for node in mapping.nodes}
        assert names == {"alpha", "beta"}

    def test_map_changes_uses_line_ranges_when_hash_matches(self, tmp_path):
        repo = tmp_path / "repo"
        repo.mkdir()
        app_py = repo / "app.py"
        app_py.write_text("def alpha():\n    pass\n\n\ndef beta():\n    pass\n", encoding="utf-8")
        file_hash = hashlib.sha256(app_py.read_bytes()).hexdigest()

        self._add_func("alpha", path="app.py", line_start=1, line_end=2)
        self._add_func("beta", path="app.py", line_start=4, line_end=5)
        self.store._conn.execute(
            "UPDATE nodes SET file_hash=? WHERE file_path=?",
            (file_hash, "app.py"),
        )
        self.store.commit()

        mapping = map_changes_with_attribution(
            self.store,
            {"app.py": [(1, 2)]},
            repo_root=str(repo),
        )
        assert mapping.stale_line_range_files == []
        assert {node.name for node in mapping.nodes} == {"alpha"}

    def test_analyze_changes_resolves_renames_via_git_name_status(self, tmp_path):
        repo = tmp_path / "repo"
        repo.mkdir()
        subprocess.run(["git", "init"], cwd=repo, check=True, capture_output=True)
        subprocess.run(
            ["git", "config", "user.email", "test@example.com"],
            cwd=repo,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test"],
            cwd=repo,
            check=True,
            capture_output=True,
        )
        src = repo / "src"
        src.mkdir()
        app_py = src / "app.py"
        app_py.write_text("def alpha():\n    return 1\n", encoding="utf-8")
        subprocess.run(["git", "add", "."], cwd=repo, check=True, capture_output=True)
        subprocess.run(["git", "commit", "-m", "init"], cwd=repo, check=True, capture_output=True)

        renamed = src / "renamed.py"
        app_py.rename(renamed)
        renamed.write_text("def alpha():\n    return 2\n", encoding="utf-8")
        subprocess.run(["git", "add", "-A"], cwd=repo, check=True, capture_output=True)
        subprocess.run(["git", "commit", "-m", "rename"], cwd=repo, check=True, capture_output=True)

        self._add_func("alpha", path="src/app.py", line_start=1, line_end=2)

        rename_map = resolve_git_renames(str(repo), "HEAD~1")
        assert rename_map.get("src/renamed.py") == "src/app.py"

        result = analyze_changes(
            self.store,
            changed_files=[str(renamed)],
            changed_ranges={"src/renamed.py": [(1, 2)]},
            repo_root=str(repo),
            base="HEAD~1",
        )
        assert result.unmapped_changed_files == []
        assert any(func["name"] == "alpha" for func in result.changed_functions)

    def test_analyze_changes_marks_base_unresolved(self, tmp_path):
        repo = tmp_path / "repo"
        repo.mkdir()
        app_py = repo / "app.py"
        app_py.write_text("def alpha():\n    pass\n", encoding="utf-8")
        self._add_func("alpha", path="app.py", line_start=1, line_end=2)
        self._add_func("beta", path="app.py", line_start=4, line_end=5)
        self._add_func("gamma", path="app.py", line_start=7, line_end=8)

        result = analyze_changes(
            self.store,
            changed_files=[str(app_py)],
            repo_root=str(repo),
            base="origin/nonexistent",
        )
        assert result.diff_parse_status == "base_unresolved"
        assert result.change_entity_summary["base"] is None
        assert result.changed_functions == []
        assert result.unmapped_changed_files == ["app.py"]
        assert "diff_base_unreachable" in result.attribution["reason_codes"]
        assert all(func.get("change_status") != "added" for func in result.changed_functions)

    def test_analyze_changes_reports_unmapped_changed_files(self):
        result = analyze_changes(
            self.store,
            changed_files=["missing.py"],
            changed_ranges={"missing.py": [(1, 5)]},
        )
        assert result.unmapped_changed_files == ["missing.py"]
        assert "unmapped_changed_files" in result.attribution["reason_codes"]

    def test_parse_diff_ranges_invalidates_when_worktree_changes(self, tmp_path):
        """Cached diff ranges must refresh after local edits in a long-lived process."""
        repo = tmp_path / "repo"
        repo.mkdir()

        def _git(*args: str) -> None:
            subprocess.run(
                ["git", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args],
                cwd=repo,
                capture_output=True,
                check=True,
            )

        _git("init")
        _git("config", "user.email", "test@example.com")
        _git("config", "user.name", "Test")
        target = repo / "app.py"
        target.write_text("def main():\n    return 1\n", encoding="utf-8")
        _git("add", ".")
        _git("commit", "-m", "initial")

        calls: list[int] = []

        def fake_git_diff(repo_root: str, base: str = "HEAD~1"):
            calls.append(1)
            from dagayn.changes import DiffParseResult

            return DiffParseResult({"app.py": [(len(calls), len(calls))]}, "ok")

        _parse_diff_ranges_cached.cache_clear()
        try:
            with patch("dagayn.changes.parse_git_diff", side_effect=fake_git_diff):
                first = parse_diff_ranges(str(repo), "HEAD")
                target.write_text("def main():\n    return 2\n\ndef extra():\n    pass\n")
                second = parse_diff_ranges(str(repo), "HEAD")

            assert first == {"app.py": [(1, 1)]}
            assert second == {"app.py": [(2, 2)]}
            assert calls == [1, 1]
        finally:
            _parse_diff_ranges_cached.cache_clear()

    def test_parse_diff_ranges_invalidates_on_successive_dirty_edits(self, tmp_path):
        """Second edit to an already-dirty file must not reuse cached diff ranges."""
        repo = tmp_path / "repo"
        repo.mkdir()

        def _git(*args: str) -> None:
            subprocess.run(
                ["git", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args],
                cwd=repo,
                capture_output=True,
                check=True,
            )

        _git("init")
        _git("config", "user.email", "test@example.com")
        _git("config", "user.name", "Test")
        target = repo / "app.py"
        target.write_text("def main():\n    return 1\n", encoding="utf-8")
        _git("add", ".")
        _git("commit", "-m", "initial")
        target.write_text("def main():\n    return 2\n", encoding="utf-8")

        calls: list[int] = []

        def fake_git_diff(repo_root: str, base: str = "HEAD~1"):
            calls.append(1)
            from dagayn.changes import DiffParseResult

            return DiffParseResult({"app.py": [(len(calls), len(calls))]}, "ok")

        _parse_diff_ranges_cached.cache_clear()
        try:
            with patch("dagayn.changes.parse_git_diff", side_effect=fake_git_diff):
                first = parse_diff_ranges(str(repo), "HEAD")
                target.write_text("def main():\n    return 3\n\ndef extra():\n    pass\n")
                second = parse_diff_ranges(str(repo), "HEAD")

            assert first == {"app.py": [(1, 1)]}
            assert second == {"app.py": [(2, 2)]}
            assert len(calls) == 2
        finally:
            _parse_diff_ranges_cached.cache_clear()

    # ---------------------------------------------------------------
    # map_changes_to_nodes
    # ---------------------------------------------------------------

    def test_map_changes_to_nodes_overlap(self):
        """Finds nodes whose line ranges overlap the changed lines."""
        self._add_func("func_a", path="app.py", line_start=5, line_end=15)
        self._add_func("func_b", path="app.py", line_start=20, line_end=30)
        self._add_func("func_c", path="app.py", line_start=35, line_end=45)

        # Change lines 10-25: overlaps func_a (5-15) and func_b (20-30)
        changed_ranges = {"app.py": [(10, 25)]}
        nodes = map_changes_to_nodes(self.store, changed_ranges)

        names = {n.name for n in nodes}
        assert "func_a" in names
        assert "func_b" in names
        assert "func_c" not in names

    def test_map_changes_to_nodes_no_overlap(self):
        """Returns empty when no nodes overlap the changed lines."""
        self._add_func("func_a", path="app.py", line_start=5, line_end=10)

        changed_ranges = {"app.py": [(50, 60)]}
        nodes = map_changes_to_nodes(self.store, changed_ranges)
        assert len(nodes) == 0

    def test_map_changes_to_nodes_deduplication(self):
        """Deduplicates nodes by qualified name when overlapping multiple ranges."""
        self._add_func("func_a", path="app.py", line_start=5, line_end=20)

        # Two ranges that both overlap func_a.
        changed_ranges = {"app.py": [(6, 8), (15, 18)]}
        nodes = map_changes_to_nodes(self.store, changed_ranges)
        assert len(nodes) == 1
        assert nodes[0].name == "func_a"

    def test_analyze_changes_uses_heuristic_test_coverage(self):
        """Test-like nodes that name the target suppress false test gaps."""
        self._add_func(
            "get_minimal_context",
            path="dagayn/tools/context.py",
            line_start=1,
            line_end=20,
        )
        self._add_func(
            "TestGetMinimalContext",
            path="tests/test_tools.py",
            is_test=True,
            line_start=1,
            line_end=20,
        )

        result = analyze_changes(
            self.store,
            changed_files=["dagayn/tools/context.py"],
            changed_ranges={"dagayn/tools/context.py": [(1, 20)]},
        )

        assert result.test_gaps == []

        direct_only = analyze_changes(
            self.store,
            changed_files=["dagayn/tools/context.py"],
            changed_ranges={"dagayn/tools/context.py": [(1, 20)]},
            include_heuristic_test_gap_evidence=False,
        )

        assert [gap["qualified_name"] for gap in direct_only.test_gaps] == [
            "dagayn/tools/context.py::get_minimal_context"
        ]

    def test_test_gap_coverage_confidence_unchecked_when_heuristic_capped(self):
        self._add_func("uncovered_a", path="app/a.py", line_start=1, line_end=10)
        self._add_func("uncovered_b", path="app/b.py", line_start=1, line_end=10)

        result = analyze_changes(
            self.store,
            changed_files=["app/a.py", "app/b.py"],
            changed_ranges={
                "app/a.py": [(1, 10)],
                "app/b.py": [(1, 10)],
            },
            heuristic_test_gap_node_limit=1,
        )

        confidences = {gap["coverage_confidence"] for gap in result.test_gaps}
        assert "unchecked" in confidences
        assert result.test_gap_evidence["heuristic_truncated"] is True

    def test_map_changes_to_nodes_different_files(self):
        """Maps changes across different files."""
        self._add_func("func_x", path="x.py", line_start=1, line_end=10)
        self._add_func("func_y", path="y.py", line_start=1, line_end=10)

        changed_ranges = {
            "x.py": [(3, 5)],
            "y.py": [(3, 5)],
        }
        nodes = map_changes_to_nodes(self.store, changed_ranges)
        names = {n.name for n in nodes}
        assert "func_x" in names
        assert "func_y" in names

    # ---------------------------------------------------------------
    # compute_risk_score
    # ---------------------------------------------------------------

    def test_risk_score_range(self):
        """Risk score is always between 0 and 1."""
        self._add_func("simple_func")
        node = self.store.get_node("app.py::simple_func")
        assert node is not None
        score = compute_risk_score(self.store, node)
        assert 0.0 <= score <= 1.0

    def test_risk_score_untested_is_higher(self):
        """Untested functions score higher than tested ones."""
        self._add_func("untested_func", path="a.py", line_start=1, line_end=10)
        self._add_func("tested_func", path="b.py", line_start=1, line_end=10)
        self._add_func("test_tested_func", path="test_b.py", is_test=True)
        self._add_tested_by("b.py::tested_func", "test_b.py::test_tested_func", "test_b.py")

        untested = self.store.get_node("a.py::untested_func")
        tested = self.store.get_node("b.py::tested_func")
        assert untested is not None
        assert tested is not None

        untested_score = compute_risk_score(self.store, untested)
        tested_score = compute_risk_score(self.store, tested)
        # Untested gets 0.30, tested gets 0.05 for test coverage component.
        assert untested_score > tested_score

    def test_risk_score_security_keywords_boost(self):
        """Functions with security keywords score higher."""
        self._add_func("process_data", path="a.py")
        self._add_func("verify_auth_token", path="b.py")

        normal = self.store.get_node("a.py::process_data")
        secure = self.store.get_node("b.py::verify_auth_token")
        assert normal is not None
        assert secure is not None

        normal_score = compute_risk_score(self.store, normal)
        secure_score = compute_risk_score(self.store, secure)
        assert secure_score > normal_score

    def test_risk_score_with_callers(self):
        """Functions with many callers get a caller count bonus."""
        self._add_func("popular_func", path="lib.py")
        for i in range(10):
            caller_name = f"caller_{i}"
            self._add_func(caller_name, path=f"c{i}.py")
            self._add_call(f"c{i}.py::{caller_name}", "lib.py::popular_func", f"c{i}.py")

        self._add_func("lonely_func", path="other.py")

        popular = self.store.get_node("lib.py::popular_func")
        lonely = self.store.get_node("other.py::lonely_func")
        assert popular is not None
        assert lonely is not None

        popular_score = compute_risk_score(self.store, popular)
        lonely_score = compute_risk_score(self.store, lonely)
        assert popular_score > lonely_score

    def test_risk_score_with_flow_membership(self):
        """Nodes participating in flows get a flow participation bonus."""
        # Build a flow: entry -> helper
        self._add_func("entry", path="app.py", line_start=1, line_end=10)
        self._add_func("helper", path="app.py", line_start=15, line_end=25)
        self._add_call("app.py::entry", "app.py::helper")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        # helper participates in a flow.
        helper = self.store.get_node("app.py::helper")
        assert helper is not None

        # An isolated node with no flows.
        self._add_func("isolated", path="iso.py")
        isolated = self.store.get_node("iso.py::isolated")
        assert isolated is not None

        helper_score = compute_risk_score(self.store, helper)
        isolated_score = compute_risk_score(self.store, isolated)
        # helper should have flow participation bonus.
        assert helper_score >= isolated_score

    def test_risk_score_weighted_by_flow_criticality(self):
        """Nodes in high-criticality flows score higher than low-criticality."""
        # Build two separate flows with different criticality
        self._add_func("hi_entry", path="hi.py", line_start=1, line_end=5)
        self._add_func("hi_func", path="hi.py", line_start=10, line_end=20)
        self._add_call("hi.py::hi_entry", "hi.py::hi_func")

        self._add_func("lo_entry", path="lo.py", line_start=1, line_end=5)
        self._add_func("lo_func", path="lo.py", line_start=10, line_end=20)
        self._add_call("lo.py::lo_entry", "lo.py::lo_func")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        # Manually set different criticality values
        self.store._conn.execute("UPDATE flows SET criticality = 0.9 WHERE name = 'hi_entry'")
        self.store._conn.execute("UPDATE flows SET criticality = 0.1 WHERE name = 'lo_entry'")
        self.store.commit()

        hi = self.store.get_node("hi.py::hi_func")
        lo = self.store.get_node("lo.py::lo_func")
        assert hi and lo

        hi_score = compute_risk_score(self.store, hi)
        lo_score = compute_risk_score(self.store, lo)
        assert hi_score > lo_score, (
            f"High-criticality flow node ({hi_score}) should score "
            f"higher than low-criticality ({lo_score})"
        )

    # ---------------------------------------------------------------
    # analyze_changes
    # ---------------------------------------------------------------

    def test_analyze_changes_returns_expected_keys(self):
        """analyze_changes returns all expected top-level keys."""
        self._add_func("changed_func", path="app.py", line_start=1, line_end=10)
        result = analyze_changes(
            self.store,
            changed_files=["app.py"],
            changed_ranges={"app.py": [(1, 10)]},
        )
        assert isinstance(result.summary, str)
        assert isinstance(result.risk_score, float)
        assert isinstance(result.changed_functions, list)
        assert isinstance(result.affected_flows, list)
        assert isinstance(result.test_gaps, list)
        assert isinstance(result.review_priorities, list)

    def test_analyze_changes_includes_files_without_diff_ranges(self):
        """Untracked files without git diff hunks are treated as whole-file changes."""
        self._add_func("tracked_func", path="/repo/tracked.py", line_start=5, line_end=8)
        self._add_func("untracked_func", path="/repo/untracked.py", line_start=1, line_end=3)

        result = analyze_changes(
            self.store,
            changed_files=["/repo/tracked.py", "/repo/untracked.py"],
            changed_ranges={"/repo/tracked.py": [(5, 8)]},
        )

        names = {f["name"] for f in result.changed_functions}
        assert names == {"tracked_func", "untracked_func"}

    def test_analyze_changes_marks_added_and_existing_entities(self):
        """Changed nodes and relevant edges expose base-vs-current status."""
        self._add_func("existing_func", path="app.py", line_start=1, line_end=5)
        self._add_func("new_func", path="app.py", line_start=8, line_end=12)
        self._add_func("helper", path="helper.py", line_start=1, line_end=5)
        self._add_call("app.py::existing_func", "helper.py::helper", "app.py")
        self._add_call("app.py::new_func", "helper.py::helper", "app.py")

        with patch(
            "dagayn.changes._base_entity_sets",
            return_value=(
                {"app.py::existing_func"},
                {("CALLS", "app.py::existing_func", "helper.py::helper", "app.py")},
            ),
        ):
            result = analyze_changes(
                self.store,
                changed_files=["app.py"],
                changed_ranges={"app.py": [(1, 12)]},
                repo_root="/repo",
                base="HEAD",
            )

        node_status = {f["name"]: f["change_status"] for f in result.changed_functions}
        edge_status = {(e["source"], e["target"]): e["change_status"] for e in result.changed_edges}
        assert node_status["existing_func"] == "existing"
        assert node_status["new_func"] == "added"
        assert edge_status[("app.py::existing_func", "helper.py::helper")] == "existing"
        assert edge_status[("app.py::new_func", "helper.py::helper")] == "added"
        assert result.change_entity_summary["nodes"] == {
            "existing": 1,
            "added": 1,
            "unknown": 0,
        }
        assert result.change_entity_summary["edges"] == {
            "existing": 1,
            "added": 1,
            "unknown": 0,
        }

    def test_analyze_changes_risk_score_range(self):
        """Overall risk score is between 0 and 1."""
        self._add_func("func_a", path="app.py", line_start=1, line_end=10)
        result = analyze_changes(
            self.store,
            changed_files=["app.py"],
            changed_ranges={"app.py": [(1, 10)]},
        )
        assert 0.0 <= result.risk_score <= 1.0

    def test_analyze_detects_test_gaps(self):
        """Changed functions without TESTED_BY edges are flagged as test gaps."""
        self._add_func("untested_a", path="app.py", line_start=1, line_end=10)
        self._add_func("untested_b", path="app.py", line_start=15, line_end=25)
        self._add_func("tested_c", path="app.py", line_start=30, line_end=40)

        # Only tested_c has a test.
        self._add_func("test_c", path="test_app.py", is_test=True)
        self._add_tested_by("app.py::tested_c", "test_app.py::test_c", "test_app.py")

        result = analyze_changes(
            self.store,
            changed_files=["app.py"],
            changed_ranges={"app.py": [(1, 40)]},
        )
        gap_names = {g["name"] for g in result.test_gaps}
        assert "untested_a" in gap_names
        assert "untested_b" in gap_names
        assert "tested_c" not in gap_names

    def test_tested_by_direction_matches_parser_contract(self):
        """TESTED_BY is production -> test for risk, coverage, and test gaps."""
        from dagayn.coverage import infer_tests_for_node

        self._add_func("core_func", path="app.py", line_start=1, line_end=10)
        self._add_func("test_behavior", path="tests/test_app.py", is_test=True)
        self._add_tested_by("app.py::core_func", "tests/test_app.py::test_behavior")

        node = self.store.get_node("app.py::core_func")
        assert node is not None

        assert compute_risk_score(self.store, node) < 0.30
        assert self.store.get_transitive_tests(node.qualified_name)[0]["qualified_name"] == (
            "tests/test_app.py::test_behavior"
        )
        inferred = infer_tests_for_node(self.store, node)
        assert inferred[0]["qualified_name"] == "tests/test_app.py::test_behavior"

        result = analyze_changes(
            self.store,
            changed_files=["app.py"],
            changed_ranges={"app.py": [(1, 10)]},
        )
        assert result.test_gaps == []

    def test_infer_tests_for_node_accepts_bare_tested_by_source(self):
        """Rust cross-file private helper calls may be stored as bare TESTED_BY sources."""
        from dagayn.coverage import infer_tests_for_node

        self._add_func("store_file_batch_tx", path="crates/dagayn-graph/src/helpers.rs")
        self._add_func(
            "stores_file_batch_edge_metadata_once_per_call_site",
            path="crates/dagayn-graph/src/tests.rs",
            is_test=True,
        )
        self._add_tested_by(
            "store_file_batch_tx",
            "crates/dagayn-graph/src/tests.rs::stores_file_batch_edge_metadata_once_per_call_site",
            "crates/dagayn-graph/src/tests.rs",
        )

        node = self.store.get_node("crates/dagayn-graph/src/helpers.rs::store_file_batch_tx")
        assert node is not None

        inferred = infer_tests_for_node(self.store, node)

        assert inferred[0]["qualified_name"] == (
            "crates/dagayn-graph/src/tests.rs::stores_file_batch_edge_metadata_once_per_call_site"
        )
        assert inferred[0]["coverage_source"] == "graph_edge"

    @pytest.mark.parametrize(
        ("language", "source_path", "test_path", "symbol", "test_symbol"),
        [
            ("python", "src/service.py", "tests/test_service.py", "load_user", "test_load_user"),
            ("typescript", "src/service.ts", "src/service.test.ts", "loadUser", "testLoadUser"),
            ("lua", "src/service.lua", "tests/service_test.lua", "load_user", "test_load_user"),
            ("go", "src/service.go", "src/service_test.go", "LoadUser", "TestLoadUser"),
        ],
    )
    def test_bare_tested_by_source_suppresses_test_gap_across_languages(
        self,
        language: str,
        source_path: str,
        test_path: str,
        symbol: str,
        test_symbol: str,
    ):
        self._add_func(symbol, path=source_path, language=language)
        self._add_func(test_symbol, path=test_path, is_test=True, language=language)
        self._add_tested_by(symbol, f"{test_path}::{test_symbol}", test_path)

        result = analyze_changes(
            self.store,
            changed_files=[source_path],
            changed_ranges={source_path: [(1, 10)]},
        )

        assert result.test_gaps == []

    def test_get_review_context_minimal_uses_tested_by_source_as_covered_node(self):
        """Minimal review context should treat TESTED_BY as production -> test."""
        from dagayn.tools.review import _generate_review_guidance, get_review_context

        self._add_func("core_func", path="/repo/app.py", line_start=1, line_end=10)
        self._add_func("test_core_func", path="/repo/tests/test_app.py", is_test=True)
        self._add_tested_by("/repo/app.py::core_func", "/repo/tests/test_app.py::test_core_func")
        prod = self.store.get_node("/repo/app.py::core_func")
        test = self.store.get_node("/repo/tests/test_app.py::test_core_func")
        edge = self.store.get_edges_by_source("/repo/app.py::core_func")[0]
        assert prod is not None
        assert test is not None

        impact = {
            "changed_nodes": [prod],
            "impacted_nodes": [prod, test],
            "impacted_files": ["/repo/app.py", "/repo/tests/test_app.py"],
            "edges": [edge],
        }
        self.store.get_impact_radius = lambda *_args, **_kwargs: impact
        self.store.close = lambda: None

        with patch(
            "dagayn.tools.review_context._get_store",
            return_value=(self.store, Path("/repo")),
        ):
            result = get_review_context(
                changed_files=["app.py"],
                repo_root="/repo",
                detail_level="minimal",
            )

        assert result["test_gaps"] == 0
        assert "lack test coverage" not in _generate_review_guidance(impact, ["app.py"])

    def test_documentation_candidates_hide_heuristic_reachable_by_default(self):
        """Reachable Markdown is an exploratory lead unless explicitly requested."""
        from dagayn.tools.review import _documentation_update_candidates

        self._add_func("service", path="app.py", line_start=1, line_end=10)
        self.store.upsert_node(
            NodeInfo(
                kind="DocSection",
                name="service-contract",
                file_path="docs/service.md",
                line_start=1,
                line_end=2,
                language="markdown",
            )
        )
        self.store.commit()
        doc = self.store.get_node("docs/service.md::service-contract")
        assert doc is not None
        changed_functions = [
            {
                "qualified_name": "app.py::service",
                "file_path": "app.py",
                "kind": "Function",
            }
        ]
        impact = cast(ImpactRadiusResult, {"impacted_nodes": [doc]})

        default_candidates = _documentation_update_candidates(
            self.store,
            impact,
            changed_functions,
            ["app.py"],
        )
        verbose_candidates = _documentation_update_candidates(
            self.store,
            impact,
            changed_functions,
            ["app.py"],
            include_heuristic_docs=True,
        )

        assert default_candidates == []
        assert verbose_candidates[0]["evidence_type"] == "heuristic_reachable"

    def test_component_density_separates_direct_heuristic_and_transitive_tests(self):
        from dagayn.tools.review import _component_density_by_scope

        self._add_func("direct_target", path="app/direct.py")
        self._add_func("heuristic_target", path="app/heuristic.py")
        self._add_func("transitive_target", path="app/transitive.py")
        self._add_func("callee", path="lib/callee.py")
        self._add_func("test_direct_target", path="tests/test_app.py", is_test=True)
        self._add_func("test_heuristic_target", path="tests/test_app.py", is_test=True)
        self._add_func("test_callee", path="tests/test_lib.py", is_test=True)
        self._add_tested_by("app/direct.py::direct_target", "tests/test_app.py::test_direct_target")
        self._add_call("app/transitive.py::transitive_target", "lib/callee.py::callee")
        self._add_tested_by("lib/callee.py::callee", "tests/test_lib.py::test_callee")

        density = _component_density_by_scope(
            self.store,
            {"app"},
            include_supplemental_tests=True,
        )["app"]

        assert density["production_node_count"] == 3
        assert density["supplemental_test_density_evaluated"] is True
        assert density["direct_test_density"] == 0.3333
        assert density["heuristic_test_density"] == 0.3333
        assert density["transitive_test_density"] == 0.3333

    def test_component_density_default_skips_supplemental_test_inference(self):
        from dagayn.tools import review

        self._add_func("direct_target", path="app/direct.py")
        self._add_func("heuristic_target", path="app/heuristic.py")
        self._add_func("test_direct_target", path="tests/test_app.py", is_test=True)
        self._add_tested_by("app/direct.py::direct_target", "tests/test_app.py::test_direct_target")

        with patch("dagayn.tools.review_helpers.infer_tests_for_node") as infer_tests_for_node:
            density = review._component_density_by_scope(self.store, {"app"})["app"]

        infer_tests_for_node.assert_not_called()
        assert density["production_node_count"] == 2
        assert density["supplemental_test_density_evaluated"] is False
        assert density["direct_test_density"] == 0.5
        assert density["heuristic_test_density"] == 0.0
        assert density["transitive_test_density"] == 0.0

    def test_component_density_verbose_supplemental_tests_are_bounded(self):
        from dagayn.tools import review

        self._add_func("a_target", path="app/a.py")
        self._add_func("b_target", path="app/b.py")
        self._add_func("c_target", path="app/c.py")

        with patch(
            "dagayn.tools.review_helpers.infer_tests_for_node",
            return_value=[],
        ) as infer_tests_for_node:
            density = review._component_density_by_scope(
                self.store,
                {"app"},
                include_supplemental_tests=True,
                supplemental_test_density_node_limit=1,
            )["app"]

        assert infer_tests_for_node.call_count == 1
        assert density["production_node_count"] == 3
        assert density["supplemental_test_density_evaluated"] is True
        assert density["supplemental_test_density_sampled_node_count"] == 1
        assert density["supplemental_test_density_truncated"] is True

    def test_analyze_changes_with_flows(self):
        """analyze_changes detects affected flows."""
        self._add_func("handler", path="routes.py", line_start=1, line_end=10)
        self._add_func("service", path="services.py", line_start=1, line_end=10)
        self._add_call("routes.py::handler", "services.py::service", "routes.py")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        result = analyze_changes(
            self.store,
            changed_files=["services.py"],
            changed_ranges={"services.py": [(1, 10)]},
        )
        assert len(result.affected_flows) >= 1

    def test_analyze_changes_review_priorities_ordered(self):
        """Review priorities are ordered by descending risk score."""
        # Create several functions with varying risk levels.
        self._add_func("safe_func", path="app.py", line_start=1, line_end=5)
        self._add_func("auth_handler", path="app.py", line_start=10, line_end=20)

        result = analyze_changes(
            self.store,
            changed_files=["app.py"],
            changed_ranges={"app.py": [(1, 20)]},
        )
        priorities = result.review_priorities
        if len(priorities) >= 2:
            for i in range(len(priorities) - 1):
                assert priorities[i]["risk_score"] >= priorities[i + 1]["risk_score"]

    def test_analyze_changes_fallback_no_ranges(self):
        """Falls back to all nodes in files when no ranges provided."""
        self._add_func("func_a", path="app.py", line_start=1, line_end=10)
        self._add_func("func_b", path="app.py", line_start=15, line_end=25)

        result = analyze_changes(
            self.store,
            changed_files=["app.py"],
            changed_ranges=None,
        )
        # Should still find functions even without ranges.
        assert len(result.changed_functions) >= 1

    # ---------------------------------------------------------------
    # detect_changes_func (integration)
    # ---------------------------------------------------------------

    def test_detect_changes_tool_no_changes(self):
        """detect_changes_func returns clean result when no changes detected."""
        from dagayn.tools import detect_changes_func

        # Patch _get_store to use our test store,
        # and get_changed_file_sources/get_staged_and_unstaged to return empty.
        with (
            patch("dagayn.tools.review._get_store") as mock_get_store,
            patch(
                "dagayn.tools.review.get_changed_file_sources",
                return_value={"files": [], "base_diff": [], "worktree": []},
            ),
            patch("dagayn.tools.review.get_staged_and_unstaged", return_value=[]),
        ):
            mock_get_store.return_value = (self.store, Path("/fake/repo"))
            # Prevent the store from being closed by the tool
            # (our teardown handles it).
            self.store.close = lambda: None

            result = detect_changes_func(base="HEAD~1", repo_root="/fake/repo")
            assert result["status"] == "ok"
            assert result["risk_score"] == 0.0
            assert result["changed_functions"] == []
            assert result["test_gaps"] == []

    def test_classify_test_gap_buckets_docs_and_tests(self):
        """Review gap classification separates docs, tests, and production code."""
        from dagayn.tools.review import _classify_test_gap

        assert _classify_test_gap({"file": "docs/COMMANDS.md"}) == "documentation"
        assert _classify_test_gap({"file": "tests/test_tools.py"}) == "test_artifact"
        assert _classify_test_gap({"file": "dagayn/tools/review.py"}) == "actionable"

    def test_stability_helper_weights_and_node_filters(self):
        """Stable-component helper scoring stays deterministic and code-only."""
        from dagayn.tools.review import (
            _confidence_weight,
            _doc_role_weight,
            _is_low_signal_doc_path,
            _is_production_code_node,
            _scope_key_for_file,
        )

        self._add_func("handler", path="app.py", line_start=1, line_end=5)
        self._add_func("test_handler", path="tests/test_app.py", is_test=True)
        prod_node = self.store.get_node("app.py::handler")
        test_node = self.store.get_node("tests/test_app.py::test_handler")

        assert _scope_key_for_file("dagayn/tools/review.py") == "dagayn/tools"
        assert _confidence_weight(0.2, "HIGH") == 0.9
        assert _doc_role_weight("implemented_by") > _doc_role_weight("discusses_artifact")
        assert _is_production_code_node(prod_node) is True
        assert _is_production_code_node(test_node) is False
        assert _is_low_signal_doc_path("AGENTS.md") is True
        assert _is_low_signal_doc_path("docs/COMMANDS.md") is False

    def test_detect_changes_tool_with_changes(self):
        """detect_changes_func returns full analysis for changed files."""
        from dagayn.tools import detect_changes_func

        self._add_func("my_func", path="/fake/repo/app.py", line_start=1, line_end=10)
        self._add_func(
            "test_my_func",
            path="/fake/repo/test_app.py",
            is_test=True,
            line_start=1,
            line_end=10,
        )
        self._add_tested_by(
            "/fake/repo/app.py::my_func",
            "/fake/repo/test_app.py::test_my_func",
            path="/fake/repo/test_app.py",
        )

        with (
            patch("dagayn.tools.review._get_store") as mock_get_store,
            patch(
                "dagayn.tools.review.get_changed_file_sources",
                return_value={
                    "files": ["app.py"],
                    "base_diff": [],
                    "worktree": ["app.py"],
                    "unstaged": ["app.py"],
                    "untracked": [],
                },
            ),
            patch(
                "dagayn.tools.review.parse_diff_result",
                return_value=DiffParseResult({"app.py": [(1, 10)]}, "ok"),
            ),
        ):
            mock_get_store.return_value = (self.store, Path("/fake/repo"))
            self.store.close = lambda: None

            result = detect_changes_func(base="HEAD~1", repo_root="/fake/repo")
            assert result["status"] == "ok"
            assert "changed_functions" in result
            assert "risk_score" in result
            assert "test_gaps" in result
            assert "review_priorities" in result
            assert result["change_file_sources"]["worktree"] == ["app.py"]
            assert "analysis_summary" in result
            summary = result["analysis_summary"]
            assert summary["risk_level"] in {"low", "medium", "high"}
            assert summary["changed_node_count"] >= 1
            assert summary["impacted_node_count"] >= 1
            assert summary["impacted_file_count"] >= 1
            assert "reason_codes" in summary
            assert summary["recommended_tests"][0]["qualified_name"].endswith("test_my_func")
            assert "next_drill_downs" in summary
            assert "test_gap_ranking" in summary
            assert "signal_quality" in summary
            assert summary["guidance"]
            assert set(summary["guidance"][0]) >= {
                "claim",
                "evidence",
                "confidence",
                "missingness",
                "action",
                "reason_codes",
                "counts",
            }

    def test_detect_changes_scores_stable_component_tests_and_docs(self):
        """Stable packages should surface stronger test and documentation signals."""
        from dagayn.tools import detect_changes_func

        root = Path("/fake/repo")
        service = root / "core" / "service.py"
        client = root / "clients" / "client.py"
        test_file = root / "tests" / "test_service.py"
        doc_file = root / "docs" / "service.md"
        service_qn = f"{service}::stable_api"
        client_qn = f"{client}::use_service"
        test_qn = f"{test_file}::test_stable_api"
        doc_qn = f"{doc_file}::stable-api-contract"

        self._add_func("stable_api", path=str(service), line_start=1, line_end=10)
        self._add_func("use_service", path=str(client), line_start=1, line_end=10)
        self._add_func(
            "test_stable_api",
            path=str(test_file),
            is_test=True,
            line_start=1,
            line_end=10,
        )
        self.store.upsert_node(
            NodeInfo(
                kind="DocSection",
                name="stable-api-contract",
                file_path=str(doc_file),
                line_start=1,
                line_end=4,
                language="markdown",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source=client_qn,
                target=service_qn,
                file_path=str(client),
                line=1,
            )
        )
        self._add_tested_by(service_qn, test_qn, path=str(test_file))
        self.store.upsert_edge(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source=doc_qn,
                target=service_qn,
                file_path=str(doc_file),
                line=2,
                extra={
                    "relationship_role": "implemented_by",
                    "bridge_kind": "documentation",
                },
            )
        )
        self.store.commit()

        with (
            patch("dagayn.tools.review._get_store") as mock_get_store,
            patch(
                "dagayn.tools.review.get_changed_file_sources",
                return_value={
                    "files": ["core/service.py"],
                    "base_diff": ["core/service.py"],
                    "worktree": [],
                },
            ),
            patch(
                "dagayn.tools.review.parse_diff_result",
                return_value=DiffParseResult({"core/service.py": [(1, 10)]}, "ok"),
            ),
        ):
            mock_get_store.return_value = (self.store, root)
            self.store.close = lambda: None

            result = detect_changes_func(base="HEAD~1", repo_root=str(root))

        summary = result["analysis_summary"]
        assert summary["recommended_tests"][0]["qualified_name"] == test_qn
        assert summary["recommended_tests"][0]["score"] >= 0.95
        assert summary["recommended_tests"][0]["stability"]["stable"] is True
        assert summary["documentation_update_candidates"][0]["qualified_name"] == doc_qn
        assert summary["documentation_update_candidates"][0]["stable_contract"] is True
        assert summary["documentation_update_candidates"][0]["directive_hint"] == (
            "<!-- dagayn: implemented-by <code-symbol> -->"
        )
        assert any(
            item["reason_codes"] == ["documentation_update_candidates"]
            and item["evidence"][0]["type"] == "authored"
            for item in summary["guidance"]
        )
        contract = summary["stability_contracts"][0]
        assert contract["scope_key"].endswith("core")
        assert contract["stable"] is True
        assert contract["status"] == "ok"

    def test_directive_hint_for_role_uses_dagayn_directives(self):
        """Documentation candidate hints should use supported dagayn directive syntax."""
        from dagayn.tools.review import (
            _directive_hint_for_role,
            _doc_evidence_type,
            _doc_missingness,
        )

        assert (
            _directive_hint_for_role(
                "implements_contract",
                direction="artifact_to_doc",
            )
            == "# dagayn: implements <doc-section>"
        )
        assert (
            _directive_hint_for_role(
                "implemented_by",
                direction="doc_to_artifact",
            )
            == "<!-- dagayn: implemented-by <code-symbol> -->"
        )
        assert (
            _directive_hint_for_role(
                None,
                direction="doc_to_artifact",
            )
            == "<!-- dagayn: discusses-artifact <code-symbol> -->"
        )
        assert _doc_evidence_type("implemented_by", "LOW") == "authored"
        assert _doc_evidence_type("describes_symbol", "HIGH") == "extracted"
        assert _doc_evidence_type(None, "UNKNOWN") == "heuristic_reachable"
        assert _doc_missingness("implemented_by", "HIGH") == []
        assert _doc_missingness("describes_symbol", "LOW") == [
            {
                "reason_code": "not_contract_documentation_edge",
                "severity": "low",
                "claim_effect": "candidate may be explanatory rather than contract-bearing",
            },
            {
                "reason_code": "low_confidence_documentation_edge",
                "severity": "medium",
                "claim_effect": "read the section before treating it as authored evidence",
            },
        ]

    def test_detect_changes_tool_trims_changed_functions(self):
        """detect_changes_func should budget changed_functions for large PRs."""
        from dagayn.tools import detect_changes_func

        huge_functions = [{"name": f"func_{i}", "payload": "x" * 500} for i in range(200)]

        with (
            patch("dagayn.tools.review._get_store") as mock_get_store,
            patch(
                "dagayn.tools.review.get_changed_file_sources",
                return_value={"files": ["app.py"], "base_diff": ["app.py"], "worktree": []},
            ),
            patch("dagayn.tools.review.parse_diff_result", return_value=DiffParseResult({}, "ok")),
            patch(
                "dagayn.tools.review.analyze_changes",
                return_value=ChangeAnalysisResult.model_validate(
                    {
                        "summary": "large diff",
                        "risk_score": 0.9,
                        "changed_functions": huge_functions,
                        "affected_flows": [
                            {"name": f"flow_{i}", "payload": "y" * 300} for i in range(50)
                        ],
                        "test_gaps": [
                            {"name": f"gap_{i}", "payload": "z" * 300} for i in range(50)
                        ],
                        "review_priorities": [
                            {"name": f"prio_{i}", "payload": "w" * 300} for i in range(20)
                        ],
                    }
                ),
            ),
        ):
            mock_get_store.return_value = (self.store, Path("/fake/repo"))
            self.store.close = lambda: None

            result = detect_changes_func(base="HEAD~1", repo_root="/fake/repo")

            assert result["status"] == "ok"
            assert result["truncated"] is True
            assert len(result["changed_functions"]) < len(huge_functions)
