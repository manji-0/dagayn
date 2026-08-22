"""Local embedding server orchestration for local GGUF embedding presets.

This module intentionally treats model servers as external executables. dagayn
owns preset selection, readiness checks, and subprocess lifecycle; it does not
import or wrap llama.cpp or MLX server internals.
"""

from __future__ import annotations

import json
import os
import shlex
import shutil
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from collections.abc import Generator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import IO, Literal
from urllib.parse import urlparse

from .state_types import LocalEmbeddingProbeStatus

LocalEmbeddingLevel = Literal["bge-m3", "low"]
LocalEmbeddingRuntime = Literal["llama"]
EmbeddingTextMode = Literal["metadata", "body", "material"]

DEFAULT_LOCAL_EMBEDDING_PORT = 18080
DEFAULT_LOCAL_EMBEDDING_PORTS: dict[LocalEmbeddingLevel, int] = {
    "bge-m3": 18080,
    "low": 18081,
}
DEFAULT_LOCAL_EMBEDDING_BIN = "auto"
DEFAULT_LOCAL_EMBEDDING_TIMEOUT = 300
_DEFAULT_LOCAL_EMBEDDING_BINARIES: dict[LocalEmbeddingRuntime, str] = {
    "llama": "llama-server",
}


@dataclass(frozen=True)
class LocalEmbeddingPreset:
    """Configuration for a local GGUF embedding model."""

    level: LocalEmbeddingLevel
    runtime: LocalEmbeddingRuntime
    repo_id: str
    quant: str | None
    model: str
    dimension: int
    default_port: int = DEFAULT_LOCAL_EMBEDDING_PORT
    text_mode: EmbeddingTextMode = "metadata"
    batch: int | None = None
    ubatch: int | None = None
    pooling: str = "last"
    flash_attention: bool = False
    cache_type_k: str | None = None
    cache_type_v: str | None = None
    request_max_length: int | None = None

    @property
    def hf_selector(self) -> str:
        return f"{self.repo_id}:{self.quant}" if self.quant else self.repo_id

    @property
    def default_binary(self) -> str:
        return _DEFAULT_LOCAL_EMBEDDING_BINARIES[self.runtime]


@dataclass(frozen=True)
class LocalEmbeddingServer:
    """A ready OpenAI-compatible local embedding endpoint."""

    preset: LocalEmbeddingPreset
    base_url: str
    command: list[str]
    started: bool


@dataclass(frozen=True)
class PersistedLocalEmbeddingProvider:
    """Local embedding preset inferred from a persisted provider identity."""

    level: LocalEmbeddingLevel
    runtime: LocalEmbeddingRuntime
    model: str
    base_url: str
    port: int


@dataclass(frozen=True)
class _ProbeResult:
    status: LocalEmbeddingProbeStatus
    detail: str = ""

    @property
    def ready(self) -> bool:
        return self.status == "ready"


LOCAL_EMBEDDING_PRESETS: dict[
    LocalEmbeddingLevel,
    dict[LocalEmbeddingRuntime, LocalEmbeddingPreset],
] = {
    "bge-m3": {
        "llama": LocalEmbeddingPreset(
            level="bge-m3",
            runtime="llama",
            repo_id="gpustack/bge-m3-GGUF",
            quant="Q8_0",
            model="bge-m3-gguf-q8_0",
            dimension=1024,
            default_port=DEFAULT_LOCAL_EMBEDDING_PORTS["bge-m3"],
            text_mode="material",
            batch=8192,
            ubatch=8192,
            pooling="cls",
            flash_attention=True,
            cache_type_k="f16",
            cache_type_v="f16",
        ),
    },
    "low": {
        "llama": LocalEmbeddingPreset(
            level="low",
            runtime="llama",
            repo_id="Qwen/Qwen3-Embedding-0.6B-GGUF",
            quant="Q8_0",
            model="qwen3-embedding-0.6b-gguf-q8_0",
            dimension=1024,
            default_port=DEFAULT_LOCAL_EMBEDDING_PORTS["low"],
            text_mode="material",
            batch=8192,
            ubatch=8192,
            flash_attention=True,
            cache_type_k="f16",
            cache_type_v="f16",
        ),
    },
}


def _default_local_embedding_runtime() -> LocalEmbeddingRuntime:
    configured = os.environ.get("DAGAYN_LOCAL_EMBEDDING_RUNTIME")
    if configured:
        normalized = configured.strip().lower()
        if normalized == "llama":
            return normalized
        raise ValueError("DAGAYN_LOCAL_EMBEDDING_RUNTIME must be: llama.")
    return "llama"


