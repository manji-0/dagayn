from __future__ import annotations

import json
import subprocess
import urllib.error
from email.message import Message

import pytest

from dagayn.local_embeddings import (
    _probe_embedding_server,
    _ProbeResult,
    get_local_embedding_preset,
    infer_local_embedding_provider,
    local_embedding_base_url,
    local_embedding_server,
    resolve_local_embedding_port,
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
    assert low.text_mode == "material"
    assert low.batch == 8192
    assert low.ubatch == 8192
    assert low.flash_attention is True
    assert low.cache_type_k == "f16"
    assert low.cache_type_v == "f16"
    assert low.default_port == 18081
    assert low.default_binary == "llama-server"


def test_local_embedding_bge_preset_is_stable():
    bge = get_local_embedding_preset("bge-m3", runtime="llama")

    assert bge.runtime == "llama"
    assert bge.hf_selector == "gpustack/bge-m3-GGUF:Q8_0"
    assert bge.model == "bge-m3-gguf-q8_0"
    assert bge.dimension == 1024
    assert bge.text_mode == "material"
    assert bge.batch == 8192
    assert bge.ubatch == 8192
    assert bge.pooling == "cls"
    assert bge.flash_attention is True
    assert bge.cache_type_k == "f16"
    assert bge.cache_type_v == "f16"
    assert bge.default_port == 18080
    assert bge.default_binary == "llama-server"


def test_local_embedding_llama_preset_is_default_on_apple_silicon(monkeypatch):
    monkeypatch.delenv("DAGAYN_LOCAL_EMBEDDING_RUNTIME", raising=False)

    low = get_local_embedding_preset("low")

    assert low.runtime == "llama"
    assert low.repo_id == "Qwen/Qwen3-Embedding-0.6B-GGUF"
    assert low.quant == "Q8_0"
    assert low.model == "qwen3-embedding-0.6b-gguf-q8_0"
    assert low.dimension == 1024
    assert low.request_max_length is None
    assert low.default_port == 18081
    assert low.default_binary == "llama-server"


def test_local_embedding_rejects_removed_high_preset():
    with pytest.raises(ValueError, match="Expected one of: none, bge-m3, low"):
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


def test_infer_bge_local_embedding_provider_from_persisted_name():
    inferred = infer_local_embedding_provider("openai:bge-m3-gguf-q8_0@http://127.0.0.1:19093/v1")

    assert inferred is not None
    assert inferred.level == "bge-m3"
    assert inferred.runtime == "llama"
    assert inferred.model == "bge-m3-gguf-q8_0"
    assert inferred.port == 19093


def test_infer_strips_text_mode_and_dim_suffixes():
    inferred = infer_local_embedding_provider(
        "openai:bge-m3-gguf-q8_0@http://127.0.0.1:18080/v1#dim=1024#text=material"
    )

    assert inferred is not None
    assert inferred.level == "bge-m3"
    assert inferred.port == 18080


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
            hdrs=Message(),
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
        flash_attn_idx = server.command.index("--flash-attn")
        assert server.command[flash_attn_idx + 1] == "on"
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
    assert commands[0][1]["stderr"] is not subprocess.DEVNULL
    assert hasattr(commands[0][1]["stderr"], "write")
    assert fake_proc.terminated is True
    assert fake_proc.killed is False


def test_local_embedding_server_starts_bge_llama_server(monkeypatch):
    probes = iter([_ProbeResult("unreachable"), _ProbeResult("unreachable"), _ProbeResult("ready")])
    fake_proc = FakeProcess()

    monkeypatch.setattr(
        "dagayn.local_embeddings._probe_embedding_server",
        lambda base_url, model, expected_dimension: next(probes),
    )
    monkeypatch.setattr("dagayn.local_embeddings.shutil.which", lambda binary: f"/bin/{binary}")
    monkeypatch.setattr("dagayn.local_embeddings.subprocess.Popen", lambda *a, **k: fake_proc)

    with local_embedding_server("bge-m3", runtime="llama", port=19093, startup_timeout=1) as server:
        assert server.started is True
        assert server.command[:3] == [
            "/bin/llama-server",
            "-hf",
            "gpustack/bge-m3-GGUF:Q8_0",
        ]
        assert "--embedding" in server.command
        assert "--pooling" in server.command
        assert "cls" in server.command
        assert "--alias" in server.command
        assert "bge-m3-gguf-q8_0" in server.command

    assert fake_proc.terminated is True


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

    with local_embedding_server(
        "low",
        runtime="llama",
        port=19092,
        keep_running=True,
        startup_timeout=1,
    ):
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


class _JsonResponse:
    def __init__(self, payload: bytes) -> None:
        self._payload = payload

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        return None

    def read(self):
        return self._payload


def _urlopen_by_path(*, models: bytes, embeddings: bytes):
    def urlopen(req, timeout=None):
        url = getattr(req, "full_url", str(req))
        if str(url).rstrip("/").endswith("/models"):
            return _JsonResponse(models)
        return _JsonResponse(embeddings)

    return urlopen


def test_resolve_local_embedding_port_uses_preset_defaults():
    assert resolve_local_embedding_port(None, "bge-m3") == 18080
    assert resolve_local_embedding_port(None, "low") == 18081
    assert resolve_local_embedding_port(19090, "low") == 19090
    assert resolve_local_embedding_port(None, None) == 18080


def test_local_embedding_server_low_defaults_to_preset_port(monkeypatch):
    monkeypatch.setattr(
        "dagayn.local_embeddings._probe_embedding_server",
        lambda base_url, model, expected_dimension: _ProbeResult("ready"),
    )
    monkeypatch.setattr("dagayn.local_embeddings.subprocess.Popen", lambda *a, **k: None)

    with local_embedding_server("low", runtime="llama") as server:
        assert server.base_url == "http://127.0.0.1:18081/v1"
        assert server.started is False


def _embedding_payload(model: str, dimension: int = 1024) -> bytes:
    return json.dumps({"model": model, "data": [{"embedding": [0.0] * dimension}]}).encode()


def test_probe_rejects_catalog_model_mismatch(monkeypatch):
    monkeypatch.setattr(
        "dagayn.local_embeddings.urllib.request.urlopen",
        _urlopen_by_path(
            models=b'{"data":[{"id":"bge-m3-gguf-q8_0"}]}',
            embeddings=_embedding_payload("qwen3-embedding-0.6b-gguf-q8_0"),
        ),
    )

    result = _probe_embedding_server(
        "http://127.0.0.1:18080/v1",
        "qwen3-embedding-0.6b-gguf-q8_0",
        1024,
    )

    assert result.status == "incompatible"
    assert "bge-m3-gguf-q8_0" in result.detail
    assert "qwen3-embedding-0.6b-gguf-q8_0" in result.detail


def test_probe_rejects_echoed_model_mismatch_when_catalog_missing(monkeypatch):
    monkeypatch.setattr(
        "dagayn.local_embeddings.urllib.request.urlopen",
        _urlopen_by_path(
            models=b'{"data":[]}',
            embeddings=_embedding_payload("bge-m3-gguf-q8_0"),
        ),
    )

    result = _probe_embedding_server(
        "http://127.0.0.1:18080/v1",
        "qwen3-embedding-0.6b-gguf-q8_0",
        1024,
    )

    assert result.status == "incompatible"
    assert "bge-m3-gguf-q8_0" in result.detail


def test_probe_accepts_matching_catalog_and_dimension(monkeypatch):
    monkeypatch.setattr(
        "dagayn.local_embeddings.urllib.request.urlopen",
        _urlopen_by_path(
            models=b'{"data":[{"id":"qwen3-embedding-0.6b-gguf-q8_0"}]}',
            embeddings=_embedding_payload("qwen3-embedding-0.6b-gguf-q8_0"),
        ),
    )

    result = _probe_embedding_server(
        "http://127.0.0.1:18081/v1",
        "qwen3-embedding-0.6b-gguf-q8_0",
        1024,
    )

    assert result.status == "ready"
