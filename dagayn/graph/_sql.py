"""SQL schema and small storage helpers for the graph store."""

from __future__ import annotations

import os

MAX_IMPACT_NODES = int(os.environ.get("CRG_MAX_IMPACT_NODES", "500"))
MAX_IMPACT_DEPTH = int(os.environ.get("CRG_MAX_IMPACT_DEPTH", "2"))
BFS_ENGINE = os.environ.get("CRG_BFS_ENGINE", "sql")


def _edge_target_name(target_qualified: str) -> str:
    """Return the normalized target name used for indexed bare-name lookup."""
    return target_qualified.rsplit("::", 1)[-1]


# ---------------------------------------------------------------------------
# Schema
# ---------------------------------------------------------------------------

_SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,          -- File, Class, Function, Type, Test
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL UNIQUE,
    file_path TEXT NOT NULL,
    line_start INTEGER,
    line_end INTEGER,
    language TEXT,
    parent_name TEXT,
    params TEXT,
    return_type TEXT,
    modifiers TEXT,
    is_test INTEGER DEFAULT 0,
    file_hash TEXT,
    extra TEXT DEFAULT '{}',
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,           -- CALLS, IMPORTS_FROM, INHERITS, REFERENCES, CROSS_ARTIFACT, etc.
    source_qualified TEXT NOT NULL,
    target_qualified TEXT NOT NULL,
    target_name TEXT NOT NULL DEFAULT '',
    file_path TEXT NOT NULL,
    line INTEGER DEFAULT 0,
    extra TEXT DEFAULT '{}',
    confidence REAL DEFAULT 1.0,
    confidence_tier TEXT DEFAULT 'EXTRACTED',
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS hub_scores (
    qualified_name TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    file_path TEXT NOT NULL,
    in_degree INTEGER NOT NULL,
    out_degree INTEGER NOT NULL,
    total_degree INTEGER NOT NULL,
    community_id INTEGER,
    computed_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS bridge_scores (
    qualified_name TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    file_path TEXT NOT NULL,
    betweenness REAL NOT NULL,
    community_id INTEGER,
    computed_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS hub_scores_code (
    qualified_name TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    file_path TEXT NOT NULL,
    in_degree INTEGER NOT NULL,
    out_degree INTEGER NOT NULL,
    total_degree INTEGER NOT NULL,
    community_id INTEGER,
    computed_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS bridge_scores_code (
    qualified_name TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    file_path TEXT NOT NULL,
    betweenness REAL NOT NULL,
    community_id INTEGER,
    computed_at REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(file_path);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_qualified ON nodes(qualified_name);
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_qualified);
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_qualified);
CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind);
CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target_qualified, kind);
CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source_qualified, kind);
CREATE INDEX IF NOT EXISTS idx_edges_file ON edges(file_path);
CREATE INDEX IF NOT EXISTS idx_hub_scores_total_degree ON hub_scores(total_degree DESC);
CREATE INDEX IF NOT EXISTS idx_bridge_scores_betweenness ON bridge_scores(betweenness DESC);
CREATE INDEX IF NOT EXISTS idx_hub_scores_code_total_degree ON hub_scores_code(total_degree DESC);
CREATE INDEX IF NOT EXISTS idx_bridge_scores_code_betweenness
    ON bridge_scores_code(betweenness DESC);
"""
