"""Vector embedding support for semantic code search.

Supports multiple providers:
1. Local (sentence-transformers) - Private, fast, offline.
2. Google Gemini - High-quality, cloud-based. Requires explicit opt-in.
3. MiniMax (embo-01) - High-quality 1536-dim cloud embeddings. Requires MINIMAX_API_KEY.
4. OpenAI-compatible - Any endpoint speaking OpenAI /v1/embeddings (real OpenAI,
   Azure OpenAI, self-hosted gateways like new-api / LiteLLM / vLLM / LocalAI / Ollama).
"""

from __future__ import annotations

import functools
import hashlib
import logging
import os
import re
import sqlite3
import struct
import sys
import time
from abc import ABC, abstractmethod
from pathlib import Path
from typing import TYPE_CHECKING, Any
from urllib.parse import urlparse

if TYPE_CHECKING:
    import numpy as np
else:
    try:
        import numpy as np

        _NUMPY_AVAILABLE = True
    except ImportError:
        np = None
        _NUMPY_AVAILABLE = False

from .graph import GraphNode, GraphStore

logger = logging.getLogger(__name__)

_DEFAULT_SLOW_EMBED_BATCH_SECONDS = 10.0
_DEFAULT_SOURCE_CHARS = 2048
_DEFAULT_DOC_BODY_WEIGHT = 2
_EMBEDDING_TEXT_MODES = {"metadata", "body", "material"}
_MARKDOWN_HEADING_RE = re.compile(r"^(#{1,6})\s+")
_DOC_BODY_KINDS = {"DocSection", "DocBody"}


def get_embedding_status(db_path: str | Path) -> dict[str, Any]:
    """Return read-only embedding coverage for a graph database."""
    path = Path(db_path)
    try:
        conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    except sqlite3.Error as exc:
        return {
            "status": "unavailable",
            "total_embeddings": 0,
            "provider_counts": {},
            "error": str(exc),
        }

    try:
        conn.row_factory = sqlite3.Row
        tables = {
            str(row["name"])
            for row in conn.execute(
                "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')"
            ).fetchall()
        }
        if "embeddings" not in tables:
            return {
                "status": "not_indexed",
                "total_embeddings": 0,
                "provider_counts": {},
            }

        total_embeddings = int(conn.execute("SELECT COUNT(*) FROM embeddings").fetchone()[0])
        provider_counts = {
            str(row["provider"]): int(row["count"])
            for row in conn.execute(
                "SELECT provider, COUNT(*) AS count FROM embeddings GROUP BY provider"
            ).fetchall()
        }
        status: dict[str, Any] = {
            "status": "empty" if total_embeddings == 0 else "unknown",
            "total_embeddings": total_embeddings,
            "provider_counts": provider_counts,
        }

        if "nodes" not in tables:
            return status

        embeddable_nodes = int(
            conn.execute("SELECT COUNT(*) FROM nodes WHERE kind != 'File'").fetchone()[0]
        )
        missing_embeddings = int(
            conn.execute(
                """
                SELECT COUNT(*)
                FROM nodes n
                LEFT JOIN embeddings e ON e.qualified_name = n.qualified_name
                WHERE n.kind != 'File' AND e.qualified_name IS NULL
                """
            ).fetchone()[0]
        )
        orphan_embeddings = int(
            conn.execute(
                """
                SELECT COUNT(*)
                FROM embeddings e
                LEFT JOIN nodes n ON n.qualified_name = e.qualified_name
                WHERE n.qualified_name IS NULL
                """
            ).fetchone()[0]
        )

        if total_embeddings == 0:
            state = "empty"
        elif orphan_embeddings:
            state = "stale"
        elif missing_embeddings:
            state = "partial"
        else:
            state = "complete"

        status.update(
            {
                "status": state,
                "embeddable_nodes": embeddable_nodes,
                "indexed_embeddings": total_embeddings - orphan_embeddings,
                "missing_embeddings": missing_embeddings,
                "orphan_embeddings": orphan_embeddings,
            }
        )
        return status
    except sqlite3.Error as exc:
        return {
            "status": "unavailable",
            "total_embeddings": 0,
            "provider_counts": {},
            "error": str(exc),
        }
    finally:
        conn.close()


# ---------------------------------------------------------------------------
# Provider Interface and Implementations
# ---------------------------------------------------------------------------


class EmbeddingProvider(ABC):
    @abstractmethod
    def embed(self, texts: list[str]) -> list[list[float]]:
        pass

    @abstractmethod
    def embed_query(self, text: str) -> list[float]:
        """Embed a search query (may use a different task type than indexing)."""
        pass

    @property
    @abstractmethod
    def dimension(self) -> int:
        pass

    @property
    @abstractmethod
    def name(self) -> str:
        pass

    @property
    def preferred_batch_size(self) -> int:
        """Number of texts to send per API call. Override in concrete providers."""
        return 64


LOCAL_DEFAULT_MODEL = "BAAI/bge-m3"


class LocalEmbeddingProvider(EmbeddingProvider):
    def __init__(self, model_name: str | None = None) -> None:
        self._model_name = model_name or os.environ.get("CRG_EMBEDDING_MODEL", LOCAL_DEFAULT_MODEL)
        self._model = None  # Lazy-loaded

    def _get_model(self):
        if self._model is None:
            try:
                import sentence_transformers

                self._model = sentence_transformers.SentenceTransformer(
                    self._model_name,
                    trust_remote_code=True,
                    model_kwargs={"trust_remote_code": True},
                )
            except ImportError:
                raise ImportError(
                    "sentence-transformers not installed. It is part of dagayn's "
                    "standard dependencies; reinstall or repair the dagayn environment."
                )
        return self._model

    def embed(self, texts: list[str]) -> list[list[float]]:
        model = self._get_model()
        vectors = model.encode(texts, show_progress_bar=False)
        return [v.tolist() for v in vectors]

    def embed_query(self, text: str) -> list[float]:
        return self.embed([text])[0]

    @property
    def dimension(self) -> int:
        model = self._get_model()
        return model.get_sentence_embedding_dimension()

    @property
    def name(self) -> str:
        return f"local:{self._model_name}"


