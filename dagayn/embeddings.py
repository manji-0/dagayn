"""Vector embedding support for semantic code search.

Supports multiple providers:
1. OpenAI-compatible - Any endpoint speaking OpenAI /v1/embeddings (local
   llama-server sidecars, real OpenAI, Azure OpenAI, self-hosted gateways like
   new-api / LiteLLM / vLLM / LocalAI / Ollama).
2. Google Gemini - High-quality, cloud-based. Requires explicit opt-in.
3. MiniMax (embo-01) - High-quality 1536-dim cloud embeddings. Requires MINIMAX_API_KEY.
"""

from __future__ import annotations

from .embeddings_providers import (
    CLOUD_PROVIDERS,
    EmbeddingProvider,
    GoogleEmbeddingProvider,
    MiniMaxEmbeddingProvider,
    OpenAIEmbeddingProvider,
    _is_localhost_url,
    embedding_provider_base_name,
    embedding_provider_text_mode,
    get_provider,
    provider_from_persisted_name,
)
from .embeddings_store import (
    EmbeddingStore,
    _cosine_similarity,
    _decode_vector,
    _encode_vector,
    _native_embedding_search,
    _native_embedding_search_prewarm,
    embed_all_nodes,
    get_embedding_status,
)
from .embeddings_text import _node_to_text

__all__ = [
    "CLOUD_PROVIDERS",
    "EmbeddingProvider",
    "EmbeddingStore",
    "GoogleEmbeddingProvider",
    "MiniMaxEmbeddingProvider",
    "OpenAIEmbeddingProvider",
    "_cosine_similarity",
    "_decode_vector",
    "_encode_vector",
    "_is_localhost_url",
    "_native_embedding_search",
    "_native_embedding_search_prewarm",
    "_node_to_text",
    "embed_all_nodes",
    "embedding_provider_base_name",
    "embedding_provider_text_mode",
    "get_embedding_status",
    "get_provider",
    "provider_from_persisted_name",
]