def get_local_embedding_preset(
    level: str,
    *,
    runtime: str | None = None,
) -> LocalEmbeddingPreset:
    """Resolve a user-facing local embedding preset name."""
    normalized = level.strip().lower()
    if normalized == "0.8b":
        normalized = "low"
    choices = ", ".join(["none", *LOCAL_EMBEDDING_PRESETS])
    if normalized not in LOCAL_EMBEDDING_PRESETS:
        raise ValueError(f"Unknown local embedding preset '{level}'. Expected one of: {choices}.")

    selected_runtime = (runtime or _default_local_embedding_runtime()).strip().lower()
    selected_level = normalized
    presets = LOCAL_EMBEDDING_PRESETS[selected_level]
    if selected_runtime not in presets:
        runtime_choices = ", ".join(sorted(presets))
        raise ValueError(
            f"Unknown local embedding runtime '{selected_runtime}' for preset "
            f"'{normalized}'. Expected one of: {runtime_choices}."
        )
    return presets[selected_runtime]


def local_embedding_base_url(port: int) -> str:
    """Return the localhost OpenAI-compatible base URL for *port*."""
    return f"http://127.0.0.1:{port}/v1"


def resolve_local_embedding_port(port: int | None, level: str | None = None) -> int:
    """Return *port*, or the preset default when the caller did not set one.

    Presets share neither a process nor a default port: ``bge-m3`` listens on
    18080 and ``low`` on 18081, so ``--keep-local-embedding-server`` cannot
    hand a Qwen build the BGE-M3 weights still sitting on 18080.
    """
    if port is not None:
        return port
    if level and level.strip().lower() not in {"", "none"}:
        try:
            return get_local_embedding_preset(level).default_port
        except ValueError:
            pass
    return DEFAULT_LOCAL_EMBEDDING_PORT


def infer_local_embedding_provider(
    provider_name: str,
) -> PersistedLocalEmbeddingProvider | None:
    """Infer a managed local embedding preset from a persisted provider name.

    Only localhost OpenAI-compatible provider identities are accepted.  This is
    used by ``dagayn serve`` to keep semantic search live when the graph DB
    already contains local Qwen vectors, without ever guessing cloud providers.
    """
    prefix = "openai:"
    from .embeddings_providers import (
        _parse_openai_identity_suffixes,
        embedding_provider_base_name,
    )

    provider_name = embedding_provider_base_name(provider_name)
    if not provider_name.startswith(prefix):
        return None
    try:
        model, base_url = provider_name[len(prefix) :].rsplit("@", 1)
    except ValueError:
        return None

    base_url, _, _ = _parse_openai_identity_suffixes(base_url)
    parsed = urlparse(base_url)
    host = (parsed.hostname or "").lower()
    if host not in {"127.0.0.1", "localhost", "::1"}:
        return None
    if parsed.scheme.lower() != "http":
        return None
    port = parsed.port
    if port is None:
        return None

    for level, runtime_presets in LOCAL_EMBEDDING_PRESETS.items():
        for runtime, preset in runtime_presets.items():
            if preset.model == model:
                return PersistedLocalEmbeddingProvider(
                    level=level,
                    runtime=runtime,
                    model=model,
                    base_url=base_url,
                    port=port,
                )
    return None


def _model_id_matches(served: str, expected: str) -> bool:
    """Return True when a served model id names the expected alias."""
    served_n = served.strip().casefold()
    expected_n = expected.strip().casefold()
    if not served_n or not expected_n:
        return False
    if served_n == expected_n:
        return True
    served_name = Path(served_n).name
    if served_name == expected_n or Path(served_name).stem == expected_n:
        return True
    return served_n.endswith("/" + expected_n)


def _extract_model_ids(body: object) -> list[str]:
    """Collect ``id`` values from an OpenAI ``/v1/models`` payload.

    Embedding-shaped ``data`` rows (vectors, no ``id``) are ignored so a probe
    mock or a confused reverse-proxy cannot be mistaken for a model catalog.
    """
    if not isinstance(body, dict):
        return []
    data = body.get("data")
    if not isinstance(data, list):
        return []
    ids: list[str] = []
    for item in data:
        if not isinstance(item, dict):
            continue
        model_id = item.get("id")
        if isinstance(model_id, str) and model_id.strip() and "embedding" not in item:
            ids.append(model_id.strip())
    return ids


