"""Tests for the `dagayn install` mode resolution and interactive prompt."""

from __future__ import annotations

import argparse

import pytest

from dagayn.cli.commands._shared import (
    _REMOTE_ENV_VARS,
    _prompt_install_mode,
    _resolve_install_mode,
)
from dagayn.cli.commands.init import handle


def _ns(**overrides) -> argparse.Namespace:
    """Build an argparse Namespace with sensible install-command defaults."""
    defaults = {
        "mode": None,
        "preset": None,
        "provider": None,
        "local_embedding": "none",
        "yes": False,
    }
    defaults.update(overrides)
    return argparse.Namespace(**defaults)


# ---------------------------------------------------------------------------
# _resolve_install_mode
# ---------------------------------------------------------------------------


class TestResolveInstallMode:
    def test_explicit_fts_only(self):
        assert _resolve_install_mode(_ns(mode="fts-only")) == ("fts-only", None, None)

    def test_legacy_explicit_local_with_preset_maps_to_llama(self):
        ns = _ns(mode="local", preset="low")
        assert _resolve_install_mode(ns) == ("local-embedding-llama", "low", None)

    def test_explicit_local_embedding_defaults_to_bge(self):
        assert _resolve_install_mode(_ns(mode="local-embedding")) == (
            "local-embedding",
            None,
            None,
        )

    def test_explicit_local_embedding_rejects_preset(self):
        with pytest.raises(SystemExit, match="does not accept --preset"):
            _resolve_install_mode(_ns(mode="local-embedding", preset="low"))

    def test_explicit_local_embedding_llama_defaults_to_low(self):
        assert _resolve_install_mode(_ns(mode="local-embedding-llama")) == (
            "local-embedding-llama",
            "low",
            None,
        )

    def test_explicit_remote_with_provider(self):
        ns = _ns(mode="remote-embedding", provider="openai")
        assert _resolve_install_mode(ns) == ("remote-embedding", None, "openai")

    def test_explicit_remote_requires_provider(self):
        with pytest.raises(SystemExit, match="--mode remote-embedding requires --provider"):
            _resolve_install_mode(_ns(mode="remote-embedding"))

    def test_legacy_local_embedding_low(self):
        ns = _ns(local_embedding="low")
        assert _resolve_install_mode(ns) == ("local-embedding-llama", "low", None)

    def test_legacy_local_embedding_bare_bge(self):
        ns = _ns(local_embedding="bge-m3")
        assert _resolve_install_mode(ns) == ("local-embedding", None, None)

    def test_legacy_local_embedding_rejects_removed_high(self):
        with pytest.raises(SystemExit, match="only supports bge-m3, low, or llama-qwen3"):
            _resolve_install_mode(_ns(local_embedding="high"))

    def test_explicit_mode_overrides_legacy(self):
        # --mode fts-only wins over --local-embedding low.
        ns = _ns(mode="fts-only", local_embedding="low")
        assert _resolve_install_mode(ns) == ("fts-only", None, None)

    def test_legacy_mode_aliases_are_accepted(self):
        assert _resolve_install_mode(_ns(mode="fts")) == ("fts-only", None, None)
        assert _resolve_install_mode(_ns(mode="local")) == ("local-embedding", None, None)
        assert _resolve_install_mode(_ns(mode="llama-qwen3")) == (
            "local-embedding-llama",
            "low",
            None,
        )
        assert _resolve_install_mode(_ns(mode="remote", provider="google")) == (
            "remote-embedding",
            None,
            "google",
        )

    def test_fail_fast_with_yes(self):
        with pytest.raises(SystemExit, match="--mode is required"):
            _resolve_install_mode(_ns(yes=True))

    def test_fail_fast_without_tty(self, monkeypatch):
        monkeypatch.setattr("sys.stdin.isatty", lambda: False)
        with pytest.raises(SystemExit, match="--mode is required"):
            _resolve_install_mode(_ns())

    def test_tty_falls_through_to_prompt(self, monkeypatch):
        """When --mode is omitted and stdin is a TTY, resolve delegates to the
        interactive prompt (here stubbed)."""
        monkeypatch.setattr("sys.stdin.isatty", lambda: True)
        monkeypatch.setattr(
            "dagayn.cli.commands._shared._prompt_install_mode",
            lambda: ("fts-only", None, None),
        )
        assert _resolve_install_mode(_ns()) == ("fts-only", None, None)


# ---------------------------------------------------------------------------
# _prompt_install_mode
# ---------------------------------------------------------------------------


