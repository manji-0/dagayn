"""Tests for skills and hooks auto-install."""

import json
import os
import stat
import sys
import tomllib
from pathlib import Path
from unittest.mock import patch

import yaml

import dagayn.skills as _skills_module
from dagayn.hook_guard import DEFAULT_HOOK_BUDGET_SECONDS
from dagayn.skills import (
    _CLAUDE_MD_SECTION_HEADING,
    _CLAUDE_MD_SECTION_MARKER,
    _MARKDOWN_POLICY_HEADING,
    _MARKDOWN_POLICY_MARKER,
    PLATFORMS,
    _cursor_hook_scripts,
    _dagayn_hook_scripts,
    _detect_serve_command,
    _has_instruction_section,
    _in_poetry_project,
    _in_uv_project,
    _opencode_plugin_content,
    _resolve_source_skills_dir,
    generate_cursor_hooks_config,
    generate_hermes_hooks_config,
    generate_hooks_config,
    generate_pi_hooks_config,
    generate_skills,
    inject_claude_md,
    inject_platform_instructions,
    install_codex_hooks,
    install_codex_skills,
    install_cursor_hooks,
    install_git_hook,
    install_global_skills,
    install_hermes_hooks,
    install_hermes_skills,
    install_hooks,
    install_opencode_plugin,
    install_opencode_skills,
    install_pi_hooks,
    install_pi_skills,
    install_platform_configs,
    install_qoder_skills,
    normalize_platform_target,
)

EXPECTED_SKILLS = [
    "architecture-analysis.md",
    "build-graph.md",
    "cross-repo-workflows.md",
    "debug-issue.md",
    "explore-codebase.md",
    "implement-feature.md",
    "install-dagayn.md",
    "reading-markdown-document.md",
    "refactor-safely.md",
    "review-changes.md",
    "review-delta.md",
    "review-pr.md",
    "semantic-search.md",
    "worktree-sync.md",
    "writing-markdown-document.md",
]

LEGACY_MCP_TOOL_NAMES = [
    "detect_changes_tool",
    "get_review_context_tool",
    "get_impact_radius_tool",
    "get_affected_flows_tool",
    "get_architecture_overview_tool",
    "semantic_search_nodes`",
    "query_graph`",
    "find_large_functions`",
    "get_minimal_context`",
]

CURRENT_MCP_TOOL_NAMES = [
    "apply_refactor_tool",
    "ensure_graph_tool",
    "get_minimal_context_tool",
    "review_tool",
    "architecture_analysis_tool",
    "query_graph_tool",
    "semantic_search_nodes_tool",
    "flow_tool",
    "refactor_tool",
    "build_or_update_graph_tool",
    "cross_repo_search_tool",
    "embed_graph_tool",
    "list_repos_tool",
    "run_postprocess_tool",
]