def _probe_http_json(
    url: str,
    *,
    data: bytes | None = None,
    timeout: float,
) -> tuple[_ProbeResult | None, object | None]:
    """GET or POST JSON. On failure return ``(probe, None)``; else ``(None, body)``."""
    headers = {"Authorization": "Bearer dagayn-local"}
    if data is not None:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # nosec B310
            return None, json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        try:
            detail = exc.read().decode("utf-8", errors="replace")
        except (OSError, UnicodeDecodeError):
            detail = str(exc)
        if exc.code == 429 or 500 <= exc.code < 600:
            return _ProbeResult("not_ready", f"HTTP {exc.code}: {detail}"), None
        return _ProbeResult("incompatible", f"HTTP {exc.code}: {detail}"), None
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        return _ProbeResult("unreachable", str(exc)), None
    except json.JSONDecodeError as exc:
        return _ProbeResult("incompatible", f"invalid JSON response: {exc}"), None


def _probe_served_model_identity(
    base_url: str,
    expected_model: str,
    timeout: float,
) -> tuple[_ProbeResult | None, bool]:
    """Compare ``GET /v1/models`` against *expected_model*.

    Returns ``(incompatible_probe, False)`` on a confirmed mismatch. Returns
    ``(None, True)`` when the catalog names the expected model. Returns
    ``(None, False)`` when identity cannot be verified (404, empty catalog,
    unreachable) so the embeddings probe can still classify the endpoint.
    """
    error, body = _probe_http_json(f"{base_url.rstrip('/')}/models", timeout=timeout)
    if error is not None:
        if error.status in {"not_ready", "unreachable"}:
            return None, False
        if error.status == "incompatible" and "HTTP 404" in error.detail:
            return None, False
        return error, False
    ids = _extract_model_ids(body)
    if not ids:
        return None, False
    if any(_model_id_matches(model_id, expected_model) for model_id in ids):
        return None, True
    served = ", ".join(ids)
    return (
        _ProbeResult(
            "incompatible",
            f"server is running {served!r}, not the requested model {expected_model!r}",
        ),
        False,
    )


def _probe_embedding_server(
    base_url: str,
    model: str,
    expected_dimension: int,
    timeout: float = 2.0,
) -> _ProbeResult:
    """Probe an OpenAI-compatible embeddings endpoint.

    Reachability and vector length are not enough: both local presets are
    1024-dim, and llama-server may ignore the request ``model`` field and
    embed with whatever weights are loaded. A catalog mismatch is a hard fail.
    """
    identity_error, catalog_confirmed = _probe_served_model_identity(base_url, model, timeout)
    if identity_error is not None:
        return identity_error

    payload = json.dumps({"model": model, "input": ["dagayn local embedding probe"]}).encode(
        "utf-8"
    )
    error, body = _probe_http_json(
        f"{base_url.rstrip('/')}/embeddings",
        data=payload,
        timeout=timeout,
    )
    if error is not None:
        return error
    if body is None:
        return _ProbeResult("incompatible", "empty embeddings response")

    if not catalog_confirmed:
        echoed = body.get("model") if isinstance(body, dict) else None
        if isinstance(echoed, str) and echoed.strip() and not _model_id_matches(echoed, model):
            return _ProbeResult(
                "incompatible",
                f"embeddings response model {echoed.strip()!r} did not match "
                f"requested model {model!r}",
            )

    data = body.get("data") if isinstance(body, dict) else None
    if isinstance(data, list) and data:
        embedding = data[0].get("embedding") if isinstance(data[0], dict) else None
        if isinstance(embedding, list) and embedding:
            if len(embedding) != expected_dimension:
                return _ProbeResult(
                    "incompatible",
                    f"embedding dimension {len(embedding)} did not match expected "
                    f"dimension {expected_dimension}",
                )
            return _ProbeResult("ready")
    return _ProbeResult("incompatible", "response did not contain an embedding vector")


def _resolve_binary(binary: str, preset: LocalEmbeddingPreset) -> str:
    requested = preset.default_binary if binary in ("", "auto") else binary
    resolved = shutil.which(requested)
    if resolved:
        return resolved
    candidate = Path(requested).expanduser()
    if candidate.exists():
        return str(candidate)
    raise RuntimeError(
        f"Could not find '{requested}'. Install llama.cpp or pass "
        "--local-embedding-bin /path/to/llama-server. See docs/LOCAL-EMBEDDINGS.md."
    )