class GoogleEmbeddingProvider(EmbeddingProvider):
    def __init__(self, api_key: str, model: str = "gemini-embedding-001") -> None:
        try:
            from google import genai

            self._client = genai.Client(api_key=api_key)
            self.model = model
            self._dimension: int | None = None
        except ImportError:
            raise ImportError(
                'google-generativeai not installed. Run: pip install "dagayn[google-embeddings] @ git+https://github.com/manji-0/dagayn.git"'
            )

    def embed(self, texts: list[str]) -> list[list[float]]:
        batch_size = 100
        results = []
        for i in range(0, len(texts), batch_size):
            batch = texts[i : i + batch_size]
            response = self._call_with_retry(
                lambda b=batch: self._client.models.embed_content(
                    model=self.model,
                    contents=b,
                    config={"task_type": "RETRIEVAL_DOCUMENT"},
                )
            )
            results.extend([e.values for e in response.embeddings])
        if self._dimension is None and results:
            self._dimension = len(results[0])
        return results

    @staticmethod
    def _call_with_retry(fn, max_retries: int = 3):
        """Call fn with exponential backoff on transient API errors."""
        for attempt in range(max_retries):
            try:
                return fn()
            except Exception as e:
                # Retry on rate-limit (429) or server errors (5xx)
                err_str = str(e)
                is_retryable = "429" in err_str or "500" in err_str or "503" in err_str
                if not is_retryable or attempt == max_retries - 1:
                    raise
                wait = 2**attempt
                logger.warning(
                    "Gemini API error (attempt %d/%d), retrying in %ds: %s",
                    attempt + 1,
                    max_retries,
                    wait,
                    e,
                )
                time.sleep(wait)

    def embed_query(self, text: str) -> list[float]:
        response = self._call_with_retry(
            lambda: self._client.models.embed_content(
                model=self.model,
                contents=[text],
                config={"task_type": "RETRIEVAL_QUERY"},
            )
        )
        vec = response.embeddings[0].values
        if self._dimension is None:
            self._dimension = len(vec)
        return vec

    @property
    def dimension(self) -> int:
        if self._dimension is not None:
            return self._dimension
        # Default for gemini-embedding-001; updated dynamically after first call
        return 768

    @property
    def name(self) -> str:
        return f"google:{self.model}"


class MiniMaxEmbeddingProvider(EmbeddingProvider):
    """MiniMax embo-01 embedding provider (1536 dimensions).

    Uses the MiniMax Embeddings API (https://api.minimax.io/v1/embeddings)
    with the embo-01 model. Requires the MINIMAX_API_KEY environment variable.
    """

    _ENDPOINT = "https://api.minimax.io/v1/embeddings"
    _MODEL = "embo-01"
    _DIMENSION = 1536

    def __init__(self, api_key: str) -> None:
        self._api_key = api_key

    def _call_api(self, texts: list[str], task_type: str) -> list[list[float]]:
        import json as _json
        import urllib.request

        payload = _json.dumps(
            {
                "model": self._MODEL,
                "texts": texts,
                "type": task_type,
            }
        ).encode("utf-8")

        req = urllib.request.Request(
            self._ENDPOINT,
            data=payload,
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {self._api_key}",
            },
        )

        max_retries = 3
        for attempt in range(max_retries):
            try:
                import ssl

                _ssl_ctx = ssl.create_default_context()
                with urllib.request.urlopen(req, timeout=60, context=_ssl_ctx) as resp:  # nosec B310
                    body = _json.loads(resp.read().decode("utf-8"))

                base_resp = body.get("base_resp", {})
                if base_resp.get("status_code", 0) != 0:
                    raise RuntimeError(
                        f"MiniMax API error: {base_resp.get('status_msg', 'unknown')}"
                    )

                return body["vectors"]
            except Exception as e:
                err_str = str(e)
                is_retryable = "429" in err_str or "500" in err_str or "503" in err_str
                if not is_retryable or attempt == max_retries - 1:
                    raise
                wait = 2**attempt
                logger.warning(
                    "MiniMax API error (attempt %d/%d), retrying in %ds: %s",
                    attempt + 1,
                    max_retries,
                    wait,
                    e,
                )
                time.sleep(wait)

        return []  # unreachable, but keeps mypy happy

    def embed(self, texts: list[str]) -> list[list[float]]:
        batch_size = 100
        results: list[list[float]] = []
        for i in range(0, len(texts), batch_size):
            batch = texts[i : i + batch_size]
            results.extend(self._call_api(batch, "db"))
        return results

    def embed_query(self, text: str) -> list[float]:
        return self._call_api([text], "query")[0]

    @property
    def dimension(self) -> int:
        return self._DIMENSION

    @property
    def name(self) -> str:
        return f"minimax:{self._MODEL}"


