"""Embedding provider interface and implementations."""

from __future__ import annotations

import logging
import os
import re
import sys
import time
from abc import ABC, abstractmethod
from typing import Any
from urllib.parse import urlparse

logger = logging.getLogger(__name__)


def _parse_openai_identity_suffixes(tail: str) -> tuple[str, int | None, int | None]:
    """Parse optional ``#max_length=`` and ``#dim=`` suffixes from an identity tail."""
    max_length: int | None = None
    dimension: int | None = None
    while True:
        if "#dim=" in tail:
            tail, _, raw = tail.rpartition("#dim=")
            if not tail:
                return "", None, None
            try:
                dimension = int(raw)
            except ValueError:
                return "", None, None
            continue
        if "#max_length=" in tail:
            tail, _, raw = tail.rpartition("#max_length=")
            if not tail:
                return "", None, None
            try:
                max_length = int(raw)
            except ValueError:
                return "", None, None
            continue
        break
    return tail, max_length, dimension


def _format_openai_identity_suffixes(*, max_length: int | None, dimension: int | None) -> str:
    parts: list[str] = []
    if max_length is not None:
        parts.append(f"#max_length={max_length}")
    if dimension is not None:
        parts.append(f"#dim={dimension}")
    return "".join(parts)


def _openai_provider_names_match(persisted: str, computed: str) -> bool:
    """Return True when *computed* matches *persisted*, including legacy names.

    Comparison is case-insensitive: a model ID spelled ``Qwen`` in one run and
    ``qwen`` in the next is the same model, and treating them as two identities
    silently re-embedded the whole corpus into a second partition. The persisted
    string keeps its original case so existing rows stay addressable.
    """
    if persisted.casefold() == computed.casefold():
        return True
    if "#dim=" in persisted:
        return False
    suffix = computed[len(persisted) :]
    if not suffix.startswith("#dim="):
        return False
    try:
        int(suffix[5:])
    except ValueError:
        return False
    return True


_PROVIDER_DIM_SUFFIX_RE = re.compile(r"#dim=\d+")


def strip_provider_dimension_suffix(provider_name: str) -> str:
    """Return *provider_name* without any ``#dim=`` suffix segments."""
    return _PROVIDER_DIM_SUFFIX_RE.sub("", provider_name)


def embedding_provider_lookup_candidates(
    provider_key: str | None,
    provider_name: str,
) -> list[str]:
    """Return persisted provider keys to probe, newest identity first."""
    candidates: list[str] = []
    for key in (provider_key, provider_name):
        if not key:
            continue
        for variant in (key, strip_provider_dimension_suffix(key)):
            if variant and variant not in candidates:
                candidates.append(variant)
    return candidates


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
    def _call_with_retry(fn, max_retries: int = 3) -> Any:
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
        # Surrounding whitespace in CRG_OPENAI_MODEL is always accidental, and
        # it would otherwise partition the embeddings table.
        self._model = model.strip()
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
        base_url, max_length, dimension = _parse_openai_identity_suffixes(base_url)
        if not model or not base_url or not _is_localhost_url(base_url):
            return None
        provider = cls(
            api_key=api_key,
            base_url=base_url,
            model=model,
            dimension=dimension,
            max_length=max_length,
        )
        if not _openai_provider_names_match(provider_name, provider.name):
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
        suffix = _format_openai_identity_suffixes(
            max_length=self._max_length,
            dimension=self._dimension,
        )
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
        "    To stay fully offline, use dagayn's managed local embedding "
        "sidecar (`--local-embedding`) or another localhost "
        "OpenAI-compatible endpoint.\n",
        file=sys.stderr,
    )


def get_provider(
    provider: str | None = None,
    model: str | None = None,
) -> EmbeddingProvider | None:
    """Get an embedding provider by name.

    Args:
        provider: Provider name. One of "google", "minimax", "openai", or
                  None. ``None`` auto-selects OpenAI-compatible embeddings only
                  when CRG_OPENAI_API_KEY, CRG_OPENAI_BASE_URL, and
                  CRG_OPENAI_MODEL are all configured.
                  Google requires GOOGLE_API_KEY env var and explicit opt-in.
                  MiniMax requires MINIMAX_API_KEY env var and explicit opt-in.
                  OpenAI requires CRG_OPENAI_API_KEY + CRG_OPENAI_BASE_URL +
                  CRG_OPENAI_MODEL env vars (or the ``model`` arg). The egress
                  warning is skipped when the base URL points to localhost.
                  Cloud providers emit a one-time stderr warning before use
                  unless ``CRG_ACCEPT_CLOUD_EMBEDDINGS=1`` is set. See: #174
        model: Model name/path to use. For Google provider this is a Gemini model ID.
               For OpenAI provider this overrides CRG_OPENAI_MODEL.
    """
    normalized_provider = provider.strip().lower() if provider else None
    if normalized_provider == "local":
        logger.warning(
            "provider='local' sentence-transformers embeddings were removed; "
            "use --local-embedding for the managed llama-server sidecar or "
            "provider='openai' with a localhost OpenAI-compatible endpoint."
        )
        return None

    if normalized_provider is None:
        if all(
            os.environ.get(name)
            for name in ("CRG_OPENAI_API_KEY", "CRG_OPENAI_BASE_URL", "CRG_OPENAI_MODEL")
        ):
            normalized_provider = "openai"
        else:
            return None

    if normalized_provider == "openai":
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

    if normalized_provider == "minimax":
        api_key = os.environ.get("MINIMAX_API_KEY")
        if not api_key:
            raise ValueError(
                "MINIMAX_API_KEY environment variable is required for "
                "the MiniMax embedding provider."
            )
        _warn_cloud_egress("minimax")
        return MiniMaxEmbeddingProvider(api_key=api_key)

    if normalized_provider == "google":
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

    raise ValueError(
        f"Unknown embedding provider '{normalized_provider}'. "
        "Expected one of: openai, google, minimax."
    )


def _embedding_provider_key(provider_name: str, text_mode: str) -> str:
    """Partition persisted vectors by provider and embedding text material."""
    if "#text=" in provider_name:
        return provider_name
    return f"{provider_name}#text={text_mode}"


def embedding_provider_base_name(provider_key: str) -> str:
    """Return provider identity without dagayn's text-mode storage suffix."""
    return provider_key.split("#text=", 1)[0]


def embedding_provider_text_mode(provider_key: str) -> str | None:
    """Return the text mode encoded in a persisted provider key, if present."""
    if "#text=" not in provider_key:
        return None
    return provider_key.rsplit("#text=", 1)[1] or None


def provider_from_persisted_name(provider_name: str) -> EmbeddingProvider | None:
    """Return a safe provider reconstructed from a persisted DB identity."""
    base_name = embedding_provider_base_name(provider_name)
    return OpenAIEmbeddingProvider.from_persisted_name(base_name)
