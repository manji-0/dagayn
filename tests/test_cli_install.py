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
    def test_explicit_fts(self):
        assert _resolve_install_mode(_ns(mode="fts")) == ("fts", None, None)

    def test_explicit_local_with_preset(self):
        ns = _ns(mode="local", preset="low")
        assert _resolve_install_mode(ns) == ("local", "low", None)

    def test_explicit_local_defaults_to_low(self):
        assert _resolve_install_mode(_ns(mode="local")) == ("local", "low", None)

    def test_explicit_local_rejects_removed_high_preset(self):
        with pytest.raises(SystemExit, match="only supports --preset low"):
            _resolve_install_mode(_ns(mode="local", preset="high"))

    def test_explicit_remote_with_provider(self):
        ns = _ns(mode="remote", provider="openai")
        assert _resolve_install_mode(ns) == ("remote", None, "openai")

    def test_explicit_remote_requires_provider(self):
        with pytest.raises(SystemExit, match="--mode remote requires --provider"):
            _resolve_install_mode(_ns(mode="remote"))

    def test_legacy_local_embedding_low(self):
        ns = _ns(local_embedding="low")
        assert _resolve_install_mode(ns) == ("local", "low", None)

    def test_legacy_local_embedding_rejects_removed_high(self):
        with pytest.raises(SystemExit, match="only supports low"):
            _resolve_install_mode(_ns(local_embedding="high"))

    def test_explicit_mode_overrides_legacy(self):
        # --mode fts wins over --local-embedding low.
        ns = _ns(mode="fts", local_embedding="low")
        assert _resolve_install_mode(ns) == ("fts", None, None)

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
            lambda: ("fts", None, None),
        )
        assert _resolve_install_mode(_ns()) == ("fts", None, None)


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
        assert _prompt_install_mode() == ("fts", None, None)

    def test_picks_local_low(self, monkeypatch):
        _scripted_input(monkeypatch, ["2"])
        assert _prompt_install_mode() == ("local", "low", None)

    def test_picks_remote_openai(self, monkeypatch):
        _scripted_input(monkeypatch, ["3", "1"])
        assert _prompt_install_mode() == ("remote", None, "openai")

    def test_picks_remote_google(self, monkeypatch):
        _scripted_input(monkeypatch, ["3", "2"])
        assert _prompt_install_mode() == ("remote", None, "google")

    def test_picks_remote_minimax(self, monkeypatch):
        _scripted_input(monkeypatch, ["3", "3"])
        assert _prompt_install_mode() == ("remote", None, "minimax")

    def test_invalid_choice_reprompts(self, monkeypatch, capsys):
        _scripted_input(monkeypatch, ["x", "9", "1"])
        assert _prompt_install_mode() == ("fts", None, None)
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
    def test_remote_mode_bakes_provider_into_serve_args(self, tmp_path, monkeypatch):
        calls: list[dict] = []

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

        handle(
            argparse.Namespace(
                repo=str(tmp_path),
                dry_run=True,
                platform="codex",
                yes=True,
                no_instructions=True,
                mode="remote",
                preset=None,
                provider="google",
                local_embedding="none",
            )
        )

        assert calls[0]["extra_serve_args"] == ["--remote-embedding", "google"]