class OpenAIEmbeddingProvider(EmbeddingProvider):
    """OpenAI-compatible embedding provider.

    Works with any endpoint that speaks the OpenAI ``/v1/embeddings`` schema:
    - Real OpenAI API (``https://api.openai.com/v1``)
    - Azure OpenAI
    - Self-hosted gateways: new-api, LiteLLM, vLLM, LocalAI, Ollama (openai mode)

    Provider identity in ``name`` includes both the model and the endpoint
    host (``openai:{model}@{host}``), so switching base URL while keeping the
    same model ID re-partitions the embeddings table and forces a clean
    re-embed. This is the only defense against silently mixing vector spaces
    from different backends (e.g. real OpenAI vs. an OpenAI-compatible
    gateway that ships different weights under the same model name).

    Dimension is detected from the first response and frozen; switching the
    ``model`` in the environment also changes ``provider.name`` and triggers
    re-embed via the same isolation key.
    """

    _DEFAULT_BATCH_SIZE = 100

    # Default ports by scheme; stripped from the host_key so the user can't
    # accidentally force a re-embed by toggling an explicit default port.
    _DEFAULT_PORTS = {"http": 80, "https": 443}

    def __init__(
        self,
        api_key: str,
        base_url: str,
        model: str,
        dimension: int | None = None,
        timeout: int = 120,
        batch_size: int | None = None,
        max_length: int | None = None,
    ) -> None:
        self._api_key = api_key
        self._base_url = base_url.rstrip("/")
        self._model = model
        self._dimension = dimension
        self._timeout = timeout
        self._batch_size = batch_size or self._DEFAULT_BATCH_SIZE
        self._max_length = max_length
        self._host_key = self._make_host_key(self._base_url)

    @classmethod
    def from_persisted_name(
        cls,
        provider_name: str,
        *,
        api_key: str = "dagayn-local",
    ) -> "OpenAIEmbeddingProvider | None":
        """Recreate a localhost OpenAI-compatible provider from a DB identity.

        Persisted provider names include the model and endpoint identity, for
        example ``openai:qwen@http://127.0.0.1:18080/v1``.  This helper only
        accepts localhost endpoints so search can reuse local embeddings
        without requiring CRG_OPENAI_* env vars, while never guessing cloud
        credentials or silently sending code off-machine.
        """
        prefix = "openai:"
        if not provider_name.startswith(prefix):
            return None
        try:
            model, base_url = provider_name[len(prefix) :].rsplit("@", 1)
        except ValueError:
            return None
        max_length = None
        if "#max_length=" in base_url:
            base_url, raw_max_length = base_url.rsplit("#max_length=", 1)
            try:
                max_length = int(raw_max_length)
            except ValueError:
                return None
        if not model or not base_url or not _is_localhost_url(base_url):
            return None
        provider = cls(api_key=api_key, base_url=base_url, model=model, max_length=max_length)
        if provider.name != provider_name:
            return None
        return provider

    @classmethod
    def _make_host_key(cls, base_url: str) -> str:
        """Normalize the identity key used in ``provider.name``.

        Codex review pushed this well past naive ``netloc`` because that
        alone has three leaks:

        1. ``netloc`` preserves ``userinfo`` (``user:pass@host``) — we'd
           persist credentials into the DB's ``embeddings.provider`` column.
           Use ``hostname`` instead.
        2. Default ports (``:80`` for http, ``:443`` for https) are
           semantically identical to omitting the port; keeping them would
           cause spurious re-embeds when the user just spelled the URL
           differently.
        3. Path is part of the backend identity for path-routed gateways:
           ``https://gw/openai/v1`` and ``https://gw/vendor-b/v1`` front
           different models and must not share cached vectors.
        """
        parsed = urlparse(base_url)
        hostname = (parsed.hostname or "").lower()
        scheme = (parsed.scheme or "").lower()
        port = parsed.port
        if port and port != cls._DEFAULT_PORTS.get(scheme):
            # Bracket IPv6 literals when appending a port.
            host_part = f"[{hostname}]:{port}" if ":" in hostname else f"{hostname}:{port}"
        else:
            host_part = hostname
        # Preserve path routing. Trim any trailing slash and any
        # ``/embeddings`` suffix that callers may have included — we append
        # that ourselves when building the request URL.
        path = (parsed.path or "").rstrip("/")
        if path.endswith("/embeddings"):
            path = path[: -len("/embeddings")].rstrip("/")
        # Include scheme: http and https to the same host+path front
        # different endpoints in practice (plaintext vs TLS, dev vs prod
        # gateway), and sharing cached vectors across them is the same
        # silent-mixing failure mode as switching base URL entirely.
        return f"{scheme}://{host_part}{path}" if path else f"{scheme}://{host_part}"

    def _call_api(self, texts: list[str]) -> list[list[float]]:
        import http.client
        import json as _json
        import socket
        import ssl
        import urllib.error
        import urllib.request

        body: dict[str, Any] = {"model": self._model, "input": texts}
        # OpenAI v3 models (text-embedding-3-*) support dimension reduction;
        # only forward the param when the user explicitly pinned one.
        if self._dimension is not None:
            body["dimensions"] = self._dimension
        # Some local OpenAI-compatible embedding servers accept max_length as
        # an extra request field. Keep this opt-in so strict cloud APIs never
        # see a non-standard parameter.
        if self._max_length is not None:
            body["max_length"] = self._max_length

        payload = _json.dumps(body).encode("utf-8")
        req = urllib.request.Request(
            f"{self._base_url}/embeddings",
            data=payload,
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {self._api_key}",
            },
        )

        max_retries = 3
        for attempt in range(max_retries):
            try:
                _ssl_ctx = ssl.create_default_context()
                try:
                    with urllib.request.urlopen(  # nosec B310
                        req,
                        timeout=self._timeout,
                        context=_ssl_ctx,
                    ) as resp:
                        raw = resp.read().decode("utf-8")
                except urllib.error.HTTPError as http_err:
                    # 429 / 5xx: re-raise and let the outer retry loop handle it.
                    # (We must not convert to RuntimeError here or retry below
                    # can't tell it was a transient HTTP failure.)
                    if http_err.code == 429 or 500 <= http_err.code < 600:
                        raise
                    # Other 4xx: surface the API error body instead of a bare
                    # "400 Bad Request" — gateways like new-api return JSON
                    # with the real reason (batch size limits, invalid model,
                    # etc.) which is far more actionable.
                    try:
                        err_body = http_err.read().decode("utf-8", errors="replace")
                    except Exception:
                        err_body = ""
                    err_msg = err_body or str(http_err)
                    try:
                        parsed = _json.loads(err_body)
                        if isinstance(parsed, dict) and "error" in parsed:
                            err_obj = parsed["error"]
                            err_msg = (
                                err_obj.get("message", err_msg)
                                if isinstance(err_obj, dict)
                                else str(err_obj)
                            )
                    except Exception:  # nosec B110
                        # Non-JSON error body is fine: we already seeded
                        # err_msg with the raw body above, so fall through.
                        pass
                    raise RuntimeError(f"OpenAI API HTTP {http_err.code}: {err_msg}") from http_err

                response = _json.loads(raw)

                if "error" in response:
                    err = response["error"]
                    msg = err.get("message", "unknown") if isinstance(err, dict) else str(err)
                    raise RuntimeError(f"OpenAI API error: {msg}")

                data = response.get("data", [])
                if not data:
                    raise RuntimeError("OpenAI API returned empty data")
                # OpenAI spec: data[i].index maps to input[i], but some
                # compatible gateways re-order results or drop entries on
                # partial failure, and others omit `index` entirely. Three
                # disjoint cases:
                #   1. All items have a valid int ``index``: must form a
                #      permutation of 0..N-1, then sort and use.
                #   2. NO item carries an ``index`` field: trust server
                #      order, only verify count matches.
                #   3. Anything in between (partial indices, str indices,
                #      missing on some): refuse. Zipping server order in
                #      that case would happily misalign the indexed items.
                any_has_index = any("index" in item for item in data)
                all_int_index = all(isinstance(item.get("index"), int) for item in data)
                if all_int_index:
                    expected = set(range(len(texts)))
                    indices = [int(item["index"]) for item in data]
                    if len(set(indices)) != len(indices) or set(indices) != expected:
                        raise RuntimeError(
                            "OpenAI API returned malformed indices "
                            f"(got {indices}, expected permutation of "
                            f"0..{len(texts) - 1}) — refusing to misalign vectors."
                        )
                    data = sorted(data, key=lambda item: int(item["index"]))
                elif not any_has_index:
                    if len(data) != len(texts):
                        raise RuntimeError(
                            f"OpenAI API returned {len(data)} embeddings for "
                            f"{len(texts)} inputs with no index field — "
                            "refusing to misalign vectors."
                        )
                else:
                    # Mixed: some items have index, others don't (or carry
                    # non-int index). Server order would silently misplace
                    # the indexed items, so we refuse.
                    raise RuntimeError(
                        "OpenAI API returned mixed indexed/unindexed data — "
                        "refusing to misalign vectors."
                    )

                vectors = [item["embedding"] for item in data]
                if vectors and self._dimension is None:
                    self._dimension = len(vectors[0])
                return vectors

            except Exception as e:
                # Retryable = HTTP 429/5xx, network/timeout/TLS issues.
                # Non-retryable = HTTP 4xx (other), malformed responses,
                # misaligned data length — those are caller-side bugs that
                # will keep failing on retry.
                is_retryable = False
                if isinstance(e, urllib.error.HTTPError):
                    is_retryable = e.code == 429 or 500 <= e.code < 600
                elif isinstance(
                    e,
                    (
                        urllib.error.URLError,
                        socket.timeout,
                        TimeoutError,
                        ConnectionError,
                        ssl.SSLError,
                        # Reverse proxies and edge gateways surface transient
                        # disconnects as these stdlib classes. Real incidents
                        # have been observed on Cloudflare-fronted endpoints
                        # and on LiteLLM when upstream providers hiccup.
                        http.client.IncompleteRead,
                        http.client.BadStatusLine,
                        http.client.RemoteDisconnected,
                    ),
                ):
                    is_retryable = True
                if not is_retryable or attempt == max_retries - 1:
                    raise
                wait = 2**attempt
                logger.warning(
                    "OpenAI embeddings API error (attempt %d/%d), retrying in %ds: %s",
                    attempt + 1,
                    max_retries,
                    wait,
                    e,
                )
                time.sleep(wait)

        return []  # unreachable

    def embed(self, texts: list[str]) -> list[list[float]]:
        if not texts:
            return []
        results: list[list[float]] = []
        for i in range(0, len(texts), self._batch_size):
            results.extend(self._call_api(texts[i : i + self._batch_size]))
        return results

    def embed_query(self, text: str) -> list[float]:
        return self._call_api([text])[0]

    @property
    def preferred_batch_size(self) -> int:
        return self._batch_size

    @property
    def dimension(self) -> int:
        if self._dimension is not None:
            return self._dimension
        # Default for text-embedding-3-small; updated after first call.
        return 1536

    @property
    def name(self) -> str:
        # Endpoint-aware identity: model alone is NOT enough — two backends
        # can serve the same model ID with different weights or dimensions,
        # and re-using cached embeddings across them silently corrupts
        # semantic ranking. Including the host partitions the embeddings
        # table so switching CRG_OPENAI_BASE_URL triggers a safe re-embed.
        suffix = f"#max_length={self._max_length}" if self._max_length is not None else ""
        return f"openai:{self._model}@{self._host_key}{suffix}"