def _scripted_input(monkeypatch, answers: list[str]) -> None:
    """Replace builtins.input with a scripted iterator over ``answers``."""
    it = iter(answers)
    monkeypatch.setattr("builtins.input", lambda _prompt="": next(it))


class TestPromptInstallMode:
    def test_picks_fts(self, monkeypatch):
        _scripted_input(monkeypatch, ["1"])
        assert _prompt_install_mode() == ("fts-only", None, None)

    def test_picks_local_embedding(self, monkeypatch):
        _scripted_input(monkeypatch, ["2"])
        assert _prompt_install_mode() == ("local-embedding", None, None)

    def test_picks_local_embedding_llama(self, monkeypatch):
        _scripted_input(monkeypatch, ["3"])
        assert _prompt_install_mode() == ("local-embedding-llama", "low", None)

    def test_picks_remote_openai(self, monkeypatch):
        _scripted_input(monkeypatch, ["4", "1"])
        assert _prompt_install_mode() == ("remote-embedding", None, "openai")

    def test_picks_remote_google(self, monkeypatch):
        _scripted_input(monkeypatch, ["4", "2"])
        assert _prompt_install_mode() == ("remote-embedding", None, "google")

    def test_picks_remote_minimax(self, monkeypatch):
        _scripted_input(monkeypatch, ["4", "3"])
        assert _prompt_install_mode() == ("remote-embedding", None, "minimax")

    def test_invalid_choice_reprompts(self, monkeypatch, capsys):
        _scripted_input(monkeypatch, ["x", "9", "1"])
        assert _prompt_install_mode() == ("fts-only", None, None)
        out = capsys.readouterr().out
        # Two rejection messages before the valid pick succeeds.
        assert out.count("Please enter one of:") == 2

    def test_eof_aborts(self, monkeypatch):
        def _raise(_p=""):
            raise EOFError()

        monkeypatch.setattr("builtins.input", _raise)
        with pytest.raises(SystemExit, match="Aborted"):
            _prompt_install_mode()


# ---------------------------------------------------------------------------
# _REMOTE_ENV_VARS sanity
# ---------------------------------------------------------------------------


class TestRemoteEnvVars:
    def test_keys_match_provider_choices(self):
        """Keys must mirror the --provider choice list so that any new
        provider added in the CLI is paired with an env-var list here."""
        assert set(_REMOTE_ENV_VARS.keys()) == {"openai", "google", "minimax"}

    def test_openai_lists_required_vars(self):
        # CRG_OPENAI_API_KEY / BASE_URL / MODEL are the three checked by
        # dagayn/embeddings.py::get_provider for the openai branch.
        assert "CRG_OPENAI_API_KEY" in _REMOTE_ENV_VARS["openai"]
        assert "CRG_OPENAI_BASE_URL" in _REMOTE_ENV_VARS["openai"]
        assert "CRG_OPENAI_MODEL" in _REMOTE_ENV_VARS["openai"]

    def test_google_lists_required_vars(self):
        assert _REMOTE_ENV_VARS["google"] == ["GOOGLE_API_KEY"]

    def test_minimax_lists_required_vars(self):
        assert _REMOTE_ENV_VARS["minimax"] == ["MINIMAX_API_KEY"]


