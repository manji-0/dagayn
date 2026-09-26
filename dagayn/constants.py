"""Shared constants for dagayn."""

from __future__ import annotations

import os

SECURITY_KEYWORDS: frozenset[str] = frozenset(
    {
        "auth",
        "login",
        "password",
        "token",
        "session",
        "crypt",
        "secret",
        "credential",
        "permission",
        "sql",
        "query",
        "execute",
        "connect",
        "socket",
        "request",
        "http",
        "sanitize",
        "validate",
        "encrypt",
        "decrypt",
        "hash",
        "sign",
        "verify",
        "admin",
        "privilege",
    }
)

# Identifier tokens that start with a security keyword but name ordinary,
# non-security concepts.  Keywords match on identifier-token prefixes (see
# ``dagayn.changes.is_security_sensitive_identifier``), so without this list
# ``hashmap`` would match ``hash``, ``signal`` would match ``sign``, and
# ``author`` would match ``auth``.  Mirrored in ``crates/dagayn-graph/src/lib.rs``.
SECURITY_KEYWORD_EXCLUDED_TOKENS: frozenset[str] = frozenset(
    {
        "hashmap",
        "hashmaps",
        "hashset",
        "hashsets",
        "hashtable",
        "hashtables",
        "signal",
        "signals",
        "signaled",
        "signaling",
        "signalled",
        "signalling",
        "significant",
        "significance",
        "significantly",
        "signify",
        "author",
        "authors",
        "authored",
        "authoring",
        "authorship",
    }
)

# ---------------------------------------------------------------------------
# Configurable limits (override via environment variables)
# ---------------------------------------------------------------------------
MAX_IMPACT_NODES = int(os.environ.get("CRG_MAX_IMPACT_NODES", "500"))
MAX_IMPACT_DEPTH = int(os.environ.get("CRG_MAX_IMPACT_DEPTH", "2"))
MAX_BFS_DEPTH = int(os.environ.get("CRG_MAX_BFS_DEPTH", "15"))
MAX_SEARCH_RESULTS = int(os.environ.get("CRG_MAX_SEARCH_RESULTS", "20"))

# BFS engine: "sql" (SQLite recursive CTE) or "networkx" (Python-side BFS)
BFS_ENGINE = os.environ.get("CRG_BFS_ENGINE", "sql")