CLOUD_PROVIDERS = {"google", "minimax", "openai"}


def _is_localhost_url(url: str) -> bool:
    """Return True if url points to a localhost host (never treat as cloud egress).

    Uses urlparse.hostname so we compare the actual host, not a substring
    match that could be fooled by e.g. ``https://my-openai.127.0.0.1.nip.io``.
    """
    try:
        host = (urlparse(url).hostname or "").lower()
    except Exception:
        return False
    # nosec B104: we're *matching* a URL hostname, not binding a listener.
    return host in {"127.0.0.1", "localhost", "0.0.0.0", "::1"}  # nosec B104


def _warn_cloud_egress(provider_name: str) -> None:
    """Print a stderr warning before a cloud embedding provider is used.

    The warning is suppressed when ``CRG_ACCEPT_CLOUD_EMBEDDINGS=1`` is
    set in the environment, so scripted / CI workloads can acknowledge
    once and move on. Use stderr (never stdin/input) to stay compatible
    with the MCP stdio transport — anything we write to stdout would
    corrupt the JSON-RPC stream. See: #174
    """
    if os.environ.get("CRG_ACCEPT_CLOUD_EMBEDDINGS", "").strip() == "1":
        return
    print(
        f"\n⚠️  dagayn: about to embed code via the '{provider_name}' "
        "cloud provider.\n"
        "    Your source code (function names, docstrings, file paths) will be "
        "sent to an external API.\n"
        "    This is necessary for semantic search with the cloud provider you "
        "selected.\n"
        "    To skip this warning in future runs, set "
        "CRG_ACCEPT_CLOUD_EMBEDDINGS=1 in your environment.\n"
        "    To stay fully offline, use the default 'local' provider instead "
        "(no API key needed).\n",
        file=sys.stderr,
    )


