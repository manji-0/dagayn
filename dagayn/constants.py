"""Shared constants for dagayn."""

from __future__ import annotations

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

# BFS engine: "sql" (SQLite recursive CTE) or "networkx" (Python-side BFS)
