from __future__ import annotations

import subprocess
import urllib.error

import pytest

from dagayn.local_embeddings import (
    _probe_embedding_server,
    _ProbeResult,
    get_local_embedding_preset,
    infer_local_embedding_provider,
    local_embedding_base_url,
    local_embedding_server,
)


class FakeProcess:
    def __init__(self) -> None:
        self.returncode = None
        self.terminated = False
        self.killed = False

    def poll(self):
        return self.returncode

    def terminate(self):
        self.terminated = True
        self.returncode = 0

    def kill(self):
        self.killed = True
        self.returncode = -9

    def wait(self, timeout=None):
        return self.returncode


def test_local_embedding_llama_preset_is_stable():
    low = get_local_embedding_preset("low", runtime="llama")

    assert low.runtime == "llama"
    assert low.hf_selector == "Qwen/Qwen3-Embedding-0.6B-GGUF:Q8_0"
    assert low.model == "qwen3-embedding-0.6b-gguf-q8_0"
    assert low.dimension == 1024
    assert low.text_mode == "metadata"
    assert low.batch == 8192
    assert low.ubatch == 8192
    assert low.flash_attention is True
    assert low.cache_type_k == "f16"
    assert low.cache_type_v == "f16"
    assert low.default_binary == "llama-server"


def test_local_embedding_llama_preset_is_default_on_apple_silicon(monkeypatch):
    monkeypatch.delenv("DAGAYN_LOCAL_EMBEDDING_RUNTIME", raising=False)

    low = get_local_embedding_preset("low")

    assert low.runtime == "llama"
    assert low.repo_id == "Qwen/Qwen3-Embedding-0.6B-GGUF"
    assert low.quant == "Q8_0"
    assert low.model == "qwen3-embedding-0.6b-gguf-q8_0"
    assert low.dimension == 1024
    assert low.request_max_length is None
    assert low.default_binary == "llama-server"


def test_local_embedding_rejects_removed_high_preset():
    with pytest.raises(ValueError, match="Expected one of: none, low"):
        get_local_embedding_preset("high")


def test_local_embedding_rejects_removed_runtime(monkeypatch):
    monkeypatch.setenv("DAGAYN_LOCAL_EMBEDDING_RUNTIME", "mlx")

    with pytest.raises(ValueError, match="DAGAYN_LOCAL_EMBEDDING_RUNTIME must be: llama"):
        get_local_embedding_preset("low")


def test_local_embedding_base_url_uses_openai_v1_path():
    assert local_embedding_base_url(18080) == "http://127.0.0.1:18080/v1"


def test_infer_local_embedding_provider_from_persisted_name():
    inferred = infer_local_embedding_provider(
        "openai:qwen3-embedding-0.6b-gguf-q8_0@http://127.0.0.1:19090/v1"
    )

    assert inferred is not None
    assert inferred.level == "low"
    assert inferred.runtime == "llama"
    assert inferred.model == "qwen3-embedding-0.6b-gguf-q8_0"
    assert inferred.port == 19090


def test_infer_local_embedding_provider_refuses_cloud_endpoint():
    assert (
        infer_local_embedding_provider(
            "openai:qwen3-embedding-0.6b-gguf-q8_0@https://api.example.com/v1"
        )
        is None
    )


def test_probe_rejects_wrong_embedding_dimension(monkeypatch):
    class Response:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc_val, exc_tb):
            return None

        def read(self):
            return b'{"data":[{"embedding":[1.0,2.0,3.0]}]}'

    monkeypatch.setattr(
        "dagayn.local_embeddings.urllib.request.urlopen", lambda *a, **k: Response()
    )

    result = _probe_embedding_server(
        "http://127.0.0.1:18080/v1",
        "qwen3-embedding-0.6b-gguf-q8_0",
        1024,
    )

    assert result.status == "incompatible"
    assert "dimension 3" in result.detail


def test_probe_treats_retryable_http_as_not_ready(monkeypatch):
    def raise_503(*args, **kwargs):
        raise urllib.error.HTTPError(
            url="http://127.0.0.1:18080/v1/embeddings",
            code=503,
            msg="loading",
            hdrs={},
            fp=None,
        )

    monkeypatch.setattr("dagayn.local_embeddings.urllib.request.urlopen", raise_503)

    result = _probe_embedding_server(
        "http://127.0.0.1:18080/v1",
        "qwen3-embedding-0.6b-gguf-q8_0",
        1024,
    )

    assert result.status == "not_ready"