def get_provider(
    provider: str | None = None,
    model: str | None = None,
) -> EmbeddingProvider | None:
    """Get an embedding provider by name.

    Args:
        provider: Provider name. One of "local", "google", "minimax", "openai",
                  or None for local.
                  Google requires GOOGLE_API_KEY env var and explicit opt-in.
                  MiniMax requires MINIMAX_API_KEY env var and explicit opt-in.
                  OpenAI requires CRG_OPENAI_API_KEY + CRG_OPENAI_BASE_URL +
                  CRG_OPENAI_MODEL env vars (or the ``model`` arg). The egress
                  warning is skipped when the base URL points to localhost.
                  Cloud providers emit a one-time stderr warning before use
                  unless ``CRG_ACCEPT_CLOUD_EMBEDDINGS=1`` is set. See: #174
        model: Model name/path to use. For local provider this is any
               sentence-transformers compatible model. Falls back to
               CRG_EMBEDDING_MODEL env var, then to BAAI/bge-m3.
               For Google provider this is a Gemini model ID.
               For OpenAI provider this overrides CRG_OPENAI_MODEL.
    """
    if provider == "openai":
        api_key = os.environ.get("CRG_OPENAI_API_KEY")
        base_url = os.environ.get("CRG_OPENAI_BASE_URL")
        resolved_model = model or os.environ.get("CRG_OPENAI_MODEL")
        if not api_key or not base_url or not resolved_model:
            missing = [
                name
                for name, val in [
                    ("CRG_OPENAI_API_KEY", api_key),
                    ("CRG_OPENAI_BASE_URL", base_url),
                    ("CRG_OPENAI_MODEL", resolved_model),
                ]
                if not val
            ]
            raise ValueError(
                "Missing required environment variable(s) for the OpenAI "
                f"embedding provider: {', '.join(missing)}."
            )
        dim_env = os.environ.get("CRG_OPENAI_DIMENSION")
        dimension = int(dim_env) if dim_env else None
        batch_env = os.environ.get("CRG_OPENAI_BATCH_SIZE")
        batch_size = int(batch_env) if batch_env else None
        timeout_env = os.environ.get("CRG_OPENAI_TIMEOUT")
        timeout = int(timeout_env) if timeout_env else 120
        max_length_env = os.environ.get("CRG_OPENAI_MAX_LENGTH")
        max_length = int(max_length_env) if max_length_env else None
        if not _is_localhost_url(base_url):
            _warn_cloud_egress("openai")
        return OpenAIEmbeddingProvider(
            api_key=api_key,
            base_url=base_url,
            model=resolved_model,
            dimension=dimension,
            batch_size=batch_size,
            timeout=timeout,
            max_length=max_length,
        )

    if provider == "minimax":
        api_key = os.environ.get("MINIMAX_API_KEY")
        if not api_key:
            raise ValueError(
                "MINIMAX_API_KEY environment variable is required for "
                "the MiniMax embedding provider."
            )
        _warn_cloud_egress("minimax")
        return MiniMaxEmbeddingProvider(api_key=api_key)

    if provider == "google":
        api_key = os.environ.get("GOOGLE_API_KEY")
        if not api_key:
            raise ValueError(
                "GOOGLE_API_KEY environment variable is required for the Google embedding provider."
            )
        _warn_cloud_egress("google")
        try:
            return GoogleEmbeddingProvider(
                api_key=api_key,
                **({"model": model} if model else {}),
            )
        except ImportError:
            return None

    # Default: local
    try:
        return LocalEmbeddingProvider(model_name=model)
    except ImportError:
        return None


def provider_from_persisted_name(provider_name: str) -> EmbeddingProvider | None:
    """Return a safe provider reconstructed from a persisted DB identity."""
    return OpenAIEmbeddingProvider.from_persisted_name(provider_name)


def _check_available() -> bool:
    """Check whether local embedding support is available."""
    try:
        import sentence_transformers  # noqa: F401

        return True
    except ImportError:
        return False


# ---------------------------------------------------------------------------
# SQLite vector storage
# ---------------------------------------------------------------------------

_EMBEDDINGS_SCHEMA = """
CREATE TABLE IF NOT EXISTS embeddings (
    qualified_name TEXT PRIMARY KEY,
    vector BLOB NOT NULL,
    text_hash TEXT NOT NULL,
    provider TEXT NOT NULL DEFAULT 'unknown'
);
"""


def _encode_vector(vec: list[float]) -> bytes:
    """Encode a float vector as a compact binary blob."""
    return struct.pack(f"{len(vec)}f", *vec)


def _decode_vector(blob: bytes) -> list[float]:
    """Decode a binary blob back to a float vector."""
    n = len(blob) // 4  # 4 bytes per float32
    return list(struct.unpack(f"{n}f", blob))


def _cosine_similarity(a: list[float], b: list[float]) -> float:
    """Compute cosine similarity between two vectors."""
    if len(a) != len(b):
        return 0.0
    dot = sum(x * y for x, y in zip(a, b))
    norm_a = sum(x * x for x in a) ** 0.5
    norm_b = sum(x * x for x in b) ** 0.5
    if norm_a == 0 or norm_b == 0:
        return 0.0
    return dot / (norm_a * norm_b)


# ---------------------------------------------------------------------------
# numpy vector cache (only used when _NUMPY_AVAILABLE is True)
# key: (db_path_str, provider_name, mtime_ns)
# value: (matrix float32 (N, D), names list[str], row_norms float32 (N,))
# ---------------------------------------------------------------------------

_np_vec_cache: dict[tuple[str, str, int], tuple[Any, list[str], Any]] = {}


def _load_vec_matrix(conn: sqlite3.Connection, provider_name: str) -> tuple[Any, list[str], Any]:
    """Load all embedding rows for *provider_name* into a numpy matrix."""
    assert np is not None
    rows = conn.execute(
        "SELECT qualified_name, vector FROM embeddings WHERE provider = ?",
        (provider_name,),
    ).fetchall()
    if not rows:
        empty = np.empty((0, 0), dtype=np.float32)
        return empty, [], np.empty((0,), dtype=np.float32)
    names = [r["qualified_name"] for r in rows]
    vecs = [np.frombuffer(r["vector"], dtype=np.float32) for r in rows]
    matrix = np.stack(vecs)
    row_norms = np.linalg.norm(matrix, axis=1).astype(np.float32)
    return matrix, names, row_norms


def _embedding_text_mode(text_mode: str | None = None) -> str:
    mode = (text_mode or os.environ.get("DAGAYN_EMBEDDING_TEXT_MODE") or "material").lower()
    if mode not in _EMBEDDING_TEXT_MODES:
        raise ValueError(
            "DAGAYN_EMBEDDING_TEXT_MODE must be one of: " + ", ".join(sorted(_EMBEDDING_TEXT_MODES))
        )
    return mode


def _embedding_source_chars() -> int:
    raw = os.environ.get("DAGAYN_EMBEDDING_SOURCE_CHARS")
    if raw is None:
        return _DEFAULT_SOURCE_CHARS
    try:
        return max(0, int(raw))
    except ValueError:
        logger.warning("Invalid DAGAYN_EMBEDDING_SOURCE_CHARS=%r; using default", raw)
        return _DEFAULT_SOURCE_CHARS


def _doc_embedding_body_weight() -> int:
    raw = os.environ.get("DAGAYN_DOC_EMBEDDING_BODY_WEIGHT")
    if raw is None:
        return _DEFAULT_DOC_BODY_WEIGHT
    try:
        return max(1, int(raw))
    except ValueError:
        logger.warning("Invalid DAGAYN_DOC_EMBEDDING_BODY_WEIGHT=%r; using default", raw)
        return _DEFAULT_DOC_BODY_WEIGHT


