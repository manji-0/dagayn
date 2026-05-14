"""Tests for skills and hooks auto-install."""

import json
import os
import stat
import sys
import tomllib
from pathlib import Path
from unittest.mock import patch

import dagayn.skills as _skills_module
from dagayn.skills import (
    _CLAUDE_MD_SECTION_MARKER,
    _MARKDOWN_POLICY_MARKER,
    PLATFORMS,
    _cursor_hook_scripts,
    _detect_serve_command,
    _in_poetry_project,
    _in_uv_project,
    _opencode_plugin_content,
    _resolve_source_skills_dir,
    generate_cursor_hooks_config,
    generate_hooks_config,
    generate_skills,
    inject_claude_md,
    inject_platform_instructions,
    install_codex_hooks,
    install_codex_skills,
    install_cursor_hooks,
    install_git_hook,
    install_global_skills,
    install_hooks,
    install_opencode_plugin,
    install_opencode_skills,
    install_platform_configs,
    normalize_platform_target,
)

EXPECTED_SKILLS = [
    "build-graph.md",
    "debug-issue.md",
    "explore-codebase.md",
    "reading-markdown-document.md",
    "refactor-safely.md",
    "review-changes.md",
    "review-delta.md",
    "review-pr.md",
    "writing-markdown-document.md",
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

    def test_explore_skill_uses_architecture_health(self, tmp_path):
        """Generated exploration skill should use the composed architecture surface."""
        skills_dir = generate_skills(tmp_path)
        content = (skills_dir / "explore-codebase.md").read_text()
        assert "architecture_health" in content

    def test_idempotent(self, tmp_path):
        """Running twice should not fail and files should still be valid."""
        generate_skills(tmp_path)
        generate_skills(tmp_path)
        skills_dir = tmp_path / ".claude" / "skills"
        assert len(list(skills_dir.iterdir())) == len(EXPECTED_SKILLS)


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
            mode="fts",
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

    def test_tree_skill_install_is_idempotent(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            install_codex_skills()
            result = install_codex_skills()

        installed = [path.name for path in result.iterdir() if path.is_dir()]
        assert sorted(installed) == sorted(path[:-3] for path in EXPECTED_SKILLS)


class TestGenerateHooksConfig:
    def test_returns_dict_with_hooks(self):
        config = generate_hooks_config(Path("/repo"))
        assert "hooks" in config

    def test_has_post_tool_use(self):
        config = generate_hooks_config(Path("/repo"))
        assert "PostToolUse" in config["hooks"]
        entry = config["hooks"]["PostToolUse"][0]
        assert entry["matcher"] == "Edit|Write|Bash"
        inner = entry["hooks"][0]
        assert inner["type"] == "command"
        assert "update" in inner["command"]
        assert 0 < inner["timeout"] <= 600

    def test_has_session_start(self):
        config = generate_hooks_config(Path("/repo"))
        assert "SessionStart" in config["hooks"]
        entry = config["hooks"]["SessionStart"][0]
        assert "matcher" in entry
        inner = entry["hooks"][0]
        assert inner["type"] == "command"
        assert "status" in inner["command"]
        assert 0 < inner["timeout"] <= 600

    def test_does_not_emit_invalid_pre_commit_hook(self):
        config = generate_hooks_config(Path("/repo"))
        assert "PreCommit" not in config["hooks"]

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

    def test_repo_root_embedded_in_commands(self):
        config = generate_hooks_config(Path("/my/project"))
        post_cmd = config["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        session_cmd = config["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        assert "/my/project" in post_cmd
        assert "/my/project" in session_cmd

    def test_quotes_repo_paths_with_spaces(self):
        config = generate_hooks_config(Path("/repo with spaces"))
        post_cmd = config["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        assert '"' in post_cmd  # path is JSON-encoded so spaces are quoted

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
        assert "dagayn detect-changes" in content

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

    def test_no_git_dir_returns_none(self, tmp_path):
        assert install_git_hook(tmp_path) is None


class TestInstallHooks:
    def test_creates_settings_file(self, tmp_path):
        install_hooks(tmp_path)
        settings_path = tmp_path / ".claude" / "settings.json"
        assert settings_path.exists()
        data = json.loads(settings_path.read_text())
        assert "hooks" in data

    def test_merges_with_existing(self, tmp_path):
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
        install_hooks(tmp_path)
        assert (tmp_path / ".claude").is_dir()

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
        hooks_path = install_codex_hooks(tmp_path)

        assert hooks_path == tmp_path / ".codex" / "hooks.json"
        data = json.loads(hooks_path.read_text())
        assert "PostToolUse" in data["hooks"]
        assert "SessionStart" in data["hooks"]

        config = tomllib.loads((tmp_path / ".codex" / "config.toml").read_text())
        assert config["features"]["codex_hooks"] is True

    def test_merges_existing_hooks_json(self, tmp_path):
        codex_dir = tmp_path / ".codex"
        codex_dir.mkdir()
        existing = {"hooks": {"Stop": []}, "customSetting": True}
        (codex_dir / "hooks.json").write_text(json.dumps(existing), encoding="utf-8")

        install_codex_hooks(tmp_path)

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

        install_codex_hooks(tmp_path)

        config = tomllib.loads((codex_dir / "config.toml").read_text())
        assert config["model"] == "gpt-5.4"
        assert config["mcp_servers"]["other"]["command"] == "other"
        assert config["features"]["codex_hooks"] is True

    def test_no_duplicate_on_reinstall(self, tmp_path):
        install_codex_hooks(tmp_path)
        install_codex_hooks(tmp_path)

        data = json.loads((tmp_path / ".codex" / "hooks.json").read_text())
        for entries in data["hooks"].values():
            dagayn_hooks = [
                entry
                for entry in entries
                if any("dagayn" in hook.get("command", "") for hook in entry.get("hooks", []))
            ]
            assert len(dagayn_hooks) == 1
        assert (tmp_path / ".codex" / "config.toml").read_text().count("codex_hooks") == 1


class TestInjectClaudeMd:
    def test_creates_section_in_new_file(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_claude_md(tmp_path)
        content = (tmp_path / ".claude" / "CLAUDE.md").read_text()
        assert _CLAUDE_MD_SECTION_MARKER in content
        assert "MCP Tools" in content
        assert "get_minimal_context" in content
        assert "How to judge analysis output" in content
        assert "truncated" in content
        assert "--tool-profile full" in content
        assert "analysis_summary" in content
        assert "architecture_health" in content

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

    def test_agents_md_mentions_tool_profiles_and_composed_outputs(self, tmp_path):
        with patch("dagayn.skills.Path.home", return_value=tmp_path):
            inject_platform_instructions(tmp_path, target="codex")

        content = (tmp_path / ".codex" / "AGENTS.md").read_text()
        assert "--tool-profile full" in content
        assert "analysis_summary" in content
        assert "architecture_health" in content
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
        original_write_text = Path.write_text

        def write_text(path, *args, **kwargs):
            if path == blocked:
                raise PermissionError("read-only")
            return original_write_text(path, *args, **kwargs)

        with (
            patch("dagayn.skills.Path.home", return_value=tmp_path),
            patch("pathlib.Path.write_text", new=write_text),
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

    def test_install_cursor_config(self, tmp_path):
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
        assert "dagayn" in data["mcpServers"]
        assert data["mcpServers"]["dagayn"]["type"] == "stdio"

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
        assert hooks[0]["timeout"] == 5

    def test_has_session_start(self):
        config = generate_cursor_hooks_config()
        hooks = config["hooks"]["sessionStart"]
        assert len(hooks) >= 1
        assert "crg-session-start.sh" in hooks[0]["command"]
        assert hooks[0]["timeout"] == 5

    def test_has_before_shell_execution(self):
        config = generate_cursor_hooks_config()
        hooks = config["hooks"]["beforeShellExecution"]
        assert len(hooks) >= 1
        assert "crg-pre-commit.sh" in hooks[0]["command"]
        assert hooks[0]["timeout"] == 10
        assert hooks[0]["matcher"] == "^git\\s+commit"

    def test_has_all_three_hook_types(self):
        config = generate_cursor_hooks_config()
        hook_types = set(config["hooks"].keys())
        assert hook_types == {"afterFileEdit", "sessionStart", "beforeShellExecution"}

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

    def test_returns_three_scripts(self):
        scripts = _cursor_hook_scripts()
        assert set(scripts.keys()) == {
            "crg-update.sh",
            "crg-session-start.sh",
            "crg-pre-commit.sh",
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
        """Each script must consume stdin (Cursor protocol)."""
        scripts = _cursor_hook_scripts()
        for name, content in scripts.items():
            assert "cat > /dev/null" in content, f"{name} missing stdin consumption"

    def test_update_script_runs_update(self):
        scripts = _cursor_hook_scripts()
        assert "dagayn update --skip-flows" in scripts["crg-update.sh"]

    def test_session_start_script_runs_status(self):
        scripts = _cursor_hook_scripts()
        assert "dagayn status" in scripts["crg-session-start.sh"]

    def test_pre_commit_script_runs_detect_changes(self):
        scripts = _cursor_hook_scripts()
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
        # Each event type should have exactly 1 crg hook
        for event, entries in data["hooks"].items():
            crg_hooks = [h for h in entries if "crg-" in h.get("command", "")]
            assert len(crg_hooks) == 1, f"{event} has {len(crg_hooks)} crg hooks after reinstall"

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

    def test_uvx_fallback(self, monkeypatch):
        """Not in Poetry/uv but uvx available -> use uvx."""
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
        assert "dagayn status" in content

    def test_hooks_tool_execute_before_event(self):
        content = _opencode_plugin_content()
        assert '"tool.execute.before"' in content
        assert "dagayn detect-changes --brief" in content

    def test_has_git_commit_detection(self):
        """Pre-commit hook should match git commit commands."""
        content = _opencode_plugin_content()
        assert "git" in content
        assert "commit" in content

    def test_all_handlers_have_try_catch(self):
        """Every event handler must use try/catch for graceful failure."""
        content = _opencode_plugin_content()
        # Count the three event registrations and ensure catch blocks
        assert content.count("} catch") >= 3


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