def _server_command(
    preset: LocalEmbeddingPreset,
    binary: str,
    port: int,
) -> list[str]:
    command = [
        binary,
        "-hf",
        preset.hf_selector,
        "--embedding",
        "--pooling",
        preset.pooling,
    ]
    if preset.flash_attention:
        # llama.cpp requires an explicit mode; a bare --flash-attn makes the
        # next flag (e.g. --cache-type-k) parse as the mode value and exit 1.
        command += ["--flash-attn", "on"]
    if preset.cache_type_k is not None:
        command += ["--cache-type-k", preset.cache_type_k]
    if preset.cache_type_v is not None:
        command += ["--cache-type-v", preset.cache_type_v]
    if preset.batch is not None:
        command += ["-b", str(preset.batch)]
    if preset.ubatch is not None:
        command += ["-ub", str(preset.ubatch)]
    command += [
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--alias",
        preset.model,
    ]
    return command


#: Extra time granted whenever the child reports download progress. Keeps a
#: first-run model fetch from being killed by the readiness timeout, while still
#: bounding a server that has genuinely stalled.
_MODEL_DOWNLOAD_GRACE_SECONDS = 120.0

#: Substrings llama-server prints while fetching a model.
_DOWNLOAD_MARKERS = ("download", "curl", "%|", "resolving", "fetching")


def _stderr_size(stderr_file: IO[bytes] | None) -> int:
    """Return the current stderr byte count, or 0 when unavailable."""
    if stderr_file is None:
        return 0
    try:
        stderr_file.flush()
        return stderr_file.tell()
    except OSError:
        return 0


def _looks_like_model_download(stderr_file: IO[bytes] | None) -> bool:
    """True when the child's recent output looks like a model download."""
    tail = _stderr_tail(stderr_file, limit=4000).lower()
    return any(marker in tail for marker in _DOWNLOAD_MARKERS)


def _stderr_tail(stderr_file: IO[bytes] | None, *, limit: int = 2000) -> str:
    """Return a decoded stderr tail for startup-failure diagnostics."""
    if stderr_file is None:
        return ""
    try:
        stderr_file.flush()
        stderr_file.seek(0)
        raw = stderr_file.read()
    except OSError:
        return ""
    if not raw:
        return ""
    text = raw.decode("utf-8", errors="replace").strip()
    if not text:
        return ""
    if len(text) > limit:
        text = text[-limit:]
    return text


def _local_embedding_lock_path(port: int) -> Path:
    return Path.home() / ".dagayn" / f"local-embedding-{port}.lock"


def _acquire_local_embedding_port_lock(port: int):
    """Serialize managed local embedding server startup for one localhost port."""
    lock_path = _local_embedding_lock_path(port)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    lock_file = lock_path.open("a+", encoding="utf-8")
    try:
        try:
            import fcntl

            fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)
        except (ImportError, OSError):
            lock_file.close()
            return None
        lock_file.seek(0)
        lock_file.truncate()
        lock_file.write(f"{os.getpid()}\n")
        lock_file.flush()
        return lock_file
    except (OSError, ValueError):
        lock_file.close()
        raise


def _release_local_embedding_port_lock(lock_file) -> None:
    if lock_file is None:
        return
    try:
        try:
            import fcntl

            fcntl.flock(lock_file.fileno(), fcntl.LOCK_UN)
        except (ImportError, OSError):
            pass
    finally:
        lock_file.close()