def _read_node_source_excerpt(
    node: GraphNode,
    *,
    source_root: Path | None = None,
    max_chars: int | None = None,
) -> str:
    """Read a bounded source span for embedding text, best-effort."""
    limit = _embedding_source_chars() if max_chars is None else max(0, max_chars)
    if limit <= 0:
        return ""

    file_path = Path(node.file_path)
    if not file_path.is_absolute():
        if source_root is None:
            return ""
        file_path = source_root / file_path
    try:
        lines = file_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""

    line_start = node.line_start or 1
    line_end = node.line_end or line_start
    start = max(int(line_start) - 1, 0)
    end = min(max(int(line_end), int(line_start)), len(lines))

    if node.kind == "DocSection":
        level = None
        if start < len(lines):
            match = _MARKDOWN_HEADING_RE.match(lines[start])
            if match:
                level = len(match.group(1))
        end = len(lines)
        for idx in range(start + 1, len(lines)):
            match = _MARKDOWN_HEADING_RE.match(lines[idx])
            if match and (level is None or len(match.group(1)) <= level):
                end = idx
                break

    return "\n".join(lines[start:end])[:limit]


def _material_base_text(node: GraphNode) -> str:
    parts = [node.name, node.qualified_name, str(node.file_path).replace("/", " ")]
    display_name = node.extra.get("display_name") if isinstance(node.extra, dict) else None
    if display_name:
        parts.append(str(display_name))
    if node.parent_name:
        parts.append(f"in {node.parent_name}")
    if node.language:
        parts.append(node.language)
    return " ".join(part for part in parts if part)


def _looks_like_comment_line(stripped: str) -> bool:
    return stripped.startswith(("#", "//", "///", "/*", "*", "--", '"""', "'''"))


def _clean_comment_line(stripped: str) -> str:
    cleaned = stripped
    for prefix in ("///", "//", "#", "/*", "*/", "*", "--", '"""', "'''"):
        if cleaned.startswith(prefix):
            cleaned = cleaned[len(prefix) :]
    return cleaned.strip(" */'\"")


def _comment_sentences_for_node(
    node: GraphNode,
    *,
    source_root: Path | None = None,
    max_chars: int | None = None,
) -> list[str]:
    limit = _embedding_source_chars() if max_chars is None else max(0, max_chars)
    if limit <= 0:
        return []

    file_path = Path(node.file_path)
    if not file_path.is_absolute():
        if source_root is None:
            return []
        file_path = source_root / file_path
    try:
        lines = file_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []

    line_start = node.line_start or 1
    line_end = node.line_end or line_start
    start = max(int(line_start) - 1, 0)
    end = min(max(int(line_end), int(line_start)), len(lines))
    comments: list[str] = []

    idx = start - 1
    while idx >= 0:
        stripped = lines[idx].strip()
        if not stripped:
            idx -= 1
            continue
        if _looks_like_comment_line(stripped):
            comments.insert(0, _clean_comment_line(stripped))
            idx -= 1
            continue
        break

    for line in lines[start:end]:
        stripped = line.strip()
        if _looks_like_comment_line(stripped):
            comments.append(_clean_comment_line(stripped))

    text = "\n".join(comment for comment in comments if comment).strip()[:limit]
    if not text:
        return []
    return [part.strip() for part in re.split(r"(?<=[.!?。！？])\s+|\n+", text) if part.strip()]


def _node_to_material_text(
    node: GraphNode,
    *,
    source_root: Path | None = None,
) -> str:
    """Convert a node to the measured default embedding material."""
    base = _material_base_text(node)
    if node.kind in _DOC_BODY_KINDS:
        source_excerpt = _read_node_source_excerpt(node, source_root=source_root)
        return f"{base} {source_excerpt}" if source_excerpt else base

    if node.kind in {"Function", "Method", "Class"}:
        comments = _comment_sentences_for_node(node, source_root=source_root)
        if comments:
            return " ".join([base, *(f"{base} {comment}" for comment in comments)])
        return base

    return base