class TestGenerateSkills:
    def test_creates_skills_directory(self, tmp_path):
        result = generate_skills(tmp_path)
        assert result.is_dir()
        assert result == tmp_path / ".claude" / "skills"

    def test_creates_skill_files_from_disk(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        files = sorted(f.name for f in skills_dir.iterdir())
        assert files == EXPECTED_SKILLS

    def test_skill_files_have_frontmatter(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        for path in skills_dir.iterdir():
            content = path.read_text()
            assert content.startswith("---\n")
            assert "name:" in content
            assert "description:" in content
            # Frontmatter closes
            lines = content.split("\n")
            assert lines[0] == "---"
            closing_idx = content.index("---", 4)
            assert closing_idx > 0

    def test_custom_skills_dir(self, tmp_path):
        custom = tmp_path / "my-skills"
        result = generate_skills(tmp_path, skills_dir=custom)
        assert result == custom
        assert result.is_dir()
        assert len(list(result.iterdir())) == len(EXPECTED_SKILLS)

    def test_markdown_skills_present(self, tmp_path):
        """The two markdown skills must ship with every install."""
        skills_dir = generate_skills(tmp_path)
        assert (skills_dir / "writing-markdown-document.md").is_file()
        assert (skills_dir / "reading-markdown-document.md").is_file()

    def test_operational_skills_cover_3_0_surfaces(self, tmp_path):
        """Install target should cover setup, embeddings, wiki, and cross-repo work."""
        skills_dir = generate_skills(tmp_path)
        install = (skills_dir / "install-dagayn.md").read_text()
        semantic = (skills_dir / "semantic-search.md").read_text()
        cross_repo = (skills_dir / "cross-repo-workflows.md").read_text()

        assert "dagayn install --platform codex" in install
        assert "--no-instructions" in install
        assert "embed_graph_tool" in semantic
        assert 'search_mode="hybrid"' in semantic
        assert "cross_repo_search_tool" in cross_repo
        assert "dagayn daemon" in cross_repo

    def test_new_workflow_skills_cover_worktree_and_feature(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        worktree = (skills_dir / "worktree-sync.md").read_text()
        feature = (skills_dir / "implement-feature.md").read_text()
        install = (skills_dir / "install-dagayn.md").read_text()
        review = (skills_dir / "review-changes.md").read_text()

        assert "dagayn session prepare" in worktree
        assert "worktree info" in worktree
        assert ".worktreeinclude" in worktree
        assert "ensure_graph_tool" in feature
        assert "extension points" in feature
        assert 'review_tool(mode="changes")' in feature
        assert "Worktree bootstrap" in install
        assert "Docs update after code change" in review
        assert "writing-markdown-document" in review

    def test_search_skills_are_mode_neutral_without_install_context(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        semantic = (skills_dir / "semantic-search.md").read_text()
        explore = (skills_dir / "explore-codebase.md").read_text()
        build = (skills_dir / "build-graph.md").read_text()
        writing = (skills_dir / "writing-markdown-document.md").read_text()

        assert "mode-neutral" in semantic
        assert "mode-neutral" in explore
        assert "mode-neutral" in build
        assert "mode-neutral" in writing
        assert "<!-- dagayn skill embedding context -->" in semantic
        assert "<!-- /dagayn skill embedding context -->" in semantic

    def test_exploration_skills_encode_search_to_traversal_ladder(self, tmp_path):
        """Agents should learn search -> relationship query -> bounded traversal."""
        skills_dir = generate_skills(tmp_path)
        explore = (skills_dir / "explore-codebase.md").read_text()
        semantic = (skills_dir / "semantic-search.md").read_text()
        debug = (skills_dir / "debug-issue.md").read_text()

        assert "## Decision Model" in explore
        assert "Unknown entity, fuzzy concept" in explore
        assert "Known entity plus a specific relationship" in explore
        assert "Neighborhood exploration: use `traverse_graph_tool` only after" in explore
        assert "Treat semantic search as start-node discovery, not final proof" in semantic
        assert "Hand off the best hit to graph tools" in semantic
        assert "Prefer relationship queries over raw traversal" in debug

    def test_generate_skills_renders_local_embedding_context(self, tmp_path):
        skills_dir = generate_skills(
            tmp_path,
            embedding_mode="local-embedding",
        )
        semantic = (skills_dir / "semantic-search.md").read_text()
        debug = (skills_dir / "debug-issue.md").read_text()
        build = (skills_dir / "build-graph.md").read_text()
        writing = (skills_dir / "writing-markdown-document.md").read_text()
        review_pr = (skills_dir / "review-pr.md").read_text()

        assert "--mode local-embedding" in semantic
        assert "BGE-M3" in semantic
        assert 'local_embedding="none"' in semantic
        assert "embedding-enabled full rebuild" in semantic
        assert "`search_mode`, `rerank_intent`, and per-result `source`" in semantic
        assert "Process-pattern prose should use narrative embeddings" in semantic
        assert "mode-neutral" not in semantic
        assert "--mode local-embedding" in debug
        assert "--mode local-embedding" in build
        assert 'local_embedding="none"' in build
        assert "trigger a large embedding refresh" in build
        assert "--mode local-embedding" in writing
        assert 'local_embedding="none"' in writing
        assert "exact symbol match" in writing
        assert "Ignore semantic near-matches" in writing
        assert "--mode local-embedding" in review_pr

    def test_generate_skills_renders_local_embedding_llama_context(self, tmp_path):
        skills_dir = generate_skills(
            tmp_path,
            embedding_mode="local-embedding-llama",
            embedding_preset="low",
        )
        semantic = (skills_dir / "semantic-search.md").read_text()
        build = (skills_dir / "build-graph.md").read_text()

        assert "--mode local-embedding-llama --preset low" in semantic
        assert "managed Qwen3" in semantic
        assert "`search_mode`, `rerank_intent`, and per-result `source`" in semantic
        assert 'local_embedding="none"' in build
        assert "server sidecar mode" in build

    def test_generate_skills_renders_fts_context(self, tmp_path):
        skills_dir = generate_skills(tmp_path, embedding_mode="fts-only")
        semantic = (skills_dir / "semantic-search.md").read_text()
        cross_repo = (skills_dir / "cross-repo-workflows.md").read_text()
        build = (skills_dir / "build-graph.md").read_text()

        assert "FTS-only mode" in semantic
        assert "Do not rebuild embeddings" in semantic
        assert "`keyword_fallback` means the FTS index is absent" in semantic
        assert "keyword/FTS search" in cross_repo
        assert "FTS-only mode" in build
        assert "Do not rebuild embeddings" in build

    def test_generate_skills_renders_remote_context(self, tmp_path):
        skills_dir = generate_skills(
            tmp_path,
            embedding_mode="remote-embedding",
            embedding_provider="openai",
        )
        semantic = (skills_dir / "semantic-search.md").read_text()
        cross_repo = (skills_dir / "cross-repo-workflows.md").read_text()
        review_pr = (skills_dir / "review-pr.md").read_text()

        assert "--mode remote-embedding --provider openai" in semantic
        assert "`search_mode`, `rerank_intent`, and per-result `source`" in semantic
        assert 'embed_graph_tool(provider="openai")' in semantic
        assert "remote embedding calls" in cross_repo
        assert "--mode remote-embedding --provider openai" in review_pr

    def test_review_skills_use_composed_analysis_outputs(self, tmp_path):
        """Generated review skills should point agents at composed Tier 1 output."""
        skills_dir = generate_skills(tmp_path)
        review_changes = (skills_dir / "review-changes.md").read_text()
        review_delta = (skills_dir / "review-delta.md").read_text()
        review_pr = (skills_dir / "review-pr.md").read_text()

        assert "analysis_summary" in review_changes
        assert "analysis_summary" in review_delta
        assert "analysis_summary" in review_pr
        assert "recommended_tests" in review_pr
        for content in (review_changes, review_delta, review_pr):
            assert "review_tool" in content
            assert "detect_changes_tool" not in content
            assert "get_review_context_tool" not in content
            assert "get_impact_radius_tool" not in content

    def test_refactor_skills_explain_function_concern_profiles(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        refactor = (skills_dir / "refactor-safely.md").read_text()
        review = (skills_dir / "review-changes.md").read_text()

        assert "Function Concern Separation Profiles" in refactor
        assert "evidence.concern_separation" in refactor
        assert "role-aware refactoring profile" in refactor
        assert "purity_likelihood" in refactor
        assert "missingness" in refactor
        assert "function_concern_pressure" in review
        assert "not correctness evidence" in review
        assert "Do not report a function concern profile as a bug by itself" in review

    def test_generated_skills_use_current_mcp_tool_names(self, tmp_path):
        """Packaged skills should match the 3.0 MCP dispatcher interface."""
        skills_dir = generate_skills(tmp_path)
        combined = "\n".join(path.read_text() for path in skills_dir.iterdir())

        for tool_name in CURRENT_MCP_TOOL_NAMES:
            assert tool_name in combined
        for legacy_name in LEGACY_MCP_TOOL_NAMES:
            assert legacy_name not in combined

    def test_workflow_skills_gate_on_ensure_graph_when_empty(self, tmp_path):
        """Everyday workflow skills must bootstrap via ensure_graph on empty graphs."""
        skills_dir = generate_skills(tmp_path)
        for name in (
            "review-changes.md",
            "explore-codebase.md",
            "debug-issue.md",
            "architecture-analysis.md",
            "refactor-safely.md",
        ):
            content = (skills_dir / name).read_text()
            assert "ensure_graph_tool" in content
            assert "graph_health.status" in content

    def test_explore_skill_follows_decision_model_not_architecture_first(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        content = (skills_dir / "explore-codebase.md").read_text()
        assert "Pick **one** next move from the Decision Model" in content
        assert "open with architecture overview unless" in content
        steps = content.split("### Steps", 1)[1]
        assert "Pick **one** next move from the Decision Model" in steps
        assert steps.index("Pick **one** next move") < steps.index(
            'architecture_analysis_tool(mode="overview"'
        )

    def test_review_skills_refresh_only_when_needed(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        delta = (skills_dir / "review-delta.md").read_text()
        pr = (skills_dir / "review-pr.md").read_text()
        for content in (delta, pr):
            assert "Refresh only when needed" in content
            assert "Otherwise skip ensure" in content
            assert "Do not call `ensure_graph_tool(force=True)` on every" in content

    def test_explore_skill_uses_architecture_health(self, tmp_path):
        """Generated exploration skill should use the composed architecture surface."""
        skills_dir = generate_skills(tmp_path)
        content = (skills_dir / "explore-codebase.md").read_text()
        assert "architecture_health" in content
        assert "architecture_analysis_tool" in content
        assert "flow_tool" in content

    def test_architecture_skill_uses_dispatcher_modes(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        content = (skills_dir / "architecture-analysis.md").read_text()
        assert 'architecture_analysis_tool(mode="overview"' in content
        assert "sdp_violations" in content
        assert "sap_violations" in content
        assert "get_architecture_overview_tool" not in content

    def test_debug_and_explore_list_flows_before_get(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        debug = (skills_dir / "debug-issue.md").read_text()
        explore = (skills_dir / "explore-codebase.md").read_text()

        assert 'flow_tool(mode="list"' in debug
        assert 'flow_tool(mode="get")' in debug
        assert debug.index('flow_tool(mode="list"') < debug.index('flow_tool(mode="get")')
        assert 'flow_tool(mode="list"' in explore
        assert 'flow_tool(mode="get")' in explore

    def test_markdown_reading_prefers_rg_for_raw_scans(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        content = (skills_dir / "reading-markdown-document.md").read_text()

        assert "rg -n '<!--" in content
        assert "grep -nE" not in content

    def test_generated_skills_include_markdown_code_traceability_guidance(self, tmp_path):
        skills_dir = generate_skills(tmp_path)
        writing = (skills_dir / "writing-markdown-document.md").read_text()
        reading = (skills_dir / "reading-markdown-document.md").read_text()
        review = (skills_dir / "review-changes.md").read_text()
        explore = (skills_dir / "explore-codebase.md").read_text()

        assert "Markdown ↔ code documentation links" in writing
        assert "<!-- dagayn: implemented-by services/auth.py::refresh_token -->" in writing
        assert "# dagayn: implements docs/auth-spec.md#Token Refresh" in writing
        assert 'query_graph_tool(pattern="implementations_of"' in reading
        assert 'query_graph_tool(pattern="docs_for"' in reading
        assert "`CROSS_ARTIFACT` documentation roles" in review
        assert "Markdown ↔ code traceability" in explore

    def test_idempotent(self, tmp_path):
        """Running twice should not fail and files should still be valid."""
        generate_skills(tmp_path)
        generate_skills(tmp_path)
        skills_dir = tmp_path / ".claude" / "skills"
        assert len(list(skills_dir.iterdir())) == len(EXPECTED_SKILLS)

    def test_reinstall_updates_existing_flat_skill_content(self, tmp_path):
        generate_skills(tmp_path)
        skills_dir = tmp_path / ".claude" / "skills"
        target = skills_dir / "writing-markdown-document.md"
        target.write_text("stale skill content", encoding="utf-8")

        generate_skills(tmp_path)

        assert target.read_text(encoding="utf-8") != "stale skill content"
        assert "writing-markdown-document" in target.read_text(encoding="utf-8")


class TestResolveSourceSkillsDir:
    """Regression coverage for wheel-install vs dev-checkout layouts.

    The wheel ships ``skills/`` inside the dagayn package via hatch
    ``force-include`` (see pyproject.toml). When that source layout
    is the only one available — i.e., the dev-checkout fallback at
    ``parent.parent / 'skills'`` is missing — resolution must still
    succeed.
    """

    def test_falls_back_to_packaged_skills_dir(self, tmp_path, monkeypatch):
        # Simulate a wheel-install layout: <site-packages>/dagayn/skills/...
        fake_pkg = tmp_path / "site-packages" / "dagayn"
        fake_pkg.mkdir(parents=True)
        skill_dir = fake_pkg / "skills" / "demo"
        skill_dir.mkdir(parents=True)
        (skill_dir / "SKILL.md").write_text(
            "---\nname: demo\ndescription: x\n---\n\nbody\n", encoding="utf-8"
        )
        # And ensure the dev-checkout candidate (parent.parent / skills)
        # is empty so the second candidate is the only valid one.
        (tmp_path / "site-packages").mkdir(exist_ok=True)

        fake_module_file = fake_pkg / "skills.py"
        fake_module_file.write_text("# stub", encoding="utf-8")
        monkeypatch.setattr(_skills_module, "__file__", str(fake_module_file))

        resolved = _resolve_source_skills_dir()
        assert resolved == fake_pkg / "skills"

    def test_prefers_package_local_over_parent_sibling(self, tmp_path, monkeypatch):
        """wheel-local skills/ takes priority over parent.parent/skills/.

        In an installed wheel, parent.parent is site-packages root.  A
        stale or unrelated directory there should not shadow the real
        package-local skills.
        """
        fake_pkg = tmp_path / "site-packages" / "dagayn"
        fake_pkg.mkdir(parents=True)

        # Wheel-local layout: <site-packages>/dagayn/skills/<name>/SKILL.md
        local_skill = fake_pkg / "skills" / "local-skill"
        local_skill.mkdir(parents=True)
        (local_skill / "SKILL.md").write_text(
            "---\nname: local-skill\ndescription: x\n---\n\nbody\n", encoding="utf-8"
        )

        # dev-checkout / stale layout: <site-packages>/skills/<name>/SKILL.md
        parent_skill = tmp_path / "site-packages" / "skills" / "stale-skill"
        parent_skill.mkdir(parents=True)
        (parent_skill / "SKILL.md").write_text(
            "---\nname: stale-skill\ndescription: y\n---\n\nbody\n", encoding="utf-8"
        )

        fake_module_file = fake_pkg / "skills.py"
        fake_module_file.write_text("# stub", encoding="utf-8")
        monkeypatch.setattr(_skills_module, "__file__", str(fake_module_file))

        resolved = _resolve_source_skills_dir()
        # Must return the package-local candidate, not the parent-sibling one.
        assert resolved == fake_pkg / "skills"


class TestInstallGlobalSkills:
    def test_writes_to_home_claude_skills(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_global_skills()
        assert result == tmp_path / ".claude" / "skills"
        assert result.is_dir()

    def test_writes_both_new_skills(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_global_skills()
        target = tmp_path / ".claude" / "skills"
        assert (target / "writing-markdown-document.md").is_file()
        assert (target / "reading-markdown-document.md").is_file()

    def test_renders_embedding_context(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_global_skills(embedding_mode="local-embedding")
        target = tmp_path / ".claude" / "skills" / "semantic-search.md"
        content = target.read_text()
        assert "--mode local-embedding" in content
        assert "BGE-M3" in content
        assert "mode-neutral" not in content

    def test_idempotent(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_global_skills()
            install_global_skills()
        target = tmp_path / ".claude" / "skills"
        assert len(list(target.iterdir())) == len(EXPECTED_SKILLS)

    def test_does_not_clobber_unrelated_files(self, tmp_path):
        """Files in ~/.claude/skills/ that don't match a packaged skill are left alone."""
        target = tmp_path / ".claude" / "skills"
        target.mkdir(parents=True)
        unrelated = target / "user-custom-skill.md"
        unrelated.write_text("# my own skill")
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_global_skills()
        assert unrelated.is_file()
        assert unrelated.read_text() == "# my own skill"

    def test_init_handle_survives_permission_error(self, tmp_path, capsys):
        """dagayn install must complete even when ~/.claude/skills/ is not writable.

        Codex review C1-2: install_global_skills() may raise PermissionError in
        CI/containers with read-only $HOME; the repo-local setup must still succeed.
        """
        import argparse

        from dagayn.cli.commands.init import handle

        args = argparse.Namespace(
            repo=str(tmp_path),
            dry_run=False,
            platform="all",
            yes=True,
            no_skills=False,
            no_hooks=True,
            no_instructions=True,
            skills=False,
            hooks=False,
            install_all=False,
            mode="fts-only",
            preset=None,
            provider=None,
        )

        with (
            patch("dagayn.incremental.find_repo_root", return_value=tmp_path),
            patch("dagayn.skills.install_platform_configs", return_value=[]),
            patch(
                "dagayn.incremental.ensure_repo_gitignore_excludes_crg",
                return_value="already",
            ),
            patch("dagayn.skills.generate_skills", return_value=tmp_path / ".claude" / "skills"),
            patch(
                "dagayn.skills.install_global_skills",
                side_effect=PermissionError("read-only home"),
            ),
        ):
            handle(args)  # must not raise

        captured = capsys.readouterr()
        assert "Skipped global skills install" in captured.err


class TestInstallTreeSkills:
    def test_installs_codex_skill_tree(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_codex_skills()

        assert result == tmp_path / ".codex" / "skills"
        assert (result / "writing-markdown-document" / "SKILL.md").is_file()
        assert (result / "reading-markdown-document" / "SKILL.md").is_file()

    def test_installs_opencode_skill_tree(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_opencode_skills()

        assert result == tmp_path / ".config" / "opencode" / "skills"
        assert (result / "writing-markdown-document" / "SKILL.md").is_file()
        assert (result / "reading-markdown-document" / "SKILL.md").is_file()

    def test_installs_pi_skill_tree(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_pi_skills()

        assert result == tmp_path / ".pi" / "agent" / "skills"
        assert (result / "writing-markdown-document" / "SKILL.md").is_file()
        assert (result / "reading-markdown-document" / "SKILL.md").is_file()

    def test_installs_hermes_skill_tree(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_hermes_skills()

        assert result == tmp_path / ".hermes" / "skills"
        assert (result / "writing-markdown-document" / "SKILL.md").is_file()
        assert (result / "reading-markdown-document" / "SKILL.md").is_file()

    def test_tree_skill_install_renders_embedding_context(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_codex_skills(embedding_mode="local-embedding")

        target = result / "semantic-search" / "SKILL.md"
        content = target.read_text()
        assert "--mode local-embedding" in content
        assert "BGE-M3" in content
        assert "explicitly doing embedding-quality" in content

    def test_tree_skill_install_is_idempotent(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_codex_skills()
            result = install_codex_skills()

        installed = [path.name for path in result.iterdir() if path.is_dir()]
        assert sorted(installed) == sorted(path[:-3] for path in EXPECTED_SKILLS)

    def test_reinstall_updates_existing_tree_skill_content(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_codex_skills()
            target = result / "writing-markdown-document" / "SKILL.md"
            target.write_text("stale skill content", encoding="utf-8")
            install_codex_skills()

        assert target.read_text(encoding="utf-8") != "stale skill content"
        assert "writing-markdown-document" in target.read_text(encoding="utf-8")

    def test_reinstall_replaces_managed_tree_skill_directory(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_codex_skills()
            managed_dir = result / "writing-markdown-document"
            stale_file = managed_dir / "removed-old-file.txt"
            stale_file.write_text("old", encoding="utf-8")
            user_skill = result / "user-skill" / "SKILL.md"
            user_skill.parent.mkdir(parents=True)
            user_skill.write_text("user", encoding="utf-8")

            install_codex_skills()

        assert not stale_file.exists()
        assert user_skill.read_text(encoding="utf-8") == "user"

    def test_tree_skill_install_includes_markdown_code_traceability_guidance(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_codex_skills()

        writing = (result / "writing-markdown-document" / "SKILL.md").read_text()
        reading = (result / "reading-markdown-document" / "SKILL.md").read_text()

        assert "Markdown ↔ code documentation links" in writing
        assert 'query_graph_tool(pattern="implementations_of"' in reading
        assert 'query_graph_tool(pattern="docs_for"' in reading


class TestInstallQoderSkills:
    def test_renders_embedding_context(self, tmp_path):
        skills_dir = tmp_path / "skills" / "semantic-search"
        skills_dir.mkdir(parents=True)
        (skills_dir / "SKILL.md").write_text(
            "---\nname: semantic-search\ndescription: x\n---\n\n"
            "<!-- dagayn skill embedding context -->\n"
            "stale\n"
            "<!-- /dagayn skill embedding context -->\n",
            encoding="utf-8",
        )

        result = install_qoder_skills(
            tmp_path,
            embedding_mode="remote-embedding",
            embedding_provider="google",
        )

        assert result is not None
        content = (result / "semantic-search" / "SKILL.md").read_text()
        assert "--mode remote-embedding --provider google" in content
        assert 'embed_graph_tool(provider="google")' in content

    def test_reinstall_updates_existing_qoder_skill_content(self, tmp_path):
        skills_dir = tmp_path / "skills" / "sample"
        skills_dir.mkdir(parents=True)
        (skills_dir / "SKILL.md").write_text("fresh", encoding="utf-8")
        result = install_qoder_skills(tmp_path)
        assert result is not None
        target = result / "sample" / "SKILL.md"
        target.write_text("stale", encoding="utf-8")

        install_qoder_skills(tmp_path)

        assert target.read_text(encoding="utf-8") == "fresh"

    def test_reinstall_replaces_managed_qoder_skill_directory(self, tmp_path):
        skills_dir = tmp_path / "skills" / "sample"
        skills_dir.mkdir(parents=True)
        (skills_dir / "SKILL.md").write_text("fresh", encoding="utf-8")
        result = install_qoder_skills(tmp_path)
        assert result is not None
        stale_file = result / "sample" / "removed-old-file.txt"
        stale_file.write_text("old", encoding="utf-8")
        user_skill = result / "user-skill" / "SKILL.md"
        user_skill.parent.mkdir(parents=True)
        user_skill.write_text("user", encoding="utf-8")

        install_qoder_skills(tmp_path)

        assert not stale_file.exists()
        assert user_skill.read_text(encoding="utf-8") == "user"


class TestGenerateHooksConfig:
    def test_returns_dict_with_hooks(self):
        config = generate_hooks_config(Path("/repo"))
        assert "hooks" in config

    def test_has_post_tool_use(self):
        config = generate_hooks_config(Path("/repo"))
        assert "PostToolUse" in config["hooks"]
        entry = config["hooks"]["PostToolUse"][0]
        # Bash is deliberately excluded: it fires on shell commands that touch
        # no tracked file, which re-diffed the graph for nothing.
        assert entry["matcher"] == "Edit|Write"
        inner = entry["hooks"][0]
        assert inner["type"] == "command"
        assert "update" in inner["command"]
        # Above dagayn's own budget so the process reports why it stopped
        # instead of being abandoned by the editor.
        assert inner["timeout"] == DEFAULT_HOOK_BUDGET_SECONDS + 30
        assert inner["timeout"] > DEFAULT_HOOK_BUDGET_SECONDS

    def test_has_session_start(self):
        config = generate_hooks_config(Path("/repo"))
        assert "SessionStart" in config["hooks"]
        entry = config["hooks"]["SessionStart"][0]
        assert "matcher" in entry
        inner = entry["hooks"][0]
        assert inner["type"] == "command"
        assert "session prepare" in inner["command"]
        assert inner["timeout"] == 60

    def test_does_not_emit_invalid_pre_commit_hook(self):
        config = generate_hooks_config(Path("/repo"))
        assert "PreCommit" not in config["hooks"]


class TestHookEmbeddingArgs:
    """Embedding flags belong to ``session prepare``, not to edit-triggered updates.

    Re-embedding on every single file edit is far too expensive for a hook that
    fires continuously; the session-start prepare covers it instead.
    """

    def _commands(self, extra):
        config = generate_hooks_config(Path("/repo"), extra_update_args=extra)
        update = config["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        prepare = config["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        return update, prepare

    def test_update_hook_omits_embedding_flags(self):
        update, _ = self._commands(["--local-embedding", "low"])
        assert "--local-embedding" not in update
        assert "dagayn update --skip-flows" in update

    def test_session_start_keeps_embedding_flags(self):
        _, prepare = self._commands(["--local-embedding", "low"])
        assert "--local-embedding low" in prepare

    def test_worktree_hook_keeps_embedding_flags(self):
        config = generate_hooks_config(
            Path("/repo"),
            extra_update_args=["--local-embedding", "low"],
        )
        worktree = config["hooks"]["PostToolUse"][1]["hooks"][0]["command"]
        assert "session prepare" in worktree
        assert "--local-embedding low" in worktree

    def test_update_hook_arms_the_budget_watchdog(self):
        update, _ = self._commands(None)
        # Without this env var dagayn would not bound itself, and neither
        # editor kills the process it started.
        assert "DAGAYN_HOOK_UPDATE=1" in update

    def test_cursor_update_hook_omits_embedding_flags(self):
        scripts = _cursor_hook_scripts(["--local-embedding", "low"])
        assert "--local-embedding" not in scripts["crg-update.sh"]
        # Detached, so dagayn's own watchdog is the only thing that can stop it.
        assert "DAGAYN_HOOK_UPDATE=1" in scripts["crg-update.sh"]
        assert "--local-embedding" not in scripts["crg-pre-commit.sh"]
        assert "DAGAYN_HOOK_UPDATE=1" in scripts["crg-pre-commit.sh"]
        assert "--local-embedding low" in scripts["crg-session-start.sh"]
        assert "--local-embedding low" in scripts["crg-relocate.sh"]

    def test_shared_hook_scripts_split_the_flags(self):
        scripts = _dagayn_hook_scripts(["--local-embedding", "low"])
        assert "--local-embedding" not in scripts["dagayn-update.sh"]
        assert "DAGAYN_HOOK_UPDATE=1" in scripts["dagayn-update.sh"]
        assert "--local-embedding low" in scripts["dagayn-status.sh"]

    def test_opencode_plugin_splits_the_flags(self):
        content = _opencode_plugin_content(["--local-embedding", "low"])
        assert "__DAGAYN_PREPARE_ARGS__" not in content
        assert "__DAGAYN_UPDATE_ARGS__" not in content
        assert "DAGAYN_HOOK_UPDATE=1 dagayn update --skip-flows" in content
        assert "session prepare --budget-seconds 45 --local-embedding low" in content

    def test_no_embedding_args_leaves_commands_clean(self):
        update, prepare = self._commands(None)
        assert "--local-embedding" not in update
        assert "--local-embedding" not in prepare
        assert "--budget-seconds 45 --repo" in prepare

    def test_has_only_valid_hook_types(self):
        config = generate_hooks_config(Path("/repo"))
        hook_types = set(config["hooks"].keys())
        assert hook_types == {"PostToolUse", "SessionStart"}

    def test_hook_entries_use_nested_hooks_array(self):
        config = generate_hooks_config(Path("/repo"))
        for hook_type, entries in config["hooks"].items():
            for entry in entries:
                assert "hooks" in entry, f"{hook_type} entry missing 'hooks' array"
                assert "command" not in entry, f"{hook_type} has bare 'command' outside hooks[]"

    def test_repo_root_resolved_at_hook_runtime(self):
        config = generate_hooks_config(Path("/my/project"))
        post_cmd = config["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        session_cmd = config["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        assert "/my/project" not in post_cmd
        assert "/my/project" not in session_cmd
        assert "git -C" not in post_cmd
        assert "git -C" not in session_cmd
        assert 'repo="$(git rev-parse --show-toplevel 2>/dev/null)"' in post_cmd
        assert 'repo="$(git rev-parse --show-toplevel 2>/dev/null)"' in session_cmd
        assert '--repo "$repo"' in post_cmd
        assert '--repo "$repo"' in session_cmd

    def test_quotes_repo_paths_with_spaces(self):
        config = generate_hooks_config(Path("/repo with spaces"))
        post_cmd = config["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        assert "/repo with spaces" not in post_cmd
        assert '--repo "$repo"' in post_cmd

    def test_extra_update_args_reach_prepare_not_the_update_hook(self):
        config = generate_hooks_config(
            Path("/repo"),
            extra_update_args=["--local-embedding", "low"],
        )
        post_cmd = config["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        start_cmd = config["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        assert 'dagayn update --skip-flows --repo "$repo"' in post_cmd
        assert "--local-embedding" not in post_cmd
        assert "--local-embedding low" in start_cmd

    def test_entries_use_claude_code_hook_schema(self):
        """Regression guard for the Claude Code hook schema.

        Claude Code rejects entries that put ``command`` directly on the
        event entry. Each entry must wrap its command(s) in a
        ``hooks: [{"type": "command", "command": ..., "timeout": ...}]``
        array — missing that wrapper causes the entire settings.json to
        fail to parse ("Expected array, but received undefined").
        """
        config = generate_hooks_config(Path("/repo"))
        for event_name, entries in config["hooks"].items():
            for entry in entries:
                assert "command" not in entry, (
                    f"{event_name} entry has a flat `command` field; "
                    "it must be wrapped in an inner `hooks` array"
                )
                assert "hooks" in entry, f"{event_name} entry is missing the inner `hooks` array"
                assert isinstance(entry["hooks"], list)
                for hook in entry["hooks"]:
                    assert hook.get("type") == "command", (
                        f'{event_name} inner hook missing type="command"'
                    )
                    assert "command" in hook
                    assert "timeout" in hook


class TestInstallGitHook:
    def _make_git_repo(self, tmp_path: Path) -> Path:
        (tmp_path / ".git" / "hooks").mkdir(parents=True)
        return tmp_path

    def test_creates_executable_pre_commit_hook(self, tmp_path):
        hook_path = install_git_hook(self._make_git_repo(tmp_path))
        assert hook_path is not None and hook_path.name == "pre-commit"
        assert os.access(hook_path, os.X_OK)
        content = hook_path.read_text()
        assert content.startswith("#!/")
        assert "dagayn update --skip-flows" in content
        assert "dagayn detect-changes" in content

    def test_creates_executable_post_commit_hook(self, tmp_path):
        repo = self._make_git_repo(tmp_path)
        install_git_hook(repo)
        hook_path = repo / ".git" / "hooks" / "post-commit"
        assert hook_path.exists()
        assert os.access(hook_path, os.X_OK)
        content = hook_path.read_text()
        assert "dagayn update || true" in content
        assert "dagayn detect-changes" not in content

    def test_appends_to_existing_hook(self, tmp_path):
        repo = self._make_git_repo(tmp_path)
        hook_path = repo / ".git" / "hooks" / "pre-commit"
        hook_path.write_text("#!/bin/sh\nexisting-command\n", encoding="utf-8")
        hook_path.chmod(0o755)
        install_git_hook(repo)
        content = hook_path.read_text()
        assert "existing-command" in content
        assert "dagayn detect-changes" in content

    def test_idempotent(self, tmp_path):
        repo = self._make_git_repo(tmp_path)
        install_git_hook(repo)
        install_git_hook(repo)
        content = (repo / ".git" / "hooks" / "pre-commit").read_text()
        assert content.count("dagayn detect-changes") == 1
        post_content = (repo / ".git" / "hooks" / "post-commit").read_text()
        assert post_content.count("dagayn update || true") == 1

    def test_reinstall_preserves_executable_bit(self, tmp_path):
        repo = self._make_git_repo(tmp_path)
        install_git_hook(repo)
        pre_commit = repo / ".git" / "hooks" / "pre-commit"
        pre_commit.chmod(0o644)
        install_git_hook(repo)
        assert os.access(pre_commit, os.X_OK)

    def test_replaces_legacy_pre_commit_full_update_block(self, tmp_path):
        repo = self._make_git_repo(tmp_path)
        hook_path = repo / ".git" / "hooks" / "pre-commit"
        hook_path.write_text(
            "#!/bin/sh\n"
            "existing-command\n"
            "# Installed by dagayn. Remove this file to disable pre-commit graph checks.\n"
            "if command -v dagayn >/dev/null 2>&1; then\n"
            "    dagayn update || true\n"
            "    dagayn detect-changes --brief || true\n"
            "fi\n",
            encoding="utf-8",
        )
        install_git_hook(repo)
        content = hook_path.read_text()
        assert "existing-command" in content
        assert "dagayn update --skip-flows" in content
        assert content.count("dagayn detect-changes") == 1
        assert "    dagayn update || true" not in content

    def test_no_git_dir_returns_none(self, tmp_path):
        assert install_git_hook(tmp_path) is None


class TestInstallHooks:
    def test_creates_settings_file(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            settings_path = install_hooks(tmp_path)
        assert settings_path == tmp_path / ".claude" / "settings.json"
        assert settings_path.exists()
        data = json.loads(settings_path.read_text())
        assert "hooks" in data

    def test_merges_with_existing(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            settings_dir = tmp_path / ".claude"
            settings_dir.mkdir(parents=True)
            existing = {"customSetting": True, "hooks": {"OtherHook": []}}
            (settings_dir / "settings.json").write_text(json.dumps(existing))

            install_hooks(tmp_path)

        data = json.loads((settings_dir / "settings.json").read_text())
        assert data["customSetting"] is True
        assert "OtherHook" in data["hooks"]
        assert "PostToolUse" in data["hooks"]
        assert "SessionStart" in data["hooks"]
        assert "PreCommit" not in data["hooks"]
        assert "OtherHook" in data["hooks"]  # pre-existing hooks must not be clobbered

    def test_creates_settings_backup(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            settings_dir = tmp_path / ".claude"
            settings_dir.mkdir(parents=True)
            existing = {"hooks": {"OtherHook": []}}
            (settings_dir / "settings.json").write_text(json.dumps(existing))

            install_hooks(tmp_path)

        backup_path = settings_dir / "settings.json.bak"
        assert backup_path.exists()
        backup = json.loads(backup_path.read_text())
        assert backup == existing

    def test_creates_claude_directory(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_hooks(tmp_path)
        assert (tmp_path / ".claude").is_dir()

    def test_claude_hooks_are_global_not_project_local(self, tmp_path):
        repo = tmp_path / "repo"
        home = tmp_path / "home"
        repo.mkdir()
        with patch("dagayn.skills.Path.home", return_value=home):
            settings_path = install_hooks(repo)

        assert settings_path == home / ".claude" / "settings.json"
        assert not (repo / ".claude" / "settings.json").exists()

    def test_passes_extra_update_args_to_claude_hooks(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            settings_path = install_hooks(
                tmp_path / "repo",
                extra_update_args=["--local-embedding", "low"],
            )

        data = json.loads(settings_path.read_text())
        # Embedding flags land on session prepare; the edit-triggered update
        # stays structure-only.
        update = data["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        prepare = data["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        assert "--local-embedding" not in update
        assert "--local-embedding low" in prepare

    def test_reinstall_replaces_legacy_seed_only_hooks(self, tmp_path):
        """A pre-4.8.3 install left seed-only hooks that must not survive."""
        settings_dir = tmp_path / ".claude"
        settings_dir.mkdir(parents=True)
        legacy = {
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": "",
                        "hooks": [
                            {
                                "type": "command",
                                "command": 'dagayn status --repo "$repo"',
                                "timeout": 10,
                            }
                        ],
                    }
                ],
                "PostToolUse": [
                    {
                        "matcher": "EnterWorktree|ExitWorktree",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "dagayn worktree sync --from-hook || true",
                                "timeout": 300,
                            }
                        ],
                    }
                ],
            }
        }
        (settings_dir / "settings.json").write_text(json.dumps(legacy))

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_hooks(tmp_path / "repo")

        data = json.loads((settings_dir / "settings.json").read_text())
        commands = [
            hook.get("command", "")
            for entries in data["hooks"].values()
            for entry in entries
            for hook in entry["hooks"]
        ]
        assert not any("dagayn status" in command for command in commands)
        assert not any("dagayn worktree sync" in command for command in commands)
        session_commands = [
            hook["command"] for entry in data["hooks"]["SessionStart"] for hook in entry["hooks"]
        ]
        assert len(session_commands) == 1
        assert "dagayn session prepare" in session_commands[0]

    def test_reinstall_updates_claude_hooks_when_extra_args_change(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            settings_path = install_hooks(tmp_path / "repo")
            install_hooks(
                tmp_path / "repo",
                extra_update_args=["--local-embedding", "low"],
            )

        data = json.loads(settings_path.read_text())
        post_tool_hooks = data["hooks"]["PostToolUse"]
        dagayn_hooks = [
            entry
            for entry in post_tool_hooks
            if any(
                "dagayn update --skip-flows" in hook.get("command", "") for hook in entry["hooks"]
            )
        ]
        assert len(dagayn_hooks) == 1
        # The entry is replaced rather than duplicated; embedding flags moved to
        # the session-start prepare.
        assert "--local-embedding" not in dagayn_hooks[0]["hooks"][0]["command"]
        session_cmd = data["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        assert "--local-embedding low" in session_cmd

    def test_install_qoder_hooks(self, tmp_path):
        install_hooks(tmp_path, platform="qoder")
        settings_path = tmp_path / ".qoder" / "settings.json"
        assert settings_path.exists()
        data = json.loads(settings_path.read_text())
        assert "hooks" in data
        assert "PostToolUse" in data["hooks"]
        assert "SessionStart" in data["hooks"]

    def test_install_qoder_hooks_merges_existing(self, tmp_path):
        settings_dir = tmp_path / ".qoder"
        settings_dir.mkdir(parents=True)
        existing = {"customSetting": True}
        (settings_dir / "settings.json").write_text(json.dumps(existing))

        install_hooks(tmp_path, platform="qoder")

        data = json.loads((settings_dir / "settings.json").read_text())
        assert data["customSetting"] is True
        assert "hooks" in data


class TestInstallCodexHooks:
    def test_creates_hooks_json_and_enables_feature(self, tmp_path):
        repo = tmp_path / "repo"
        home = tmp_path / "home"
        repo.mkdir()
        with patch("dagayn.skills.Path.home", return_value=home):
            hooks_path = install_codex_hooks(repo)

        assert hooks_path == home / ".codex" / "hooks.json"
        assert not (repo / ".codex" / "hooks.json").exists()
        data = json.loads(hooks_path.read_text())
        assert "PostToolUse" in data["hooks"]
        assert "SessionStart" in data["hooks"]

        config = tomllib.loads((home / ".codex" / "config.toml").read_text())
        assert config["features"]["hooks"] is True

    def test_merges_existing_hooks_json(self, tmp_path):
        codex_dir = tmp_path / ".codex"
        codex_dir.mkdir()
        existing = {"hooks": {"Stop": []}, "customSetting": True}
        (codex_dir / "hooks.json").write_text(json.dumps(existing), encoding="utf-8")

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_codex_hooks(tmp_path / "repo")

        data = json.loads((codex_dir / "hooks.json").read_text())
        assert data["customSetting"] is True
        assert "Stop" in data["hooks"]
        assert "PostToolUse" in data["hooks"]
        assert "SessionStart" in data["hooks"]
        assert json.loads((codex_dir / "hooks.json.bak").read_text()) == existing

    def test_preserves_existing_codex_config_toml(self, tmp_path):
        codex_dir = tmp_path / ".codex"
        codex_dir.mkdir()
        (codex_dir / "config.toml").write_text(
            'model = "gpt-5.4"\n\n[mcp_servers.other]\ncommand = "other"\n',
            encoding="utf-8",
        )

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_codex_hooks(tmp_path / "repo")

        config = tomllib.loads((codex_dir / "config.toml").read_text())
        assert config["model"] == "gpt-5.4"
        assert config["mcp_servers"]["other"]["command"] == "other"
        assert config["features"]["hooks"] is True

    def test_migrates_deprecated_codex_hooks_feature(self, tmp_path):
        codex_dir = tmp_path / ".codex"
        codex_dir.mkdir()
        (codex_dir / "config.toml").write_text(
            "[features]\ncodex_hooks = true\n",
            encoding="utf-8",
        )

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_codex_hooks(tmp_path / "repo")

        config_text = (codex_dir / "config.toml").read_text()
        config = tomllib.loads(config_text)
        assert config["features"]["hooks"] is True
        assert "codex_hooks" not in config_text

    def test_no_duplicate_on_reinstall(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_codex_hooks(tmp_path / "repo")
            install_codex_hooks(tmp_path / "repo")

        data = json.loads((tmp_path / ".codex" / "hooks.json").read_text())
        for entries in data["hooks"].values():
            dagayn_hooks = [
                entry
                for entry in entries
                if any("dagayn" in hook.get("command", "") for hook in entry.get("hooks", []))
            ]
            assert len(dagayn_hooks) == 1
        assert (tmp_path / ".codex" / "config.toml").read_text().count("hooks = true") == 1

    def test_passes_extra_update_args_to_codex_hooks(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            hooks_path = install_codex_hooks(
                tmp_path / "repo",
                extra_update_args=["--local-embedding", "low"],
            )

        data = json.loads(hooks_path.read_text())
        command = data["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        assert "DAGAYN_HOOK_UPDATE=1" in command
        assert "--local-embedding" not in command
        assert "--local-embedding low" in data["hooks"]["SessionStart"][0]["hooks"][0]["command"]

    def test_reinstall_updates_codex_hooks_when_extra_args_change(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            hooks_path = install_codex_hooks(tmp_path / "repo")
            install_codex_hooks(
                tmp_path / "repo",
                extra_update_args=["--local-embedding", "low"],
            )

        data = json.loads(hooks_path.read_text())
        post_tool_hooks = data["hooks"]["PostToolUse"]
        dagayn_hooks = [
            entry
            for entry in post_tool_hooks
            if any(
                "dagayn update --skip-flows" in hook.get("command", "") for hook in entry["hooks"]
            )
        ]
        assert len(dagayn_hooks) == 1
        # The entry is replaced rather than duplicated; embedding flags moved to
        # the session-start prepare.
        assert "--local-embedding" not in dagayn_hooks[0]["hooks"][0]["command"]
        session_cmd = data["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        assert "--local-embedding low" in session_cmd

    def test_generate_hermes_hooks_config(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            config = generate_hermes_hooks_config()

        assert "post_tool_call" in config
        assert config["post_tool_call"][0]["matcher"] == "terminal|write_file|patch"
        assert str(tmp_path / ".hermes" / "agent-hooks") in config["post_tool_call"][0]["command"]
        assert "on_session_start" in config

    def test_install_hermes_hooks_merges_existing_config(self, tmp_path):
        hermes_dir = tmp_path / ".hermes"
        hermes_dir.mkdir()
        (hermes_dir / "config.yaml").write_text(
            "model:\n  default: local\nhooks:\n  post_tool_call:\n    - command: other.sh\n",
            encoding="utf-8",
        )

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            hooks_path = install_hermes_hooks(extra_update_args=["--local-embedding", "low"])

        assert hooks_path == hermes_dir / "config.yaml"
        data = yaml.safe_load(hooks_path.read_text())
        assert data["model"]["default"] == "local"
        commands = [entry["command"] for entry in data["hooks"]["post_tool_call"]]
        assert "other.sh" in commands
        assert any("dagayn-update.sh" in command for command in commands)
        script = hermes_dir / "agent-hooks" / "dagayn-update.sh"
        assert script.exists()
        assert os.access(script, os.X_OK)
        # Structure only here; the embedding flags belong to session prepare.
        assert "--local-embedding" not in script.read_text()
        status = hermes_dir / "agent-hooks" / "dagayn-status.sh"
        assert "--local-embedding low" in status.read_text()

    def test_reinstall_hermes_hooks_deduplicates_dagayn_entries(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            hooks_path = install_hermes_hooks()
            install_hermes_hooks()

        data = yaml.safe_load(hooks_path.read_text())
        commands = [entry["command"] for entry in data["hooks"]["post_tool_call"]]
        assert len([command for command in commands if "dagayn-update.sh" in command]) == 1

    def test_generate_pi_hooks_config(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            hooks = generate_pi_hooks_config()

        assert hooks[0]["event"] == "file.changed"
        assert str(tmp_path / ".pi" / "agent" / "hook") in hooks[0]["actions"][0]["bash"]
        assert hooks[1]["event"] == "session.created"

    def test_install_pi_hooks_merges_existing_config(self, tmp_path):
        hook_dir = tmp_path / ".pi" / "agent" / "hook"
        hook_dir.mkdir(parents=True)
        (hook_dir / "hooks.yaml").write_text(
            "hooks:\n  - event: session.idle\n    actions:\n      - notify: idle\n",
            encoding="utf-8",
        )

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            hooks_path = install_pi_hooks(extra_update_args=["--remote-embedding", "google"])

        assert hooks_path == hook_dir / "hooks.yaml"
        data = yaml.safe_load(hooks_path.read_text())
        assert any(entry["event"] == "session.idle" for entry in data["hooks"])
        assert any(entry["event"] == "file.changed" for entry in data["hooks"])
        script = hook_dir / "dagayn-update.sh"
        assert script.exists()
        assert os.access(script, os.X_OK)
        # Structure only here; the embedding flags belong to session prepare.
        assert "--remote-embedding" not in script.read_text()
        status = hook_dir / "dagayn-status.sh"
        assert "--remote-embedding google" in status.read_text()

    def test_reinstall_pi_hooks_deduplicates_dagayn_entries(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            hooks_path = install_pi_hooks()
            install_pi_hooks()

        data = yaml.safe_load(hooks_path.read_text())
        update_entries = [
            entry
            for entry in data["hooks"]
            if any(
                isinstance(action, dict) and "dagayn-update.sh" in action.get("bash", "")
                for action in entry.get("actions", [])
            )
        ]
        assert len(update_entries) == 1


class TestInjectClaudeMd:
    def test_has_instruction_section_accepts_markers_and_heading_aliases(self):
        assert _has_instruction_section(_CLAUDE_MD_SECTION_MARKER, _CLAUDE_MD_SECTION_MARKER)
        assert _has_instruction_section(_CLAUDE_MD_SECTION_HEADING, _CLAUDE_MD_SECTION_MARKER)
        assert _has_instruction_section(_MARKDOWN_POLICY_MARKER, _MARKDOWN_POLICY_MARKER)
        assert _has_instruction_section(
            _MARKDOWN_POLICY_HEADING,
            _MARKDOWN_POLICY_MARKER,
        )
        assert _has_instruction_section(
            "## Markdown documentation policy\nBody",
            _MARKDOWN_POLICY_MARKER,
        )
        assert not _has_instruction_section("plain text", _CLAUDE_MD_SECTION_MARKER)

    def test_creates_section_in_new_file(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)
        content = (tmp_path / ".claude" / "CLAUDE.md").read_text()
        assert _CLAUDE_MD_SECTION_MARKER in content
        assert "MCP Tools" in content
        assert "get_minimal_context_tool" in content
        assert "How to judge analysis output" in content
        assert "truncated" in content
        assert "--tools" in content
        assert "--tool-profile" not in content
        assert "analysis_summary" in content
        assert "architecture_health" in content
        assert "architecture_analysis_tool" in content
        assert "review_tool" in content
        assert "flow_tool" in content
        assert "get_architecture_overview" not in content
        for legacy_name in LEGACY_MCP_TOOL_NAMES:
            assert legacy_name not in content

    def test_appends_to_existing_file(self, tmp_path):
        claude_md = tmp_path / ".claude" / "CLAUDE.md"
        claude_md.parent.mkdir(parents=True)
        claude_md.write_text("# My Project\n\nExisting content.\n")

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)

        content = claude_md.read_text()
        assert "# My Project" in content
        assert "Existing content." in content
        assert _CLAUDE_MD_SECTION_MARKER in content

    def test_idempotent(self, tmp_path):
        """Running twice should not duplicate the section."""
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)
        first_content = (tmp_path / ".claude" / "CLAUDE.md").read_text()

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)
        second_content = (tmp_path / ".claude" / "CLAUDE.md").read_text()

        assert first_content == second_content
        assert second_content.count(_CLAUDE_MD_SECTION_MARKER) == 1

    def test_idempotent_with_existing_content(self, tmp_path):
        claude_md = tmp_path / ".claude" / "CLAUDE.md"
        claude_md.parent.mkdir(parents=True)
        claude_md.write_text("# Existing\n")

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)
        first_content = claude_md.read_text()

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)
        second_content = claude_md.read_text()

        assert first_content == second_content
        assert second_content.count(_CLAUDE_MD_SECTION_MARKER) == 1

    def test_also_injects_markdown_policy(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)
        content = (tmp_path / ".claude" / "CLAUDE.md").read_text()
        assert _MARKDOWN_POLICY_MARKER in content
        assert "constrained-by" in content

    def test_policy_idempotent(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)
        first = (tmp_path / ".claude" / "CLAUDE.md").read_text()
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)
        second = (tmp_path / ".claude" / "CLAUDE.md").read_text()
        assert first == second
        assert second.count(_MARKDOWN_POLICY_MARKER) == 1

    def test_existing_sections_without_markers_are_normalized_not_duplicated(self, tmp_path):
        claude_md = tmp_path / ".claude" / "CLAUDE.md"
        claude_md.parent.mkdir(parents=True)
        claude_md.write_text(
            "\n".join(
                [
                    _CLAUDE_MD_SECTION_HEADING,
                    "Use dagayn tools.",
                    "",
                    _MARKDOWN_POLICY_HEADING,
                    "Use dependency directives.",
                    "",
                ]
            ),
            encoding="utf-8",
        )

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)

        content = claude_md.read_text(encoding="utf-8")
        assert content.count(_CLAUDE_MD_SECTION_HEADING) == 1
        assert content.count(_MARKDOWN_POLICY_HEADING) == 1
        assert content.count(_CLAUDE_MD_SECTION_MARKER) == 1
        assert content.count(_MARKDOWN_POLICY_MARKER) == 1

    def test_policy_appended_to_existing_mcp_section(self, tmp_path):
        """Re-install onto a file that already has the MCP tools section adds the policy."""
        claude_md = tmp_path / ".claude" / "CLAUDE.md"
        claude_md.parent.mkdir(parents=True)
        claude_md.write_text(f"{_CLAUDE_MD_SECTION_MARKER}\n## MCP Tools: dagayn\n(existing)\n")

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)

        content = claude_md.read_text()
        assert content.count(_CLAUDE_MD_SECTION_MARKER) == 1
        assert _MARKDOWN_POLICY_MARKER in content

    def test_permission_error_is_reported_without_raising(self, tmp_path):
        errors: list[str] = []
        with (
            patch("dagayn.skills.Path.home", return_value=tmp_path),
            patch("pathlib.Path.write_text", side_effect=PermissionError("read-only")),
        ):
            updated = inject_claude_md(tmp_path, errors=errors)

        assert updated == []
        assert errors
        assert "read-only" in errors[0]


class TestInstructionFilesToModify:
    def test_claude_preview_uses_global_claude_md(self, tmp_path):
        from dagayn.cli.commands.init import _instruction_files_to_modify

        with patch("dagayn.cli.commands.init.Path.home", return_value=tmp_path):
            targets = _instruction_files_to_modify(tmp_path, "claude")

        assert targets == ["~/.claude/CLAUDE.md (new)"]

    def test_codex_preview_uses_global_agents_md(self, tmp_path):
        from dagayn.cli.commands.init import _instruction_files_to_modify

        with patch("dagayn.cli.commands.init.Path.home", return_value=tmp_path):
            targets = _instruction_files_to_modify(tmp_path, "codex")

        assert targets == ["~/.codex/AGENTS.md (new)"]

    def test_opencode_preview_uses_global_agents_md(self, tmp_path):
        from dagayn.cli.commands.init import _instruction_files_to_modify

        with patch("dagayn.cli.commands.init.Path.home", return_value=tmp_path):
            targets = _instruction_files_to_modify(tmp_path, "opencode")

        assert targets == ["~/.config/opencode/AGENTS.md (new)"]

    def test_all_preview_includes_global_agents_md_targets(self, tmp_path):
        from dagayn.cli.commands.init import _instruction_files_to_modify

        with patch("dagayn.cli.commands.init.Path.home", return_value=tmp_path):
            targets = _instruction_files_to_modify(tmp_path, "all")

        assert "~/.codex/AGENTS.md (new)" in targets
        assert "~/.config/opencode/AGENTS.md (new)" in targets

    def test_existing_writable_file_is_previewed_as_append(self, tmp_path):
        from dagayn.cli.commands.init import _instruction_files_to_modify

        claude_md = tmp_path / ".claude" / "CLAUDE.md"
        claude_md.parent.mkdir(parents=True)
        claude_md.write_text("# mine\n", encoding="utf-8")

        with patch("dagayn.cli.commands.init.Path.home", return_value=tmp_path):
            targets = _instruction_files_to_modify(tmp_path, "claude")

        assert targets == ["~/.claude/CLAUDE.md (append)"]

    def test_read_only_file_is_previewed_as_skipped(self, tmp_path):
        """home-manager/chezmoi symlinks cannot be injected — say so up front.

        Promising the injection and then reporting it as skipped further down
        the output is what actually confused a user.
        """
        from dagayn.cli.commands.init import _instruction_files_to_modify

        claude_md = tmp_path / ".claude" / "CLAUDE.md"
        claude_md.parent.mkdir(parents=True)
        claude_md.write_text("# managed elsewhere\n", encoding="utf-8")
        claude_md.chmod(0o444)

        try:
            with patch("dagayn.cli.commands.init.Path.home", return_value=tmp_path):
                targets = _instruction_files_to_modify(tmp_path, "claude")
        finally:
            claude_md.chmod(0o644)

        assert targets == ["~/.claude/CLAUDE.md (read-only — will be skipped)"]


class TestInjectPlatformInstructionsFiltering:
    def test_all_writes_every_file(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            updated = inject_platform_instructions(tmp_path, target="all")
        assert set(updated) == {
            "AGENTS.md",
            "GEMINI.md",
            ".cursorrules",
            ".windsurfrules",
            "QODER.md",
            ".kiro/steering/dagayn.md",
        }
        assert (tmp_path / ".codex" / "AGENTS.md").exists()
        assert (tmp_path / ".config" / "opencode" / "AGENTS.md").exists()

    def test_default_is_all(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            updated = inject_platform_instructions(tmp_path)
        assert set(updated) == {
            "AGENTS.md",
            "GEMINI.md",
            ".cursorrules",
            ".windsurfrules",
            "QODER.md",
            ".kiro/steering/dagayn.md",
        }
        assert (tmp_path / ".codex" / "AGENTS.md").exists()
        assert (tmp_path / ".config" / "opencode" / "AGENTS.md").exists()

    def test_claude_writes_nothing(self, tmp_path):
        updated = inject_platform_instructions(tmp_path, target="claude")
        assert updated == []
        assert not (tmp_path / "AGENTS.md").exists()
        assert not (tmp_path / ".codex" / "AGENTS.md").exists()
        assert not (tmp_path / ".config" / "opencode" / "AGENTS.md").exists()
        assert not (tmp_path / "GEMINI.md").exists()
        assert not (tmp_path / ".cursorrules").exists()
        assert not (tmp_path / ".windsurfrules").exists()
        assert not (tmp_path / "QODER.md").exists()

    def test_cursor_writes_only_cursor_files(self, tmp_path):
        updated = inject_platform_instructions(tmp_path, target="cursor")
        assert set(updated) == {"AGENTS.md", ".cursorrules"}
        assert not (tmp_path / "GEMINI.md").exists()
        assert not (tmp_path / ".windsurfrules").exists()
        assert not (tmp_path / "QODER.md").exists()

    def test_windsurf_writes_only_windsurfrules(self, tmp_path):
        updated = inject_platform_instructions(tmp_path, target="windsurf")
        assert updated == [".windsurfrules"]

    def test_antigravity_writes_agents_and_gemini(self, tmp_path):
        updated = inject_platform_instructions(tmp_path, target="antigravity")
        assert set(updated) == {"AGENTS.md", "GEMINI.md"}

    def test_opencode_writes_only_agents(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            updated = inject_platform_instructions(tmp_path, target="opencode")
        assert updated == ["AGENTS.md"]
        assert (tmp_path / ".config" / "opencode" / "AGENTS.md").exists()
        assert not (tmp_path / "AGENTS.md").exists()

    def test_codex_writes_only_agents(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            updated = inject_platform_instructions(tmp_path, target="codex")
        assert updated == ["AGENTS.md"]
        assert (tmp_path / ".codex" / "AGENTS.md").exists()
        assert not (tmp_path / "AGENTS.md").exists()

    def test_qcoder_alias_writes_only_qoder_md(self, tmp_path):
        updated = inject_platform_instructions(tmp_path, target="qcoder")
        assert updated == ["QODER.md"]

    def test_qoder_writes_only_qoder_md(self, tmp_path):
        updated = inject_platform_instructions(tmp_path, target="qoder")
        assert updated == ["QODER.md"]
        assert not (tmp_path / "AGENTS.md").exists()
        assert not (tmp_path / "GEMINI.md").exists()
        assert not (tmp_path / ".cursorrules").exists()
        assert not (tmp_path / ".windsurfrules").exists()

    def test_files_contain_markdown_policy(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_platform_instructions(tmp_path, target="all")
        for filename in ("GEMINI.md", ".cursorrules", ".windsurfrules", "QODER.md"):
            content = (tmp_path / filename).read_text()
            assert _MARKDOWN_POLICY_MARKER in content, f"{filename} missing policy marker"
        assert _MARKDOWN_POLICY_MARKER in (tmp_path / ".codex" / "AGENTS.md").read_text()
        opencode_agents = tmp_path / ".config" / "opencode" / "AGENTS.md"
        assert _MARKDOWN_POLICY_MARKER in opencode_agents.read_text()

    def test_agents_md_mentions_tool_surface_and_composed_outputs(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_platform_instructions(tmp_path, target="codex")

        content = (tmp_path / ".codex" / "AGENTS.md").read_text()
        assert "--tools" in content
        assert "--tool-profile" not in content
        assert "analysis_summary" in content
        assert "architecture_health" in content
        assert "architecture_analysis_tool" in content
        assert "Drill-down tools" in content

    def test_policy_injected_when_only_mcp_section_exists(self, tmp_path):
        """Existing file with only the MCP section gets the policy section on re-run."""
        agents_md = tmp_path / ".config" / "opencode" / "AGENTS.md"
        agents_md.parent.mkdir(parents=True)
        agents_md.write_text(f"{_CLAUDE_MD_SECTION_MARKER}\n## MCP Tools: dagayn\n(stale)\n")

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            updated = inject_platform_instructions(tmp_path, target="opencode")

        assert updated == ["AGENTS.md"]
        content = agents_md.read_text()
        assert content.count(_CLAUDE_MD_SECTION_MARKER) == 1
        assert _MARKDOWN_POLICY_MARKER in content

    def test_idempotent_with_both_sections(self, tmp_path):
        inject_platform_instructions(tmp_path, target="windsurf")
        first = (tmp_path / ".windsurfrules").read_text()
        updated = inject_platform_instructions(tmp_path, target="windsurf")
        second = (tmp_path / ".windsurfrules").read_text()
        assert updated == []
        assert first == second

    def test_one_failed_instruction_file_does_not_stop_remaining_files(self, tmp_path):
        errors: list[str] = []
        blocked = tmp_path / ".codex" / "AGENTS.md"
        original_write = _skills_module.write_text_atomic

        def write_text_atomic(path, *args, **kwargs):
            if Path(path) == blocked:
                raise PermissionError("read-only")
            return original_write(path, *args, **kwargs)

        with (
            patch("dagayn.skills.Path.home", return_value=tmp_path),
            patch.object(_skills_module, "write_text_atomic", new=write_text_atomic),
        ):
            updated = inject_platform_instructions(tmp_path, target="all", errors=errors)

        assert "AGENTS.md" in updated
        assert (tmp_path / "AGENTS.md").exists()
        assert (tmp_path / ".config" / "opencode" / "AGENTS.md").exists()
        assert (tmp_path / "GEMINI.md").exists()
        assert errors
        assert str(blocked) in errors[0]


class TestInstallPlatformConfigs:
    def test_install_codex_config(self, tmp_path):
        codex_config = tmp_path / ".codex" / "config.toml"
        with patch.dict(
            PLATFORMS,
            {
                "codex": {
                    **PLATFORMS["codex"],
                    "config_path": lambda root: codex_config,
                    "detect": lambda: True,
                },
            },
        ):
            configured = install_platform_configs(tmp_path, target="codex")
        assert "Codex" in configured
        data = tomllib.loads(codex_config.read_text())
        entry = data["mcp_servers"]["dagayn"]
        assert entry["type"] == "stdio"
        assert "serve" in entry["args"]

    def test_install_codex_preserves_existing_toml(self, tmp_path):
        codex_config = tmp_path / ".codex" / "config.toml"
        codex_config.parent.mkdir(parents=True)
        codex_config.write_text(
            'model = "gpt-5.4"\n\n[mcp_servers.other]\ncommand = "other"\n',
            encoding="utf-8",
        )
        with patch.dict(
            PLATFORMS,
            {
                "codex": {
                    **PLATFORMS["codex"],
                    "config_path": lambda root: codex_config,
                    "detect": lambda: True,
                },
            },
        ):
            install_platform_configs(tmp_path, target="codex")
        data = tomllib.loads(codex_config.read_text())
        assert data["model"] == "gpt-5.4"
        assert data["mcp_servers"]["other"]["command"] == "other"
        expected_cmd, _ = _detect_serve_command()
        assert data["mcp_servers"]["dagayn"]["command"] == expected_cmd

    def test_install_codex_no_duplicate(self, tmp_path):
        codex_config = tmp_path / ".codex" / "config.toml"
        codex_config.parent.mkdir(parents=True)
        codex_config.write_text(
            "\n".join(
                [
                    "[mcp_servers.dagayn]",
                    'command = "uvx"',
                    'args = ["dagayn", "serve"]',
                    'type = "stdio"',
                    "",
                ]
            ),
            encoding="utf-8",
        )
        with patch.dict(
            PLATFORMS,
            {
                "codex": {
                    **PLATFORMS["codex"],
                    "config_path": lambda root: codex_config,
                    "detect": lambda: True,
                },
            },
        ):
            install_platform_configs(tmp_path, target="codex")
        assert codex_config.read_text().count("[mcp_servers.dagayn]") == 1

    def test_reinstall_codex_updates_existing_dagayn_args(self, tmp_path):
        codex_config = tmp_path / ".codex" / "config.toml"
        codex_config.parent.mkdir(parents=True)
        codex_config.write_text(
            "\n".join(
                [
                    'model = "gpt-5.4"',
                    "",
                    "[mcp_servers.dagayn]",
                    'command = "uvx"',
                    'args = ["dagayn", "serve"]',
                    'type = "stdio"',
                    "",
                    "[mcp_servers.other]",
                    'command = "other"',
                    "",
                ]
            ),
            encoding="utf-8",
        )
        with patch.dict(
            PLATFORMS,
            {
                "codex": {
                    **PLATFORMS["codex"],
                    "config_path": lambda root: codex_config,
                    "detect": lambda: True,
                },
            },
        ):
            install_platform_configs(
                tmp_path,
                target="codex",
                extra_serve_args=["--local-embedding", "low"],
            )

        data = tomllib.loads(codex_config.read_text())
        assert data["model"] == "gpt-5.4"
        assert data["mcp_servers"]["other"]["command"] == "other"
        assert data["mcp_servers"]["dagayn"]["args"][-2:] == ["--local-embedding", "low"]
        assert codex_config.read_text().count("[mcp_servers.dagayn]") == 1

    def test_install_cursor_config(self, tmp_path, monkeypatch):
        fake_home = tmp_path / "home"
        fake_home.mkdir()
        monkeypatch.setattr("dagayn.skills.Path.home", lambda: fake_home)
        with patch.dict(
            PLATFORMS,
            {
                "cursor": {**PLATFORMS["cursor"], "detect": lambda: True},
            },
        ):
            configured = install_platform_configs(tmp_path, target="cursor")
        assert "Cursor" in configured
        config_path = tmp_path / ".cursor" / "mcp.json"
        assert config_path.exists()
        data = json.loads(config_path.read_text())
        entry = data["mcpServers"]["dagayn"]
        assert entry["type"] == "stdio"
        assert "cwd" not in entry
        assert "--repo" not in entry["args"]

        user_config = fake_home / ".cursor" / "mcp.json"
        assert user_config.exists()
        user_entry = json.loads(user_config.read_text())["mcpServers"]["dagayn"]
        assert "--repo" not in user_entry["args"]
        assert "cwd" not in user_entry

    def test_install_windsurf_config(self, tmp_path):
        windsurf_dir = tmp_path / ".codeium" / "windsurf"
        windsurf_dir.mkdir(parents=True)
        config_path = windsurf_dir / "mcp_config.json"
        with patch.dict(
            PLATFORMS,
            {
                "windsurf": {
                    **PLATFORMS["windsurf"],
                    "config_path": lambda root: config_path,
                    "detect": lambda: True,
                },
            },
        ):
            configured = install_platform_configs(tmp_path, target="windsurf")
        assert "Windsurf" in configured
        data = json.loads(config_path.read_text())
        entry = data["mcpServers"]["dagayn"]
        assert "type" not in entry
        expected_cmd, _ = _detect_serve_command()
        assert entry["command"] == expected_cmd

    def test_install_zed_config(self, tmp_path):
        zed_settings = tmp_path / "zed" / "settings.json"
        zed_settings.parent.mkdir(parents=True)
        with patch.dict(
            PLATFORMS,
            {
                "zed": {
                    **PLATFORMS["zed"],
                    "config_path": lambda root: zed_settings,
                    "detect": lambda: True,
                },
            },
        ):
            configured = install_platform_configs(tmp_path, target="zed")
        assert "Zed" in configured
        data = json.loads(zed_settings.read_text())
        assert "context_servers" in data
        assert "dagayn" in data["context_servers"]

    def test_install_continue_config(self, tmp_path):
        continue_dir = tmp_path / ".continue"
        continue_dir.mkdir()
        config_path = continue_dir / "config.json"
        with patch.dict(
            PLATFORMS,
            {
                "continue": {
                    **PLATFORMS["continue"],
                    "config_path": lambda root: config_path,
                    "detect": lambda: True,
                },
            },
        ):
            configured = install_platform_configs(tmp_path, target="continue")
        assert "Continue" in configured
        data = json.loads(config_path.read_text())
        assert isinstance(data["mcpServers"], list)
        assert data["mcpServers"][0]["name"] == "dagayn"
        assert data["mcpServers"][0]["type"] == "stdio"

    def test_install_opencode_config(self, tmp_path):
        configured = install_platform_configs(tmp_path, target="opencode")
        assert "OpenCode" in configured
        config_path = tmp_path / ".opencode.json"
        data = json.loads(config_path.read_text())
        entry = data["mcpServers"]["dagayn"]
        assert entry["type"] == "stdio"
        assert entry["env"] == []

    def test_install_qwen_config(self, tmp_path):
        """Qwen Code uses ~/.qwen/settings.json with mcpServers (see #83)."""
        qwen_config = tmp_path / ".qwen" / "settings.json"
        with patch.dict(
            PLATFORMS,
            {
                "qwen": {
                    **PLATFORMS["qwen"],
                    "config_path": lambda root: qwen_config,
                    "detect": lambda: True,
                },
            },
        ):
            configured = install_platform_configs(tmp_path, target="qwen")
        assert "Qwen Code" in configured
        data = json.loads(qwen_config.read_text())
        entry = data["mcpServers"]["dagayn"]
        assert entry["type"] == "stdio"
        assert entry["args"][-1] == "serve"

    def test_install_qwen_preserves_existing_servers(self, tmp_path):
        """Adding qwen should merge with, not clobber, existing mcpServers."""
        qwen_config = tmp_path / ".qwen" / "settings.json"
        qwen_config.parent.mkdir(parents=True)
        qwen_config.write_text(
            json.dumps({"mcpServers": {"other-server": {"command": "other"}}}),
            encoding="utf-8",
        )
        with patch.dict(
            PLATFORMS,
            {
                "qwen": {
                    **PLATFORMS["qwen"],
                    "config_path": lambda root: qwen_config,
                    "detect": lambda: True,
                },
            },
        ):
            install_platform_configs(tmp_path, target="qwen")
        data = json.loads(qwen_config.read_text())
        assert "other-server" in data["mcpServers"]
        assert "dagayn" in data["mcpServers"]

    def test_install_all_detected(self, tmp_path):
        """Installing 'all' configures auto-detected platforms."""
        codex_config = tmp_path / ".codex" / "config.toml"
        with patch.dict(
            PLATFORMS,
            {
                "codex": {
                    **PLATFORMS["codex"],
                    "config_path": lambda root: codex_config,
                    "detect": lambda: True,
                },
                "claude": {**PLATFORMS["claude"], "detect": lambda: True},
                "opencode": {**PLATFORMS["opencode"], "detect": lambda: True},
                "cursor": {**PLATFORMS["cursor"], "detect": lambda: False},
                "windsurf": {**PLATFORMS["windsurf"], "detect": lambda: False},
                "zed": {**PLATFORMS["zed"], "detect": lambda: False},
                "continue": {**PLATFORMS["continue"], "detect": lambda: False},
                "antigravity": {**PLATFORMS["antigravity"], "detect": lambda: False},
            },
        ):
            configured = install_platform_configs(tmp_path, target="all")
        assert "Codex" in configured
        assert "Claude Code" in configured
        assert "OpenCode" in configured
        assert codex_config.exists()
        assert (tmp_path / ".mcp.json").exists()
        assert (tmp_path / ".opencode.json").exists()

    def test_merge_existing_servers(self, tmp_path):
        """Should not overwrite existing MCP servers."""
        mcp_path = tmp_path / ".mcp.json"
        existing = {"mcpServers": {"other-server": {"command": "other"}}}
        mcp_path.write_text(json.dumps(existing))
        install_platform_configs(tmp_path, target="claude")
        data = json.loads(mcp_path.read_text())
        assert "other-server" in data["mcpServers"]
        assert "dagayn" in data["mcpServers"]

    def test_reinstall_json_config_updates_existing_dagayn_args(self, tmp_path):
        mcp_path = tmp_path / ".mcp.json"
        existing = {
            "mcpServers": {
                "dagayn": {"command": "uvx", "args": ["dagayn", "serve"], "type": "stdio"},
                "other-server": {"command": "other"},
            }
        }
        mcp_path.write_text(json.dumps(existing))

        install_platform_configs(
            tmp_path,
            target="claude",
            extra_serve_args=["--local-embedding", "low"],
        )

        data = json.loads(mcp_path.read_text())
        assert data["mcpServers"]["other-server"]["command"] == "other"
        assert data["mcpServers"]["dagayn"]["args"][-2:] == ["--local-embedding", "low"]

    def test_dry_run_no_write(self, tmp_path):
        configured = install_platform_configs(tmp_path, target="claude", dry_run=True)
        assert "Claude Code" in configured
        assert not (tmp_path / ".mcp.json").exists()

    def test_already_configured_skips(self, tmp_path):
        install_platform_configs(tmp_path, target="claude")
        configured = install_platform_configs(tmp_path, target="claude")
        assert "Claude Code" in configured

    def test_continue_array_no_duplicate(self, tmp_path):
        config_path = tmp_path / ".continue" / "config.json"
        config_path.parent.mkdir(parents=True)
        existing = {
            "mcpServers": [
                {
                    "name": "dagayn",
                    "command": "uvx",
                    "args": ["dagayn", "serve"],
                }
            ]
        }
        config_path.write_text(json.dumps(existing))
        with patch.dict(
            PLATFORMS,
            {
                "continue": {
                    **PLATFORMS["continue"],
                    "config_path": lambda root: config_path,
                    "detect": lambda: True,
                },
            },
        ):
            install_platform_configs(tmp_path, target="continue")
        data = json.loads(config_path.read_text())
        assert len(data["mcpServers"]) == 1

    def test_reinstall_continue_array_updates_existing_dagayn_args(self, tmp_path):
        config_path = tmp_path / ".continue" / "config.json"
        config_path.parent.mkdir(parents=True)
        existing = {
            "mcpServers": [
                {"name": "dagayn", "command": "uvx", "args": ["dagayn", "serve"]},
                {"name": "other", "command": "other"},
            ]
        }
        config_path.write_text(json.dumps(existing))
        with patch.dict(
            PLATFORMS,
            {
                "continue": {
                    **PLATFORMS["continue"],
                    "config_path": lambda root: config_path,
                    "detect": lambda: True,
                },
            },
        ):
            install_platform_configs(
                tmp_path,
                target="continue",
                extra_serve_args=["--local-embedding", "low"],
            )

        data = json.loads(config_path.read_text())
        assert len(data["mcpServers"]) == 2
        dagayn_entry = next(entry for entry in data["mcpServers"] if entry["name"] == "dagayn")
        assert dagayn_entry["args"][-2:] == ["--local-embedding", "low"]

    def test_install_qoder_config(self, tmp_path):
        qoder_config = tmp_path / ".qoder" / "mcp.json"
        with patch.dict(
            PLATFORMS,
            {
                "qoder": {
                    **PLATFORMS["qoder"],
                    "config_path": lambda root: qoder_config,
                    "detect": lambda: True,
                },
            },
        ):
            configured = install_platform_configs(tmp_path, target="qoder")
        assert "Qoder" in configured
        data = json.loads(qoder_config.read_text())
        assert "mcpServers" in data
        assert "dagayn" in data["mcpServers"]
        assert data["mcpServers"]["dagayn"]["type"] == "stdio"
        from dagayn.skills import _detect_serve_command

        expected_cmd, expected_args = _detect_serve_command()
        assert data["mcpServers"]["dagayn"]["command"] == expected_cmd
        assert data["mcpServers"]["dagayn"]["args"] == expected_args

    def test_install_qcoder_alias_config(self, tmp_path):
        qoder_config = tmp_path / ".qoder" / "mcp.json"
        with patch.dict(
            PLATFORMS,
            {
                "qoder": {
                    **PLATFORMS["qoder"],
                    "config_path": lambda root: qoder_config,
                    "detect": lambda: True,
                },
            },
        ):
            configured = install_platform_configs(tmp_path, target="qcoder")
        assert "Qoder" in configured
        assert qoder_config.exists()

    def test_install_pi_config(self, tmp_path):
        configured = install_platform_configs(tmp_path, target="pi")
        assert "Pi" in configured
        config_path = tmp_path / ".pi" / "mcp.json"
        data = json.loads(config_path.read_text())
        entry = data["mcpServers"]["dagayn"]
        assert entry["transport"] == "stdio"
        assert entry["lifecycle"] == "lazy"
        assert "type" not in entry

    def test_install_hermes_config(self, tmp_path):
        hermes_config = tmp_path / ".hermes" / "config.yaml"
        with patch.dict(
            PLATFORMS,
            {
                "hermes": {
                    **PLATFORMS["hermes"],
                    "config_path": lambda root: hermes_config,
                    "detect": lambda: True,
                },
            },
        ):
            configured = install_platform_configs(tmp_path, target="hermes")
        assert "Hermes Agent" in configured
        data = yaml.safe_load(hermes_config.read_text())
        entry = data["mcp_servers"]["dagayn"]
        assert entry["args"][-1] == "serve"
        assert "type" not in entry

    def test_install_hermes_preserves_existing_servers(self, tmp_path):
        hermes_config = tmp_path / ".hermes" / "config.yaml"
        hermes_config.parent.mkdir(parents=True)
        hermes_config.write_text(
            "model:\n  default: local\nmcp_servers:\n  other:\n    command: other\n",
            encoding="utf-8",
        )
        with patch.dict(
            PLATFORMS,
            {
                "hermes": {
                    **PLATFORMS["hermes"],
                    "config_path": lambda root: hermes_config,
                    "detect": lambda: True,
                },
            },
        ):
            install_platform_configs(tmp_path, target="hermes")
        data = yaml.safe_load(hermes_config.read_text())
        assert data["model"]["default"] == "local"
        assert data["mcp_servers"]["other"]["command"] == "other"
        assert "dagayn" in data["mcp_servers"]

    def test_install_all_includes_workspace_pi(self, tmp_path):
        (tmp_path / ".pi").mkdir()
        with patch.dict(
            PLATFORMS,
            {key: {**plat, "detect": lambda: False} for key, plat in PLATFORMS.items()},
            clear=True,
        ):
            configured = install_platform_configs(tmp_path, target="all")
        assert configured == ["Pi"]
        assert (tmp_path / ".pi" / "mcp.json").exists()


class TestNormalizePlatformTarget:
    def test_claude_code_alias(self):
        assert normalize_platform_target("claude-code") == "claude"

    def test_qcoder_alias(self):
        assert normalize_platform_target("qcoder") == "qoder"

    def test_passthrough(self):
        assert normalize_platform_target("opencode") == "opencode"


class TestCursorHooksConfig:
    """Tests for generate_cursor_hooks_config()."""

    def test_has_version_1(self):
        config = generate_cursor_hooks_config()
        assert config["version"] == 1

    def test_has_after_file_edit(self):
        config = generate_cursor_hooks_config()
        hooks = config["hooks"]["afterFileEdit"]
        assert len(hooks) >= 1
        assert "crg-update.sh" in hooks[0]["command"]
        assert hooks[0]["timeout"] == 15

    def test_has_session_start(self):
        config = generate_cursor_hooks_config()
        hooks = config["hooks"]["sessionStart"]
        assert len(hooks) >= 1
        assert "crg-session-start.sh" in hooks[0]["command"]
        assert hooks[0]["timeout"] == 60

    def test_has_before_shell_execution(self):
        config = generate_cursor_hooks_config()
        hooks = config["hooks"]["beforeShellExecution"]
        assert len(hooks) == 1
        assert "crg-pre-commit.sh" in hooks[0]["command"]
        assert hooks[0]["timeout"] == 120
        assert hooks[0]["matcher"] == (r"(?:^|[/\\]|\s)git(?:\.exe)?\s+commit\b")

    def test_has_after_shell_execution_relocate(self):
        config = generate_cursor_hooks_config()
        hooks = config["hooks"]["afterShellExecution"]
        assert len(hooks) == 1
        assert "crg-relocate.sh" in hooks[0]["command"]
        assert hooks[0]["timeout"] == 60
        assert hooks[0]["matcher"] == (
            r"(?:^|[/\\]|\s)git(?:\.exe)?\s+"
            r"(?:checkout|switch|reset|pull|merge|rebase|cherry-pick)\b"
        )

    def test_git_commit_matcher_covers_path_qualified_git(self):
        import re

        from dagayn.skills import _GIT_COMMIT_COMMAND_MATCHER

        pattern = re.compile(_GIT_COMMIT_COMMAND_MATCHER)
        assert pattern.search("git commit -m 'x'")
        assert pattern.search("/Users/manji0/.nix-profile/bin/git commit -m 'x'")
        assert pattern.search(r"C:\Program Files\Git\cmd\git.exe commit -m x")
        assert pattern.search("env FOO=1 /usr/bin/git commit")
        assert pattern.search("echo git commit")
        assert not pattern.search("git status")
        assert not pattern.search("gitaly commit")
        assert not pattern.search("git-commit hook")

    def test_has_all_hook_types(self):
        config = generate_cursor_hooks_config()
        hook_types = set(config["hooks"].keys())
        assert hook_types == {
            "afterFileEdit",
            "sessionStart",
            "beforeShellExecution",
            "afterShellExecution",
        }

    def test_commands_point_to_home_cursor_hooks(self):
        config = generate_cursor_hooks_config()
        from pathlib import Path

        hooks_dir = str(Path.home() / ".cursor" / "hooks")
        for event, entries in config["hooks"].items():
            for entry in entries:
                assert entry["command"].startswith(hooks_dir), (
                    f"{event} command does not start with {hooks_dir}"
                )


class TestCursorHookScripts:
    """Tests for _cursor_hook_scripts()."""

    def test_returns_four_scripts(self):
        scripts = _cursor_hook_scripts()
        assert set(scripts.keys()) == {
            "crg-update.sh",
            "crg-session-start.sh",
            "crg-pre-commit.sh",
            "crg-relocate.sh",
        }

    def test_scripts_start_with_shebang(self):
        scripts = _cursor_hook_scripts()
        for name, content in scripts.items():
            assert content.startswith("#!/usr/bin/env bash"), f"{name} missing shebang line"

    def test_scripts_exit_zero(self):
        """Each script must end with exit 0 for graceful failure."""
        scripts = _cursor_hook_scripts()
        for name, content in scripts.items():
            assert "exit 0" in content, f"{name} missing 'exit 0'"

    def test_scripts_consume_stdin(self):
        """Each script must read the JSON payload Cursor sends on stdin."""
        scripts = _cursor_hook_scripts()
        for name, content in scripts.items():
            assert 'payload="$(cat 2>/dev/null || true)"' in content, (
                f"{name} does not read the hook payload"
            )

    def test_scripts_resolve_repo_from_payload(self):
        """User hooks run from ~/.cursor, so cwd must never decide the repo."""
        scripts = _cursor_hook_scripts()
        for name, content in scripts.items():
            assert "dagayn hook-repo --no-cwd-fallback" in content, (
                f"{name} does not resolve the repo from the payload"
            )
            assert "CURSOR_PROJECT_DIR" in content, f"{name} missing env fallback"

    def test_scripts_pass_repo_explicitly(self):
        scripts = _cursor_hook_scripts()
        for name, content in scripts.items():
            if "dagayn update" in content or "dagayn session prepare" in content:
                assert '--repo "$repo"' in content, f"{name} does not pass --repo"

    def test_update_script_runs_update(self):
        scripts = _cursor_hook_scripts()
        assert "dagayn update --skip-flows" in scripts["crg-update.sh"]

    def test_extra_update_args_reach_prepare_scripts_only(self):
        scripts = _cursor_hook_scripts(["--local-embedding", "low"])
        # Edit- and commit-triggered updates stay structure-only; re-embedding
        # on every edit is far too expensive.
        assert "--local-embedding" not in scripts["crg-update.sh"]
        assert "--local-embedding" not in scripts["crg-pre-commit.sh"]
        assert "--local-embedding low" in scripts["crg-session-start.sh"]
        assert "--local-embedding low" in scripts["crg-relocate.sh"]

    def test_session_start_script_runs_status(self):
        scripts = _cursor_hook_scripts()
        assert "session prepare" in scripts["crg-session-start.sh"]

    def test_session_start_script_inherits_worktree_graph(self):
        script = _cursor_hook_scripts()["crg-session-start.sh"]
        assert "dagayn session prepare" in script
        assert "crg-relocate.sh" in _cursor_hook_scripts()
        assert "additional_context" in script

    def test_pre_commit_script_returns_permission_decision(self):
        script = _cursor_hook_scripts()["crg-pre-commit.sh"]
        assert '"permission"' in script
        assert "allow" in script

    def test_relocate_script_is_observational(self):
        script = _cursor_hook_scripts()["crg-relocate.sh"]
        assert "session prepare" in script
        assert "afterShellExecution" in script or "HEAD-moving" in script
        assert '"permission"' not in script

    def test_pre_commit_script_runs_detect_changes(self):
        scripts = _cursor_hook_scripts()
        assert "dagayn update --skip-flows" in scripts["crg-pre-commit.sh"]
        assert "dagayn detect-changes --brief" in scripts["crg-pre-commit.sh"]


class TestInstallCursorHooks:
    """Tests for install_cursor_hooks()."""

    def test_creates_hooks_json(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_cursor_hooks()
        hooks_json = tmp_path / ".cursor" / "hooks.json"
        assert hooks_json.exists()
        assert result == hooks_json
        data = json.loads(hooks_json.read_text())
        assert data["version"] == 1
        assert "afterFileEdit" in data["hooks"]

    def test_creates_hook_scripts(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_cursor_hooks()
        hooks_dir = tmp_path / ".cursor" / "hooks"
        assert (hooks_dir / "crg-update.sh").exists()
        assert (hooks_dir / "crg-session-start.sh").exists()
        assert (hooks_dir / "crg-pre-commit.sh").exists()

    def test_scripts_are_executable(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_cursor_hooks()
        hooks_dir = tmp_path / ".cursor" / "hooks"
        for script in hooks_dir.iterdir():
            mode = script.stat().st_mode
            assert mode & stat.S_IXUSR, f"{script.name} not executable by owner"
            assert mode & stat.S_IXGRP, f"{script.name} not executable by group"

    def test_merges_with_existing_hooks_json(self, tmp_path):
        cursor_dir = tmp_path / ".cursor"
        cursor_dir.mkdir(parents=True)
        existing = {
            "version": 1,
            "hooks": {
                "afterFileEdit": [{"command": "/some/other/hook.sh", "timeout": 3}],
                "stop": [{"command": "/some/stop-hook.sh", "timeout": 2}],
            },
        }
        (cursor_dir / "hooks.json").write_text(json.dumps(existing))

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_cursor_hooks()

        data = json.loads((cursor_dir / "hooks.json").read_text())
        # Original hook preserved
        commands = [h["command"] for h in data["hooks"]["afterFileEdit"]]
        assert "/some/other/hook.sh" in commands
        # Our hook added
        assert any("crg-update.sh" in c for c in commands)
        # Unrelated hook type preserved
        assert "stop" in data["hooks"]

    def test_no_duplicate_on_reinstall(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_cursor_hooks()
            install_cursor_hooks()

        data = json.loads((tmp_path / ".cursor" / "hooks.json").read_text())
        # dagayn hooks should not duplicate on reinstall.
        expected = {
            "afterFileEdit": 1,
            "sessionStart": 1,
            "beforeShellExecution": 1,
            "afterShellExecution": 1,
        }
        for event, entries in data["hooks"].items():
            crg_hooks = [h for h in entries if "crg-" in h.get("command", "")]
            assert len(crg_hooks) == expected.get(event, len(crg_hooks)), (
                f"{event} has {len(crg_hooks)} crg hooks after reinstall"
            )

    def test_reinstall_migrates_relocate_to_after_shell(self, tmp_path):
        cursor_dir = tmp_path / ".cursor"
        hooks_dir = cursor_dir / "hooks"
        hooks_dir.mkdir(parents=True)
        relocate_cmd = str(hooks_dir / "crg-relocate.sh")
        existing = {
            "version": 1,
            "hooks": {
                "beforeShellExecution": [
                    {
                        "matcher": r"(?:^|[/\\]|\s)git(?:\.exe)?\s+commit\b",
                        "command": str(hooks_dir / "crg-pre-commit.sh"),
                        "timeout": 120,
                    },
                    {
                        "matcher": "git checkout",
                        "command": relocate_cmd,
                        "timeout": 60,
                    },
                ],
            },
        }
        (cursor_dir / "hooks.json").write_text(json.dumps(existing))

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_cursor_hooks()

        data = json.loads((cursor_dir / "hooks.json").read_text())
        before = data["hooks"]["beforeShellExecution"]
        assert not any("crg-relocate.sh" in h.get("command", "") for h in before)
        after = data["hooks"]["afterShellExecution"]
        assert any("crg-relocate.sh" in h.get("command", "") for h in after)

    def test_reinstall_updates_git_commit_matcher(self, tmp_path):
        cursor_dir = tmp_path / ".cursor"
        hooks_dir = cursor_dir / "hooks"
        hooks_dir.mkdir(parents=True)
        stale_cmd = str(hooks_dir / "crg-pre-commit.sh")
        existing = {
            "version": 1,
            "hooks": {
                "beforeShellExecution": [
                    {
                        "matcher": "^git\\s+commit",
                        "command": stale_cmd,
                        "timeout": 10,
                    }
                ],
                "sessionStart": [{"command": "bash '/tmp/herdr-agent-state.sh' session"}],
            },
        }
        (cursor_dir / "hooks.json").write_text(json.dumps(existing))

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_cursor_hooks()

        data = json.loads((cursor_dir / "hooks.json").read_text())
        shell_hooks = data["hooks"]["beforeShellExecution"]
        crg = [h for h in shell_hooks if "crg-pre-commit.sh" in h.get("command", "")]
        assert len(crg) == 1
        assert crg[0]["matcher"] == r"(?:^|[/\\]|\s)git(?:\.exe)?\s+commit\b"
        # Unrelated hooks preserved
        session_cmds = [h["command"] for h in data["hooks"]["sessionStart"]]
        assert "bash '/tmp/herdr-agent-state.sh' session" in session_cmds
        assert any("crg-session-start.sh" in c for c in session_cmds)

    def test_handles_corrupt_existing_json(self, tmp_path):
        cursor_dir = tmp_path / ".cursor"
        cursor_dir.mkdir(parents=True)
        (cursor_dir / "hooks.json").write_text("not valid json{{{")

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_cursor_hooks()

        assert result.exists()
        data = json.loads(result.read_text())
        assert data["version"] == 1


class TestKiroPlatform:
    """Tests for Kiro platform support."""

    def test_kiro_platform_entry_exists(self):
        """PLATFORMS dict has a 'kiro' key with correct metadata."""
        assert "kiro" in PLATFORMS
        kiro = PLATFORMS["kiro"]
        assert kiro["name"] == "Kiro"
        assert kiro["key"] == "mcpServers"
        assert kiro["format"] == "object"
        assert kiro["needs_type"] is True

    def test_install_kiro_config(self, tmp_path):
        """install_platform_configs creates .kiro/settings/mcp.json."""
        configured = install_platform_configs(tmp_path, target="kiro")
        assert "Kiro" in configured
        config_path = tmp_path / ".kiro" / "settings" / "mcp.json"
        assert config_path.exists()
        data = json.loads(config_path.read_text())
        assert "dagayn" in data["mcpServers"]
        entry = data["mcpServers"]["dagayn"]
        assert entry["type"] == "stdio"

    def test_install_kiro_preserves_existing_servers(self, tmp_path):
        """Existing mcpServers entries are preserved when adding dagayn."""
        config_path = tmp_path / ".kiro" / "settings" / "mcp.json"
        config_path.parent.mkdir(parents=True)
        config_path.write_text(
            json.dumps({"mcpServers": {"other-server": {"command": "other"}}}),
            encoding="utf-8",
        )
        install_platform_configs(tmp_path, target="kiro")
        data = json.loads(config_path.read_text())
        assert "other-server" in data["mcpServers"]
        assert "dagayn" in data["mcpServers"]

    def test_install_kiro_no_duplicate(self, tmp_path):
        """Second install skips when dagayn already exists."""
        install_platform_configs(tmp_path, target="kiro")
        config_path = tmp_path / ".kiro" / "settings" / "mcp.json"
        first_content = config_path.read_text()
        install_platform_configs(tmp_path, target="kiro")
        second_content = config_path.read_text()
        assert first_content == second_content
        data = json.loads(second_content)
        assert list(data["mcpServers"].keys()).count("dagayn") == 1

    def test_kiro_steering_file_written(self, tmp_path):
        """inject_platform_instructions creates .kiro/steering/dagayn.md."""
        updated = inject_platform_instructions(tmp_path, target="kiro")
        assert ".kiro/steering/dagayn.md" in updated
        steering = tmp_path / ".kiro" / "steering" / "dagayn.md"
        assert steering.exists()
        content = steering.read_text()
        assert _CLAUDE_MD_SECTION_MARKER in content

    def test_kiro_steering_idempotent(self, tmp_path):
        """Running inject twice produces identical content."""
        inject_platform_instructions(tmp_path, target="kiro")
        first = (tmp_path / ".kiro" / "steering" / "dagayn.md").read_text()
        inject_platform_instructions(tmp_path, target="kiro")
        second = (tmp_path / ".kiro" / "steering" / "dagayn.md").read_text()
        assert first == second

    def test_kiro_included_in_all_when_detected(self, tmp_path):
        """install_platform_configs with target='all' includes Kiro when .kiro exists."""
        (tmp_path / ".kiro").mkdir()
        # Mock Path.home() to a dir without .kiro so only workspace detection fires
        fake_home = tmp_path / "fakehome"
        fake_home.mkdir()
        with patch("dagayn.skills.Path.home", return_value=fake_home):
            configured = install_platform_configs(tmp_path, target="all")
        assert "Kiro" in configured

    def test_kiro_workspace_detection(self, tmp_path):
        """Kiro detected when repo_root/.kiro exists even if ~/.kiro does not."""
        (tmp_path / ".kiro").mkdir()
        fake_home = tmp_path / "fakehome"
        fake_home.mkdir()
        with patch("dagayn.skills.Path.home", return_value=fake_home):
            configured = install_platform_configs(tmp_path, target="all")
        assert "Kiro" in configured
        config_path = tmp_path / ".kiro" / "settings" / "mcp.json"
        assert config_path.exists()

    def test_kiro_dry_run(self, tmp_path):
        """dry_run=True does not create any files."""
        configured = install_platform_configs(tmp_path, target="kiro", dry_run=True)
        assert "Kiro" in configured
        config_path = tmp_path / ".kiro" / "settings" / "mcp.json"
        assert not config_path.exists()


class TestDetectServeCommand:
    """Tests for _detect_serve_command() and its helpers."""

    # ------------------------------------------------------------------
    # _in_poetry_project() unit tests
    # ------------------------------------------------------------------

    def test_in_poetry_project_via_poetry_active(self, monkeypatch):
        """POETRY_ACTIVE=1 signals a poetry shell session."""
        monkeypatch.setenv("POETRY_ACTIVE", "1")
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        assert _in_poetry_project() is True

    def test_in_poetry_project_via_virtual_env(self, monkeypatch):
        """VIRTUAL_ENV containing 'pypoetry' signals a poetry run session."""
        monkeypatch.delenv("POETRY_ACTIVE", raising=False)
        monkeypatch.setenv("VIRTUAL_ENV", "/home/user/.cache/pypoetry/virtualenvs/proj-xxx")
        assert _in_poetry_project() is True

    def test_in_poetry_project_false_for_plain_venv(self, monkeypatch):
        """A plain venv (no pypoetry in path) is not treated as poetry."""
        monkeypatch.delenv("POETRY_ACTIVE", raising=False)
        monkeypatch.setenv("VIRTUAL_ENV", "/home/user/myproject/.venv")
        assert _in_poetry_project() is False

    def test_in_poetry_project_false_when_nothing_set(self, monkeypatch):
        """No env vars → not in a poetry project."""
        monkeypatch.delenv("POETRY_ACTIVE", raising=False)
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        assert _in_poetry_project() is False

    # ------------------------------------------------------------------
    # _detect_serve_command() integration tests
    # ------------------------------------------------------------------

    def test_poetry_active_returns_poetry_run(self, monkeypatch):
        """POETRY_ACTIVE=1 (poetry shell) → 'poetry run' invocation."""
        monkeypatch.setenv("POETRY_ACTIVE", "1")
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        monkeypatch.setattr(
            "dagayn.skills.shutil.which",
            lambda x: "/usr/bin/poetry" if x == "poetry" else None,
        )
        cmd, args = _detect_serve_command()
        assert cmd == "poetry"
        assert args == ["run", "dagayn", "serve"]

    def test_virtual_env_pypoetry_returns_poetry_run(self, monkeypatch):
        """VIRTUAL_ENV with 'pypoetry' (poetry run) → 'poetry run' invocation."""
        monkeypatch.delenv("POETRY_ACTIVE", raising=False)
        monkeypatch.setenv("VIRTUAL_ENV", "/home/user/.cache/pypoetry/virtualenvs/proj-abc123")
        monkeypatch.setattr(
            "dagayn.skills.shutil.which",
            lambda x: "/usr/bin/poetry" if x == "poetry" else None,
        )
        cmd, args = _detect_serve_command()
        assert cmd == "poetry"
        assert args == ["run", "dagayn", "serve"]

    def test_poetry_env_without_poetry_on_path_falls_through(self, monkeypatch):
        """If poetry venv is detected but poetry binary is missing, fall through."""
        monkeypatch.setenv("POETRY_ACTIVE", "1")
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
        monkeypatch.setattr("dagayn.skills._in_uv_project", lambda: False)
        # poetry not on PATH → should fall through to uvx
        monkeypatch.setattr(
            "dagayn.skills.shutil.which",
            lambda x: "/usr/bin/uvx" if x == "uvx" else None,
        )
        cmd, _ = _detect_serve_command()
        assert cmd == "uvx"

    def test_uv_project_env_returns_uv_run(self, monkeypatch):
        """UV_PROJECT_ENVIRONMENT set + uv on PATH → 'uv run' invocation."""
        monkeypatch.delenv("POETRY_ACTIVE", raising=False)
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", "/some/.venv")
        monkeypatch.setattr(
            "dagayn.skills.shutil.which",
            lambda x: "/usr/bin/uv" if x == "uv" else None,
        )
        cmd, args = _detect_serve_command()
        assert cmd == "uv"
        assert args == ["run", "dagayn", "serve"]

    def test_uv_lock_detection_returns_uv_run(self, monkeypatch, tmp_path):
        """uv.lock alongside sys.executable → detected as a uv project."""
        monkeypatch.delenv("POETRY_ACTIVE", raising=False)
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
        venv = tmp_path / ".venv" / "bin"
        venv.mkdir(parents=True)
        (tmp_path / "uv.lock").write_text("")
        fake_python = venv / "python"
        fake_python.write_text("")
        monkeypatch.setattr("dagayn.skills.sys.executable", str(fake_python))
        monkeypatch.setattr(
            "dagayn.skills.shutil.which",
            lambda x: "/usr/bin/uv" if x == "uv" else None,
        )
        assert _in_uv_project() is True
        cmd, args = _detect_serve_command()
        assert cmd == "uv"
        assert args == ["run", "dagayn", "serve"]

    def test_installed_dagayn_preferred_before_uvx(self, monkeypatch):
        """Not in Poetry/uv but dagayn available -> use installed CLI."""
        monkeypatch.delenv("POETRY_ACTIVE", raising=False)
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
        monkeypatch.setattr("dagayn.skills._in_uv_project", lambda: False)
        monkeypatch.setattr(
            "dagayn.skills.shutil.which",
            lambda x: f"/usr/bin/{x}" if x in {"dagayn", "uvx"} else None,
        )
        cmd, args = _detect_serve_command()
        assert cmd == "dagayn"
        assert args == ["serve"]

    def test_uvx_fallback(self, monkeypatch):
        """Not in Poetry/uv and no dagayn executable, but uvx available -> use uvx."""
        monkeypatch.delenv("POETRY_ACTIVE", raising=False)
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
        monkeypatch.setattr("dagayn.skills._in_uv_project", lambda: False)
        monkeypatch.setattr(
            "dagayn.skills.shutil.which",
            lambda x: "/usr/bin/uvx" if x == "uvx" else None,
        )
        cmd, args = _detect_serve_command()
        assert cmd == "uvx"
        assert args == ["dagayn", "serve"]

    def test_sys_executable_fallback(self, monkeypatch):
        """Nothing else available → fall back to sys.executable -m."""
        monkeypatch.delenv("POETRY_ACTIVE", raising=False)
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
        monkeypatch.setattr("dagayn.skills._in_uv_project", lambda: False)
        monkeypatch.setattr("dagayn.skills.shutil.which", lambda _: None)
        cmd, args = _detect_serve_command()
        assert cmd == sys.executable
        assert args == ["-m", "dagayn", "serve"]

    def test_poetry_takes_priority_over_uv(self, monkeypatch):
        """Poetry detection wins even when UV_PROJECT_ENVIRONMENT is also set."""
        monkeypatch.setenv("POETRY_ACTIVE", "1")
        monkeypatch.delenv("VIRTUAL_ENV", raising=False)
        monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", "/some/.venv")
        monkeypatch.setattr(
            "dagayn.skills.shutil.which",
            lambda x: "/usr/bin/poetry" if x == "poetry" else None,
        )
        cmd, _ = _detect_serve_command()
        assert cmd == "poetry"

    def test_in_uv_project_false_without_lockfile(self, monkeypatch, tmp_path):
        """_in_uv_project returns False when no uv.lock in ancestor dirs."""
        fake_python = tmp_path / "bin" / "python"
        fake_python.parent.mkdir(parents=True)
        fake_python.write_text("")
        monkeypatch.setattr("dagayn.skills.sys.executable", str(fake_python))
        monkeypatch.setattr("dagayn.skills.Path.home", staticmethod(lambda: tmp_path))
        assert _in_uv_project() is False


class TestOpenCodePluginContent:
    """Tests for _opencode_plugin_content()."""

    def test_returns_non_empty_string(self):
        content = _opencode_plugin_content()
        assert isinstance(content, str)
        assert len(content) > 100

    def test_has_plugin_type_import(self):
        content = _opencode_plugin_content()
        assert "import type" in content
        assert "@opencode-ai/plugin" in content

    def test_has_default_export(self):
        content = _opencode_plugin_content()
        assert "export default" in content

    def test_hooks_file_edited_event(self):
        content = _opencode_plugin_content()
        assert '"file.edited"' in content
        assert "dagayn update --skip-flows" in content

    def test_hooks_session_created_event(self):
        content = _opencode_plugin_content()
        assert '"session.created"' in content
        assert "dagayn session prepare" in content
        assert "dagayn status" in content

    def test_hooks_resolve_repo_and_pass_repo_flag(self):
        content = _opencode_plugin_content()
        assert "git rev-parse --show-toplevel" in content
        assert "--repo ${repo}" in content

    def test_plugin_includes_extra_update_args(self):
        content = _opencode_plugin_content(["--local-embedding"])
        assert "--local-embedding" in content

    def test_hooks_tool_execute_before_event(self):
        content = _opencode_plugin_content()
        assert '"tool.execute.before"' in content
        assert "dagayn detect-changes --brief" in content

    def test_hooks_tool_execute_after_relocate(self):
        content = _opencode_plugin_content()
        assert '"tool.execute.after"' in content
        assert "checkout|switch|reset|pull|merge|rebase|cherry-pick" in content
        # Relocate must not run on the before hook.
        before_idx = content.index('"tool.execute.before"')
        after_idx = content.index('"tool.execute.after"')
        before_block = content[before_idx:after_idx]
        assert "session prepare" not in before_block
        assert "session prepare" in content[after_idx:]

    def test_has_git_commit_detection(self):
        """Pre-commit hook should match path-qualified git commit commands."""
        content = _opencode_plugin_content()
        assert "git" in content
        assert "commit" in content
        assert r"(?:^|[\/\\]|\s)git(?:\.exe)?\s+commit\b" in content

    def test_all_handlers_have_try_catch(self):
        """Every event handler must use try/catch for graceful failure."""
        content = _opencode_plugin_content()
        # file.edited, session.created, tool.execute.before, tool.execute.after
        assert content.count("} catch") >= 4


class TestInstallOpenCodePlugin:
    """Tests for install_opencode_plugin()."""

    def test_creates_plugin_file(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_opencode_plugin()
        plugin_path = tmp_path / ".config" / "opencode" / "plugins" / "crg-plugin.ts"
        assert plugin_path.exists()
        assert result == plugin_path

    def test_plugin_file_has_correct_content(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_opencode_plugin()
        content = result.read_text(encoding="utf-8")
        assert "export default" in content
        assert "file.edited" in content

    def test_creates_parent_directories(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_opencode_plugin()
        plugins_dir = tmp_path / ".config" / "opencode" / "plugins"
        assert plugins_dir.is_dir()

    def test_overwrites_existing_plugin(self, tmp_path):
        plugins_dir = tmp_path / ".config" / "opencode" / "plugins"
        plugins_dir.mkdir(parents=True)
        old_plugin = plugins_dir / "crg-plugin.ts"
        old_plugin.write_text("// old version")

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_opencode_plugin()

        content = old_plugin.read_text()
        assert "// old version" not in content
        assert "export default" in content

    def test_idempotent(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_opencode_plugin()
            result = install_opencode_plugin()
        content = result.read_text()
        assert "export default" in content
        # Only one default export in the file
        assert content.count("export default") == 1

    def test_plugin_is_typescript(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_opencode_plugin()
        assert result.suffix == ".ts"

    def test_preserves_other_plugins(self, tmp_path):
        plugins_dir = tmp_path / ".config" / "opencode" / "plugins"
        plugins_dir.mkdir(parents=True)
        other_plugin = plugins_dir / "other-plugin.ts"
        other_plugin.write_text("// other plugin")

        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_opencode_plugin()

        assert other_plugin.exists()
        assert other_plugin.read_text() == "// other plugin"

    def test_file_is_utf8(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            result = install_opencode_plugin()
        # Should be readable as UTF-8 without errors
        content = result.read_text(encoding="utf-8")
        assert len(content) > 0