def test_local_embedding_server_reuses_ready_endpoint(monkeypatch):
    popen_calls = []

    monkeypatch.setattr(
        "dagayn.local_embeddings._probe_embedding_server",
        lambda base_url, model, expected_dimension: _ProbeResult("ready"),
    )
    monkeypatch.setattr(
        "dagayn.local_embeddings.subprocess.Popen", lambda *a, **k: popen_calls.append(a)
    )

    with local_embedding_server("low", runtime="llama", port=18080) as server:
        assert server.started is False
        assert server.base_url == "http://127.0.0.1:18080/v1"
        assert server.command == []

    assert popen_calls == []


def test_local_embedding_server_starts_and_stops_llama_server(monkeypatch):
    probes = iter([_ProbeResult("unreachable"), _ProbeResult("unreachable"), _ProbeResult("ready")])
    fake_proc = FakeProcess()
    commands = []

    monkeypatch.setattr(
        "dagayn.local_embeddings._probe_embedding_server",
        lambda base_url, model, expected_dimension: next(probes),
    )
    monkeypatch.setattr("dagayn.local_embeddings.shutil.which", lambda binary: f"/bin/{binary}")

    def fake_popen(command, **kwargs):
        commands.append((command, kwargs))
        return fake_proc

    monkeypatch.setattr("dagayn.local_embeddings.subprocess.Popen", fake_popen)

    with local_embedding_server("low", runtime="llama", port=19090, startup_timeout=1) as server:
        assert server.started is True
        assert server.command[:3] == [
            "/bin/llama-server",
            "-hf",
            "Qwen/Qwen3-Embedding-0.6B-GGUF:Q8_0",
        ]
        assert "--embedding" in server.command
        assert "--pooling" in server.command
        assert "last" in server.command
        assert "--flash-attn" in server.command
        assert "--cache-type-k" in server.command
        assert "f16" in server.command
        assert "--cache-type-v" in server.command
        assert "-b" in server.command
        assert "-ub" in server.command
        assert "8192" in server.command
        assert "--alias" in server.command
        assert "qwen3-embedding-0.6b-gguf-q8_0" in server.command

    assert commands
    assert commands[0][1]["stdin"] is subprocess.DEVNULL
    assert commands[0][1]["stdout"] is subprocess.DEVNULL
    assert commands[0][1]["stderr"] is subprocess.DEVNULL
    assert fake_proc.terminated is True
    assert fake_proc.killed is False


def test_local_embedding_server_rechecks_after_port_lock(monkeypatch):
    probes = iter([_ProbeResult("unreachable"), _ProbeResult("ready")])
    popen_calls = []

    monkeypatch.setattr(
        "dagayn.local_embeddings._probe_embedding_server",
        lambda base_url, model, expected_dimension: next(probes),
    )
    monkeypatch.setattr(
        "dagayn.local_embeddings.subprocess.Popen", lambda *a, **k: popen_calls.append(a)
    )

    with local_embedding_server("low", runtime="llama", port=19091) as server:
        assert server.started is False
        assert server.command == []

    assert popen_calls == []


def test_local_embedding_server_keep_running_leaves_process(monkeypatch):
    probes = iter([_ProbeResult("unreachable"), _ProbeResult("unreachable"), _ProbeResult("ready")])
    fake_proc = FakeProcess()

    monkeypatch.setattr(
        "dagayn.local_embeddings._probe_embedding_server",
        lambda base_url, model, expected_dimension: next(probes),
    )
    monkeypatch.setattr("dagayn.local_embeddings.shutil.which", lambda binary: f"/bin/{binary}")
    monkeypatch.setattr("dagayn.local_embeddings.subprocess.Popen", lambda *a, **k: fake_proc)

    with local_embedding_server("low", runtime="llama", keep_running=True, startup_timeout=1):
        pass

    assert fake_proc.terminated is False


def test_local_embedding_server_rejects_incompatible_existing_port(monkeypatch):
    monkeypatch.setattr(
        "dagayn.local_embeddings._probe_embedding_server",
        lambda base_url, model, expected_dimension: _ProbeResult("incompatible", "not embeddings"),
    )

    with pytest.raises(RuntimeError, match="not a compatible embedding endpoint"):
        with local_embedding_server("low", runtime="llama"):
            pass