def _node_to_text(
    node: GraphNode,
    *,
    source_root: Path | None = None,
    text_mode: str | None = None,
) -> str:
    """Convert a node to a searchable text representation."""
    mode = _embedding_text_mode(text_mode)
    if mode == "material":
        return _node_to_material_text(node, source_root=source_root)

    parts = [node.name, node.qualified_name, str(node.file_path).replace("/", " ")]
    display_name = node.extra.get("display_name") if isinstance(node.extra, dict) else None
    if display_name:
        parts.append(str(display_name))
    if node.kind != "File":
        parts.append(node.kind.lower())
    if node.parent_name:
        parts.append(f"in {node.parent_name}")
    if node.signature:
        parts.append(node.signature)
    if node.params:
        parts.append(node.params)
    if node.return_type:
        parts.append(f"returns {node.return_type}")
    if node.language:
        parts.append(node.language)
    include_source = mode == "body" or node.kind in _DOC_BODY_KINDS
    if include_source:
        source_excerpt = _read_node_source_excerpt(node, source_root=source_root)
        if source_excerpt:
            repetitions = _doc_embedding_body_weight() if node.kind in _DOC_BODY_KINDS else 1
            if repetitions > 1:
                per_repetition = max(1, _embedding_source_chars() // repetitions)
                source_excerpt = source_excerpt[:per_repetition]
            parts.extend([source_excerpt] * repetitions)
    return " ".join(parts)


def _slow_embed_batch_seconds() -> float:
    raw = os.environ.get("CRG_EMBEDDING_SLOW_BATCH_SECONDS")
    if raw is None:
        return _DEFAULT_SLOW_EMBED_BATCH_SECONDS
    try:
        return max(0.0, float(raw))
    except ValueError:
        return _DEFAULT_SLOW_EMBED_BATCH_SECONDS


@functools.lru_cache(maxsize=256)
def _embed_query_cached(provider: "EmbeddingProvider", query: str) -> list[float]:
    """Cache embed_query results keyed on (provider, query_text).

    Provider instances are compared by identity; the cache is naturally
    invalidated when a new EmbeddingStore (and therefore new provider
    instance) is created after a DB mtime change.
    """
    return provider.embed_query(query)


class EmbeddingStore:
    """Manages vector embeddings for graph nodes in SQLite."""

    def __init__(
        self,
        db_path: str | Path,
        provider: str | None = None,
        model: str | None = None,
        provider_instance: EmbeddingProvider | None = None,
        text_mode: str | None = None,
        source_root: str | Path | None = None,
    ) -> None:
        self.provider = provider_instance or get_provider(provider, model=model)
        self.available = self.provider is not None
        self.db_path = Path(db_path)
        self.text_mode = _embedding_text_mode(text_mode)
        self.source_root = Path(source_root) if source_root is not None else None
        self._conn = sqlite3.connect(
            str(self.db_path),
            timeout=30,
            check_same_thread=False,
            isolation_level=None,
        )
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._conn.execute("PRAGMA cache_size=-32000")  # 32 MB page cache
        self._conn.execute("PRAGMA mmap_size=134217728")  # 128 MB memory-mapped I/O
        self._conn.execute("PRAGMA temp_store=MEMORY")
        self._conn.executescript(_EMBEDDINGS_SCHEMA)
        self.last_orphans_removed = 0

        # Migration for existing DBs missing the provider column
        try:
            self._conn.execute("SELECT provider FROM embeddings LIMIT 1")
        except sqlite3.OperationalError:
            self._conn.execute(
                "ALTER TABLE embeddings ADD COLUMN provider TEXT NOT NULL DEFAULT 'unknown'"
            )

        self._conn.commit()

    def __enter__(self) -> "EmbeddingStore":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:  # type: ignore[no-untyped-def]
        self.close()

    def close(self) -> None:
        self._conn.close()

    def checkpoint_writes(self, *, truncate: bool = False) -> None:
        """Checkpoint pending WAL pages after embedding writes."""
        mode = "TRUNCATE" if truncate else "PASSIVE"
        try:
            self._conn.execute(f"PRAGMA wal_checkpoint({mode})")
        except sqlite3.Error:
            logger.debug("Could not checkpoint embedding writes", exc_info=True)

    def embed_nodes(
        self,
        nodes: list[GraphNode],
        *,
        show_progress: bool = False,
    ) -> int:
        """Compute and store embeddings for a list of nodes."""
        if not self.provider:
            return 0

        # Filter to nodes that need embedding
        provider_name = self.provider.name
        candidate_nodes = [n for n in nodes if n.kind != "File"]
        if not candidate_nodes:
            return 0

        # Batch-fetch existing hashes in one query instead of N individual SELECTs
        qns = [n.qualified_name for n in candidate_nodes]
        _hash_fetch_batch = 450  # SQLite variable limit is 999
        existing_hashes: dict[str, tuple[str, str]] = {}  # qn -> (text_hash, provider)
        for i in range(0, len(qns), _hash_fetch_batch):
            chunk = qns[i : i + _hash_fetch_batch]
            placeholders = ",".join("?" for _ in chunk)
            rows = self._conn.execute(  # nosec B608
                f"SELECT qualified_name, text_hash, provider FROM embeddings"
                f" WHERE qualified_name IN ({placeholders})",
                chunk,
            ).fetchall()
            for r in rows:
                existing_hashes[r["qualified_name"]] = (r["text_hash"], r["provider"])

        to_embed: list[tuple[GraphNode, str, str]] = []
        for node in candidate_nodes:
            text = _node_to_text(node, source_root=self.source_root, text_mode=self.text_mode)
            text_hash = hashlib.sha256(text.encode()).hexdigest()
            ex = existing_hashes.get(node.qualified_name)
            if ex and ex[0] == text_hash and ex[1] == provider_name:
                continue
            to_embed.append((node, text, text_hash))

        if not to_embed:
            return 0

        # Encode and persist in provider-sized batches. Persisting each batch
        # makes long local embedding runs resumable if a later request stalls.
        api_batch = self.provider.preferred_batch_size
        total = len(to_embed)
        use_progress = show_progress and sys.stderr.isatty()
        start_time = time.monotonic()
        embedded = 0
        slow_batch_seconds = _slow_embed_batch_seconds()

        for i in range(0, total, api_batch):
            batch = to_embed[i : i + api_batch]
            batch_texts = [t for _, t, _ in batch]
            batch_number = (i // api_batch) + 1
            batch_total = (total + api_batch - 1) // api_batch
            batch_started = time.monotonic()
            try:
                vectors = self.provider.embed(batch_texts)
            except Exception as e:
                if len(batch) > 1:
                    embedded += self._embed_nodes_individually_after_batch_failure(
                        batch,
                        provider_name=provider_name,
                        batch_number=batch_number,
                        batch_total=batch_total,
                        original_error=e,
                    )
                    if use_progress:
                        done = min(i + api_batch, total)
                        elapsed = time.monotonic() - start_time
                        _draw_embed_progress(done, total, elapsed, end=(done >= total))
                    continue
                first_qn = batch[0][0].qualified_name if batch else "<empty>"
                raise RuntimeError(
                    "Embedding batch "
                    f"{batch_number}/{batch_total} failed "
                    f"({len(batch_texts)} node(s), first={first_qn!r}): {e}"
                ) from e
            if len(vectors) != len(batch):
                first_qn = batch[0][0].qualified_name if batch else "<empty>"
                raise RuntimeError(
                    "Embedding batch "
                    f"{batch_number}/{batch_total} returned {len(vectors)} vector(s) "
                    f"for {len(batch)} node(s), first={first_qn!r}."
                )
            elapsed_batch = time.monotonic() - batch_started
            if slow_batch_seconds and elapsed_batch >= slow_batch_seconds:
                logger.warning(
                    "Embedding batch %d/%d took %.1fs (%d node(s), first=%r, last=%r)",
                    batch_number,
                    batch_total,
                    elapsed_batch,
                    len(batch),
                    batch[0][0].qualified_name,
                    batch[-1][0].qualified_name,
                )
            self._conn.executemany(
                """INSERT OR REPLACE INTO embeddings (qualified_name, vector, text_hash, provider)
                   VALUES (?, ?, ?, ?)""",
                [
                    (node.qualified_name, _encode_vector(vec), text_hash, provider_name)
                    for (node, _text, text_hash), vec in zip(batch, vectors)
                ],
            )
            self._conn.commit()
            embedded += len(batch)
            if use_progress:
                done = min(i + api_batch, total)
                elapsed = time.monotonic() - start_time
                _draw_embed_progress(done, total, elapsed, end=(done >= total))

        self.checkpoint_writes()

        return embedded

    def _embed_nodes_individually_after_batch_failure(
        self,
        batch: list[tuple[GraphNode, str, str]],
        *,
        provider_name: str,
        batch_number: int,
        batch_total: int,
        original_error: Exception,
    ) -> int:
        """Retry a failed provider batch one node at a time to isolate bad inputs."""
        embedded = 0
        failures: list[tuple[str, str]] = []
        for node, text, text_hash in batch:
            try:
                vectors = self.provider.embed([text]) if self.provider else []
            except Exception as e:
                failures.append((node.qualified_name, str(e)))
                continue
            if len(vectors) != 1:
                failures.append(
                    (
                        node.qualified_name,
                        f"returned {len(vectors)} vector(s) for one node",
                    )
                )
                continue
            self._conn.execute(
                """INSERT OR REPLACE INTO embeddings (qualified_name, vector, text_hash, provider)
                   VALUES (?, ?, ?, ?)""",
                (node.qualified_name, _encode_vector(vectors[0]), text_hash, provider_name),
            )
            self._conn.commit()
            embedded += 1

        if failures:
            sample = "; ".join(f"{qn}: {err}" for qn, err in failures[:5])
            more = "" if len(failures) <= 5 else f"; ... +{len(failures) - 5} more"
            raise RuntimeError(
                "Embedding batch "
                f"{batch_number}/{batch_total} failed as a batch "
                f"({len(batch)} node(s)): {original_error}. "
                f"Isolated {len(failures)} failing node(s): {sample}{more}"
            ) from original_error

        logger.warning(
            "Embedding batch %d/%d failed as a batch but all %d node(s) succeeded "
            "when retried individually: %s",
            batch_number,
            batch_total,
            len(batch),
            original_error,
        )
        return embedded

    def search(self, query: str, limit: int = 20) -> list[tuple[str, float]]:
        """Search for nodes by semantic similarity.

        When numpy is available (``embeddings`` extra), vectors are cached in a
        process-level matrix keyed by (db_path, provider, mtime_ns) and
        similarity is computed via a single BLAS matrix-vector product.
        Falls back to a pure-Python loop when numpy is not installed.
        """
        if not self.provider:
            return []

        provider_name = self.provider.name
        query_vec = _embed_query_cached(self.provider, query)

        if not _NUMPY_AVAILABLE:
            # Pure-Python fallback (no numpy installed)
            scored: list[tuple[str, float]] = []
            cursor = self._conn.execute(
                "SELECT qualified_name, vector FROM embeddings WHERE provider = ?",
                (provider_name,),
            )
            while True:
                rows = cursor.fetchmany(500)
                if not rows:
                    break
                for row in rows:
                    vec = _decode_vector(row["vector"])
                    sim = _cosine_similarity(query_vec, vec)
                    scored.append((row["qualified_name"], sim))
            scored.sort(key=lambda x: x[1], reverse=True)
            return scored[:limit]

        # numpy fast path: process-level matrix cache keyed by mtime
        assert np is not None
        try:
            mtime_ns = int(self.db_path.stat().st_mtime_ns)
        except OSError:
            mtime_ns = 0
        cache_key = (str(self.db_path), provider_name, mtime_ns)
        if cache_key not in _np_vec_cache:
            _np_vec_cache[cache_key] = _load_vec_matrix(self._conn, provider_name)
            # Evict stale entries for the same (path, provider) to bound memory
            for k in list(_np_vec_cache):
                if k != cache_key and k[0] == cache_key[0] and k[1] == cache_key[1]:
                    del _np_vec_cache[k]

        matrix, names, row_norms = _np_vec_cache[cache_key]
        if not names:
            return []

        q = np.array(query_vec, dtype=np.float32)
        q_norm = float(np.linalg.norm(q))
        if q_norm == 0.0:
            return []
        q = q / q_norm

        # Single BLAS call: (N, D) @ (D,) → (N,)
        dots = matrix @ q
        safe_norms = np.where(row_norms > 0, row_norms, 1.0)
        sims = (dots / safe_norms).astype(np.float32)

        n = len(names)
        k = min(limit, n)
        if k == n:
            top_idx = np.argsort(-sims)
        else:
            top_idx = np.argpartition(-sims, k)[:k]
            top_idx = top_idx[np.argsort(-sims[top_idx])]

        return [(names[int(i)], float(sims[i])) for i in top_idx]

    def remove_node(self, qualified_name: str) -> None:
        self._conn.execute("DELETE FROM embeddings WHERE qualified_name = ?", (qualified_name,))
        self._conn.commit()

    def remove_orphans(self, live_qualified_names: set[str]) -> int:
        """Delete embeddings for this provider whose nodes no longer exist."""
        if not self.provider:
            return 0

        provider_name = self.provider.name
        rows = self._conn.execute(
            "SELECT qualified_name FROM embeddings WHERE provider = ?",
            (provider_name,),
        ).fetchall()
        orphan_names = [
            row["qualified_name"]
            for row in rows
            if row["qualified_name"] not in live_qualified_names
        ]
        if not orphan_names:
            return 0

        batch_size = 450
        deleted = 0
        for i in range(0, len(orphan_names), batch_size):
            chunk = orphan_names[i : i + batch_size]
            placeholders = ",".join("?" for _ in chunk)
            cursor = self._conn.execute(  # nosec B608
                f"DELETE FROM embeddings WHERE provider = ? AND qualified_name IN ({placeholders})",
                [provider_name, *chunk],
            )
            deleted += cursor.rowcount if cursor.rowcount is not None else len(chunk)
        self._conn.commit()
        return deleted

    def count(self) -> int:
        return self._conn.execute("SELECT COUNT(*) FROM embeddings").fetchone()[0]

    def count_provider(self) -> int:
        if not self.provider:
            return 0
        return self._conn.execute(
            "SELECT COUNT(*) FROM embeddings WHERE provider = ?",
            (self.provider.name,),
        ).fetchone()[0]


def _draw_embed_progress(done: int, total: int, elapsed: float, *, end: bool = False) -> None:
    """Draw a single-line embedding progress bar to stderr."""
    if total == 0:
        return
    pct = done / total
    width = 20
    filled = int(width * pct)
    bar = "█" * filled + "░" * (width - filled)
    rate = done / elapsed if elapsed > 0 else 0
    if rate > 0 and done < total:
        secs_left = (total - done) / rate
        eta = f"{int(secs_left // 60)}:{int(secs_left % 60):02d}"
    else:
        eta = "--:--"
    line = f"\rEmbedding  [{bar}]  {done}/{total}  {pct:3.0%}  {rate:.1f} nodes/s  ETA {eta}"
    print(line, end="\n" if end else "", flush=True, file=sys.stderr)


def embed_all_nodes(
    graph_store: GraphStore,
    embedding_store: EmbeddingStore,
    *,
    show_progress: bool = False,
) -> int:
    """Embed all non-file nodes in the graph."""
    if not embedding_store.available:
        return 0

    all_nodes = graph_store.get_all_nodes(exclude_files=True)
    embedding_store.last_orphans_removed = embedding_store.remove_orphans(
        {node.qualified_name for node in all_nodes}
    )

    if embedding_store.source_root is None:
        get_repo_root = getattr(graph_store, "get_repo_root", None)
        if callable(get_repo_root):
            embedding_store.source_root = get_repo_root()

    return embedding_store.embed_nodes(all_nodes, show_progress=show_progress)
