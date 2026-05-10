"""Local embedding server orchestration for Qwen GGUF presets.

This module intentionally treats ``llama-server`` as an external executable.
dagayn owns preset selection, readiness checks, and subprocess lifecycle; it
does not import or wrap llama.cpp internals.
"""

from __future__ import annotations

import json
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
from typing import Literal

LocalEmbeddingLevel = Literal["low", "high"]

DEFAULT_LOCAL_EMBEDDING_PORT = 18080
DEFAULT_LOCAL_EMBEDDING_BIN = "llama-server"
DEFAULT_LOCAL_EMBEDDING_TIMEOUT = 300


@dataclass(frozen=True)
class LocalEmbeddingPreset:
    """Configuration for a local Qwen embedding model."""

    level: LocalEmbeddingLevel
    repo_id: str
    quant: str
    model: str
    dimension: int
    ubatch: int = 8192

    @property
    def hf_selector(self) -> str:
        return f"{self.repo_id}:{self.quant}"


@dataclass(frozen=True)
class LocalEmbeddingServer:
    """A ready OpenAI-compatible local embedding endpoint."""

    preset: LocalEmbeddingPreset
    base_url: str
    command: list[str]
    started: bool


@dataclass(frozen=True)
class _ProbeResult:
    status: Literal["ready", "unreachable", "not_ready", "incompatible"]
    detail: str = ""


LOCAL_EMBEDDING_PRESETS: dict[LocalEmbeddingLevel, LocalEmbeddingPreset] = {
    "low": LocalEmbeddingPreset(
        level="low",
        repo_id="Qwen/Qwen3-Embedding-0.6B-GGUF",
        quant="Q8_0",
        model="qwen3-embedding-0.6b-gguf-q8_0",
        dimension=1024,
    ),
    "high": LocalEmbeddingPreset(
        level="high",
        repo_id="Qwen/Qwen3-Embedding-4B-GGUF",
        quant="Q4_K_M",
        model="qwen3-embedding-4b-gguf-q4_k_m",
        dimension=2560,
    ),
}


def get_local_embedding_preset(level: str) -> LocalEmbeddingPreset:
    """Resolve a user-facing local embedding preset name."""
    normalized = level.strip().lower()
    if normalized == "0.8b":
        normalized = "low"
    if normalized not in LOCAL_EMBEDDING_PRESETS:
        choices = ", ".join(["none", *LOCAL_EMBEDDING_PRESETS])
        raise ValueError(f"Unknown local embedding preset '{level}'. Expected one of: {choices}.")
    return LOCAL_EMBEDDING_PRESETS[normalized]  # type: ignore[index]


def local_embedding_base_url(port: int) -> str:
    """Return the localhost OpenAI-compatible base URL for *port*."""
    return f"http://127.0.0.1:{port}/v1"


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


def _resolve_binary(binary: str) -> str:
    resolved = shutil.which(binary)
    if resolved:
        return resolved
    candidate = Path(binary).expanduser()
    if candidate.exists():
        return str(candidate)
    raise RuntimeError(
        f"Could not find '{binary}'. Install llama.cpp or pass "
        "--local-embedding-bin /path/to/llama-server. See docs/LOCAL-EMBEDDINGS.md."
    )


def _server_command(
    preset: LocalEmbeddingPreset,
    binary: str,
    port: int,
) -> list[str]:
    return [
        binary,
        "-hf",
        preset.hf_selector,
        "--embedding",
        "--pooling",
        "last",
        "-ub",
        str(preset.ubatch),
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--alias",
        preset.model,
    ]


@contextmanager
def local_embedding_server(
    level: str,
    *,
    port: int = DEFAULT_LOCAL_EMBEDDING_PORT,
    binary: str = DEFAULT_LOCAL_EMBEDDING_BIN,
    keep_running: bool = False,
    startup_timeout: int = DEFAULT_LOCAL_EMBEDDING_TIMEOUT,
) -> Iterator[LocalEmbeddingServer]:
    """Ensure a Qwen local embedding server is ready for one build/update run.

    If a compatible server is already listening on *port*, it is reused and
    never terminated by dagayn. Otherwise dagayn starts ``llama-server`` and,
    unless *keep_running* is true, stops it when the context exits.
    """
    preset = get_local_embedding_preset(level)
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
    if probe.status == "not_ready":
        deadline = time.monotonic() + startup_timeout
        while time.monotonic() < deadline:
            probe = _probe_embedding_server(base_url, preset.model, preset.dimension)
            if probe.status == "ready":
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

    resolved_binary = _resolve_binary(binary)
    command = _server_command(preset, resolved_binary, port)
    proc = subprocess.Popen(  # noqa: S603
        command,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.monotonic() + startup_timeout
    ready = False
    try:
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                raise RuntimeError(
                    "llama-server exited before the embedding endpoint became ready "
                    f"(exit code {proc.returncode}). Command: {shlex.join(command)}"
                )
            probe = _probe_embedding_server(base_url, preset.model, preset.dimension)
            if probe.status == "ready":
                ready = True
                yield LocalEmbeddingServer(
                    preset=preset,
                    base_url=base_url,
                    command=command,
                    started=True,
                )
                return
            if probe.status == "incompatible":
                raise RuntimeError(
                    "llama-server responded, but not as a compatible embedding endpoint: "
                    f"{probe.detail}. Command: {shlex.join(command)}"
                )
            time.sleep(0.5)
        raise RuntimeError(
            "Timed out waiting for llama-server to expose /v1/embeddings. "
            f"Command: {shlex.join(command)}"
        )
    finally:
        if (not keep_running or not ready) and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)