class TestInstallHandleRemoteMode:
    def test_local_mode_bakes_bge_into_serve_args(self, tmp_path, monkeypatch):
        calls: list[dict] = []
        hook_calls: list[dict] = []

        monkeypatch.setattr(
            "dagayn.skills.install_platform_configs",
            lambda repo_root, **kwargs: (
                calls.append({"repo_root": repo_root, **kwargs}) or ["codex"]
            ),
        )
        monkeypatch.setattr("dagayn.skills.normalize_platform_target", lambda target: target)
        monkeypatch.setattr(
            "dagayn.cli.commands.init._instruction_files_to_modify",
            lambda *_args, **_kwargs: [],
        )
        monkeypatch.setattr(
            "dagayn.skills.install_codex_hooks",
            lambda repo_root, **kwargs: (
                hook_calls.append({"repo_root": repo_root, **kwargs}) or tmp_path / "hooks.json"
            ),
        )

        handle(
            argparse.Namespace(
                repo=str(tmp_path),
                dry_run=False,
                platform="codex",
                yes=True,
                no_instructions=True,
                mode="local-embedding",
                preset=None,
                provider=None,
                local_embedding="none",
            )
        )

        assert calls[0]["extra_serve_args"] == ["--local-embedding"]
        assert hook_calls[0]["extra_update_args"] == ["--local-embedding"]

    def test_llama_qwen3_mode_bakes_sidecar_into_serve_args(self, tmp_path, monkeypatch):
        calls: list[dict] = []
        hook_calls: list[dict] = []

        monkeypatch.setattr(
            "dagayn.skills.install_platform_configs",
            lambda repo_root, **kwargs: (
                calls.append({"repo_root": repo_root, **kwargs}) or ["codex"]
            ),
        )
        monkeypatch.setattr("dagayn.skills.normalize_platform_target", lambda target: target)
        monkeypatch.setattr(
            "dagayn.cli.commands.init._instruction_files_to_modify",
            lambda *_args, **_kwargs: [],
        )
        monkeypatch.setattr(
            "dagayn.skills.install_codex_hooks",
            lambda repo_root, **kwargs: (
                hook_calls.append({"repo_root": repo_root, **kwargs}) or tmp_path / "hooks.json"
            ),
        )

        handle(
            argparse.Namespace(
                repo=str(tmp_path),
                dry_run=False,
                platform="codex",
                yes=True,
                no_instructions=True,
                mode="local-embedding-llama",
                preset=None,
                provider=None,
                local_embedding="none",
            )
        )

        assert calls[0]["extra_serve_args"] == [
            "--local-embedding",
            "--mode",
            "llama-qwen3",
        ]
        assert hook_calls[0]["extra_update_args"] == [
            "--local-embedding",
            "--mode",
            "llama-qwen3",
        ]

    def test_local_embedding_mode_bakes_sidecar_options_into_hooks(self, tmp_path, monkeypatch):
        calls: list[dict] = []
        hook_calls: list[dict] = []

        monkeypatch.setattr(
            "dagayn.skills.install_platform_configs",
            lambda repo_root, **kwargs: (
                calls.append({"repo_root": repo_root, **kwargs}) or ["codex"]
            ),
        )
        monkeypatch.setattr("dagayn.skills.normalize_platform_target", lambda target: target)
        monkeypatch.setattr(
            "dagayn.cli.commands.init._instruction_files_to_modify",
            lambda *_args, **_kwargs: [],
        )
        monkeypatch.setattr(
            "dagayn.skills.install_codex_hooks",
            lambda repo_root, **kwargs: (
                hook_calls.append({"repo_root": repo_root, **kwargs}) or tmp_path / "hooks.json"
            ),
        )

        handle(
            argparse.Namespace(
                repo=str(tmp_path),
                dry_run=False,
                platform="codex",
                yes=True,
                no_instructions=True,
                mode="local-embedding",
                preset=None,
                provider=None,
                local_embedding="none",
                local_embedding_port=19093,
                local_embedding_bin="/opt/bin/llama-server",
                local_embedding_timeout=420,
            )
        )

        expected = [
            "--local-embedding",
            "--local-embedding-port",
            "19093",
            "--local-embedding-bin",
            "/opt/bin/llama-server",
            "--local-embedding-timeout",
            "420",
        ]
        assert calls[0]["extra_serve_args"] == expected
        assert hook_calls[0]["extra_update_args"] == expected

    def test_remote_mode_bakes_provider_into_serve_args(self, tmp_path, monkeypatch):
        calls: list[dict] = []
        hook_calls: list[dict] = []

        monkeypatch.setattr(
            "dagayn.skills.install_platform_configs",
            lambda repo_root, **kwargs: (
                calls.append({"repo_root": repo_root, **kwargs}) or ["codex"]
            ),
        )
        monkeypatch.setattr("dagayn.skills.normalize_platform_target", lambda target: target)
        monkeypatch.setattr(
            "dagayn.cli.commands.init._instruction_files_to_modify",
            lambda *_args, **_kwargs: [],
        )
        monkeypatch.setattr(
            "dagayn.skills.install_codex_hooks",
            lambda repo_root, **kwargs: (
                hook_calls.append({"repo_root": repo_root, **kwargs}) or tmp_path / "hooks.json"
            ),
        )

        handle(
            argparse.Namespace(
                repo=str(tmp_path),
                dry_run=False,
                platform="codex",
                yes=True,
                no_instructions=True,
                mode="remote-embedding",
                preset=None,
                provider="google",
                local_embedding="none",
            )
        )

        assert calls[0]["extra_serve_args"] == ["--remote-embedding", "google"]
        assert hook_calls[0]["extra_update_args"] is None
