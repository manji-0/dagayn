"""Local embedding server orchestration for Qwen local presets.

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
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, cast
from urllib.parse import urlparse

LocalEmbeddingLevel = Literal["low"]
LocalEmbeddingRuntime = Literal["llama"]
EmbeddingTextMode = Literal["metadata", "body"]

DEFAULT_LOCAL_EMBEDDING_PORT = 18080
DEFAULT_LOCAL_EMBEDDING_BIN = "auto"
DEFAULT_LOCAL_EMBEDDING_TIMEOUT = 300
_DEFAULT_LOCAL_EMBEDDING_BINARIES: dict[LocalEmbeddingRuntime, str] = {
    "llama": "llama-server",
}


@dataclass(frozen=True)
class LocalEmbeddingPreset:
    """Configuration for a local Qwen embedding model."""

    level: LocalEmbeddingLevel
    runtime: LocalEmbeddingRuntime
    repo_id: str
    quant: str | None
    model: str
    dimension: int
    text_mode: EmbeddingTextMode = "metadata"
    batch: int | None = None
    ubatch: int | None = None
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
    status: Literal["ready", "unreachable", "not_ready", "incompatible"]
    detail: str = ""


LOCAL_EMBEDDING_PRESETS: dict[
    LocalEmbeddingLevel,
    dict[LocalEmbeddingRuntime, LocalEmbeddingPreset],
] = {
    "low": {
        "llama": LocalEmbeddingPreset(
            level="low",
            runtime="llama",
            repo_id="Qwen/Qwen3-Embedding-0.6B-GGUF",
            quant="Q8_0",
            model="qwen3-embedding-0.6b-gguf-q8_0",
            dimension=1024,
            text_mode="metadata",
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
            return cast(LocalEmbeddingRuntime, normalized)
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
    selected_level = cast(LocalEmbeddingLevel, normalized)
    presets = LOCAL_EMBEDDING_PRESETS[selected_level]
    if selected_runtime not in presets:
        runtime_choices = ", ".join(sorted(presets))
        raise ValueError(
            f"Unknown local embedding runtime '{selected_runtime}' for preset "
            f"'{normalized}'. Expected one of: {runtime_choices}."
        )
    return presets[cast(LocalEmbeddingRuntime, selected_runtime)]


def local_embedding_base_url(port: int) -> str:
    """Return the localhost OpenAI-compatible base URL for *port*."""
    return f"http://127.0.0.1:{port}/v1"


def infer_local_embedding_provider(
    provider_name: str,
) -> PersistedLocalEmbeddingProvider | None:
    """Infer a managed local embedding preset from a persisted provider name.

    Only localhost OpenAI-compatible provider identities are accepted.  This is
    used by ``dagayn serve`` to keep semantic search live when the graph DB
    already contains local Qwen vectors, without ever guessing cloud providers.
    """
    prefix = "openai:"
    if not provider_name.startswith(prefix):
        return None
    try:
        model, base_url = provider_name[len(prefix) :].rsplit("@", 1)
    except ValueError:
        return None

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


def _probe_embedding_server(
    base_url: str,
    model: str,
    expected_dimension: int,
    timeout: float = 2.0,
) -> _ProbeResult:
    """Probe an OpenAI-compatible embeddings endpoint."""
    payload = json.dumps({"model": model, "input": ["dagayn local embedding probe"]}).encode(
        "utf-8"
    )
    req = urllib.request.Request(
        f"{base_url.rstrip('/')}/embeddings",
        data=payload,
        headers={
            "Content-Type": "application/json",
            "Authorization": "Bearer dagayn-local",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # nosec B310
            body = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        try:
            detail = exc.read().decode("utf-8", errors="replace")
        except Exception:  # nosec B110
            detail = str(exc)
        if exc.code == 429 or 500 <= exc.code < 600:
            return _ProbeResult("not_ready", f"HTTP {exc.code}: {detail}")
        return _ProbeResult("incompatible", f"HTTP {exc.code}: {detail}")
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        return _ProbeResult("unreachable", str(exc))
    except json.JSONDecodeError as exc:
        return _ProbeResult("incompatible", f"invalid JSON response: {exc}")

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
        "last",
    ]
    if preset.flash_attention:
        command.append("--flash-attn")
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
    except Exception:
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
    port: int = DEFAULT_LOCAL_EMBEDDING_PORT,
    binary: str = DEFAULT_LOCAL_EMBEDDING_BIN,
    keep_running: bool = False,
    startup_timeout: int = DEFAULT_LOCAL_EMBEDDING_TIMEOUT,
) -> Iterator[LocalEmbeddingServer]:
    """Ensure a Qwen local embedding server is ready for one build/update run.

    If a compatible server is already listening on *port*, it is reused and
    never terminated by dagayn. Otherwise dagayn starts the selected local
    model server and, unless *keep_running* is true, stops it when the context
    exits.
    """
    preset = get_local_embedding_preset(level, runtime=runtime)
    base_url = local_embedding_base_url(port)
    probe = _probe_embedding_server(base_url, preset.model, preset.dimension)
    if probe.status == "ready":
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
        if probe.status == "ready":
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
                if probe.status == "ready":
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
        proc = subprocess.Popen(  # noqa: S603
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        deadline = time.monotonic() + startup_timeout
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                raise RuntimeError(
                    "Local embedding server exited before the endpoint became ready "
                    f"(exit code {proc.returncode}). Command: {shlex.join(command)}"
                )
            probe = _probe_embedding_server(base_url, preset.model, preset.dimension)
            if probe.status == "ready":
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
            time.sleep(0.5)
        raise RuntimeError(
            "Timed out waiting for local embedding server to expose /v1/embeddings. "
            f"Command: {shlex.join(command)}"
        )
    finally:
        if proc is not None and (not keep_running or not ready) and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)
        _release_local_embedding_port_lock(port_lock)