@contextmanager
def local_embedding_server(
    level: str,
    *,
    runtime: str | None = None,
    port: int | None = None,
    binary: str = DEFAULT_LOCAL_EMBEDDING_BIN,
    keep_running: bool = False,
    startup_timeout: int = DEFAULT_LOCAL_EMBEDDING_TIMEOUT,
) -> Generator[LocalEmbeddingServer, None, None]:
    """Ensure a local embedding server is ready for one build/update run.

    If a compatible server is already listening on *port*, it is reused and
    never terminated by dagayn. Otherwise dagayn starts the selected local
    model server and, unless *keep_running* is true, stops it when the context
    exits. When *port* is omitted, the preset's default port is used so two
    kept-alive sidecars do not share 18080.
    """
    preset = get_local_embedding_preset(level, runtime=runtime)
    port = resolve_local_embedding_port(port, preset.level)
    base_url = local_embedding_base_url(port)
    probe = _probe_embedding_server(base_url, preset.model, preset.dimension)
    if probe.ready:
        yield LocalEmbeddingServer(preset=preset, base_url=base_url, command=[], started=False)
        return
    if probe.status == "incompatible":
        raise RuntimeError(
            f"Port {port} is already serving something, but it is not a compatible "
            f"embedding endpoint for preset '{preset.level}': {probe.detail}"
        )
    port_lock = _acquire_local_embedding_port_lock(port)
    proc: subprocess.Popen[bytes] | None = None
    ready = False
    try:
        probe = _probe_embedding_server(base_url, preset.model, preset.dimension)
        if probe.ready:
            _release_local_embedding_port_lock(port_lock)
            port_lock = None
            yield LocalEmbeddingServer(preset=preset, base_url=base_url, command=[], started=False)
            return
        if probe.status == "incompatible":
            raise RuntimeError(
                f"Port {port} is already serving something, but it is not a compatible "
                f"embedding endpoint for preset '{preset.level}': {probe.detail}"
            )
        if probe.status == "not_ready":
            deadline = time.monotonic() + startup_timeout
            while time.monotonic() < deadline:
                probe = _probe_embedding_server(base_url, preset.model, preset.dimension)
                if probe.ready:
                    _release_local_embedding_port_lock(port_lock)
                    port_lock = None
                    yield LocalEmbeddingServer(
                        preset=preset,
                        base_url=base_url,
                        command=[],
                        started=False,
                    )
                    return
                if probe.status == "incompatible":
                    raise RuntimeError(
                        f"Port {port} is already serving something, but it is not a compatible "
                        f"embedding endpoint for preset '{preset.level}': {probe.detail}"
                    )
                if probe.status == "unreachable":
                    break
                time.sleep(0.5)
            if probe.status == "not_ready":
                raise RuntimeError(
                    f"Timed out waiting for existing local embedding server on port {port}."
                )

        resolved_binary = _resolve_binary(binary, preset)
        command = _server_command(preset, resolved_binary, port)
        stderr_file: IO[bytes] | None = tempfile.TemporaryFile()
        try:
            proc = subprocess.Popen(  # noqa: S603
                command,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=stderr_file,
            )
            deadline = time.monotonic() + startup_timeout
            # A first run downloads the model (hundreds of MB for
            # ``-hf gpustack/bge-m3-GGUF:Q8_0``) inside this same window. On a
            # slow link the loop timed out, the ``finally`` below terminated the
            # downloading child, and the error blamed ``/v1/embeddings`` without
            # mentioning the download at all -- and retrying re-entered the same
            # race. While the child reports progress the deadline is extended.
            download_seen = False
            last_progress_size = _stderr_size(stderr_file)
            while time.monotonic() < deadline:
                if proc.poll() is not None:
                    detail = _stderr_tail(stderr_file)
                    message = (
                        "Local embedding server exited before the endpoint became ready "
                        f"(exit code {proc.returncode}). Command: {shlex.join(command)}"
                    )
                    if detail:
                        message = f"{message}\nstderr:\n{detail}"
                    raise RuntimeError(message)
                probe = _probe_embedding_server(base_url, preset.model, preset.dimension)
                if probe.ready:
                    ready = True
                    if keep_running:
                        _release_local_embedding_port_lock(port_lock)
                        port_lock = None
                    yield LocalEmbeddingServer(
                        preset=preset,
                        base_url=base_url,
                        command=command,
                        started=True,
                    )
                    return
                if probe.status == "incompatible":
                    raise RuntimeError(
                        "Local embedding server responded, but not as a compatible "
                        "embedding endpoint: "
                        f"{probe.detail}. Command: {shlex.join(command)}"
                    )
                current_size = _stderr_size(stderr_file)
                if current_size != last_progress_size:
                    last_progress_size = current_size
                    if _looks_like_model_download(stderr_file):
                        download_seen = True
                        deadline = max(
                            deadline,
                            time.monotonic() + _MODEL_DOWNLOAD_GRACE_SECONDS,
                        )
                time.sleep(0.5)
            detail = _stderr_tail(stderr_file)
            if download_seen:
                message = (
                    "Local embedding server is still downloading its model and did not "
                    "become ready in time. Re-run to resume the download, or pre-fetch "
                    f"the model. Command: {shlex.join(command)}"
                )
            else:
                message = (
                    "Timed out waiting for local embedding server to expose /v1/embeddings. "
                    f"Command: {shlex.join(command)}"
                )
            if detail:
                message = f"{message}\nstderr:\n{detail}"
            raise RuntimeError(message)
        finally:
            if stderr_file is not None:
                stderr_file.close()
                stderr_file = None
    finally:
        if proc is not None and (not keep_running or not ready) and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)
        _release_local_embedding_port_lock(port_lock)
