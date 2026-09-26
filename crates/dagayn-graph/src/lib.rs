use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Value, json};
use thiserror::Error;

const LATEST_VERSION: i64 = 17;
const MAX_INSERT_PARAMS: usize = 30_000;
const NODE_INSERT_PARAM_COUNT: usize = 16;
const EDGE_INSERT_PARAM_COUNT: usize = 10;
const NODE_INSERT_ROWS: usize = MAX_INSERT_PARAMS / NODE_INSERT_PARAM_COUNT;
const EDGE_INSERT_ROWS: usize = MAX_INSERT_PARAMS / EDGE_INSERT_PARAM_COUNT;
const SUSPEND_INDEX_FILE_THRESHOLD: usize = 64;
/// Indexes dropped for the duration of a bulk load. `idx_nodes_file` and
/// `idx_edges_file` are deliberately absent: every batch deletes and re-reads
/// its files by `file_path`, and without them each batch scans both tables.
const WRITE_INDEXES: &[(&str, &str)] = &[
    (
        "idx_nodes_kind",
        "CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind)",
    ),
    (
        "idx_nodes_parent_name",
        "CREATE INDEX IF NOT EXISTS idx_nodes_parent_name ON nodes(parent_name, name)",
    ),
    (
        "idx_nodes_community",
        "CREATE INDEX IF NOT EXISTS idx_nodes_community ON nodes(community_id)",
    ),
    (
        "idx_edges_kind",
        "CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind)",
    ),
    (
        "idx_edges_target_kind",
        "CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target_qualified, kind)",
    ),
    (
        "idx_edges_source_kind",
        "CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source_qualified, kind)",
    ),
    (
        "idx_edges_target_name_kind",
        "CREATE INDEX IF NOT EXISTS idx_edges_target_name_kind ON edges(target_name, kind)",
    ),
];
const SECURITY_KEYWORDS: &[&str] = &[
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
];

/// Identifier tokens that start with a security keyword but name ordinary,
/// non-security concepts. Mirrors `SECURITY_KEYWORD_EXCLUDED_TOKENS` in
/// `dagayn/constants.py`.
const SECURITY_KEYWORD_EXCLUDED_TOKENS: &[&str] = &[
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
];

/// Split an identifier or qualified name into lowercase word tokens.
///
/// Splits on every non-ASCII-alphanumeric character, on camelCase /
/// PascalCase boundaries, on acronym boundaries (`HTTPServer` -> `http`,
/// `server`), and between letters and digits (`sha256Hash` -> `sha`, `256`,
/// `hash`). Mirrors `identifier_tokens` in `dagayn/changes.py`.
pub(crate) fn identifier_tokens(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    let collect = |from: usize, to: usize| -> String {
        chars[from..to]
            .iter()
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    while i < n {
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            while i < n && chars[i].is_ascii_digit() {
                i += 1;
            }
            tokens.push(collect(start, i));
        } else if c.is_ascii_lowercase() {
            let start = i;
            while i < n && chars[i].is_ascii_lowercase() {
                i += 1;
            }
            tokens.push(collect(start, i));
        } else if c.is_ascii_uppercase() {
            let start = i;
            let mut upper_end = i;
            while upper_end < n && chars[upper_end].is_ascii_uppercase() {
                upper_end += 1;
            }
            if upper_end < n && chars[upper_end].is_ascii_lowercase() {
                // The last capital starts a PascalCase word; any capitals
                // before it form an acronym token.
                let word_start = upper_end - 1;
                if word_start > start {
                    tokens.push(collect(start, word_start));
                }
                let mut end = upper_end;
                while end < n && chars[end].is_ascii_lowercase() {
                    end += 1;
                }
                tokens.push(collect(word_start, end));
                i = end;
            } else {
                tokens.push(collect(start, upper_end));
                i = upper_end;
            }
        } else {
            i += 1;
        }
    }
    tokens
}

/// Return whether any identifier token of `texts` starts with one of
/// `keywords`, skipping `SECURITY_KEYWORD_EXCLUDED_TOKENS` (as a single token
/// or as a token joined with the following one, e.g. `Hash` + `Map`).
///
/// Matching at token starts keeps inflections (`tokens`, `hashed`,
/// `signature`, `authenticate`) while dropping mid-word hits such as `design`
/// or `assign` for `sign`. Mirrors `is_security_sensitive_identifier` in
/// `dagayn/changes.py`.
pub(crate) fn identifier_matches_keywords(texts: &[&str], keywords: &[&str]) -> bool {
    texts.iter().any(|text| {
        let tokens = identifier_tokens(text);
        tokens.iter().enumerate().any(|(index, token)| {
            if SECURITY_KEYWORD_EXCLUDED_TOKENS.contains(&token.as_str()) {
                return false;
            }
            // `HashMap` splits into `hash` + `map`; treat the pair like the
            // single excluded token `hashmap`.
            if let Some(next) = tokens.get(index + 1)
                && SECURITY_KEYWORD_EXCLUDED_TOKENS.contains(&format!("{token}{next}").as_str())
            {
                return false;
            }
            keywords.iter().any(|keyword| token.starts_with(keyword))
        })
    })
}

/// Security-keyword check shared by change risk and flow criticality.
pub(crate) fn is_security_sensitive_identifier(name: &str, qualified_name: &str) -> bool {
    identifier_matches_keywords(&[name, qualified_name], SECURITY_KEYWORDS)
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
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
    mtime_ns INTEGER DEFAULT 0,
    extra TEXT DEFAULT '{}',
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
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

CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(file_path);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_parent_name ON nodes(parent_name, name);
CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind);
CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target_qualified, kind);
CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source_qualified, kind);
CREATE INDEX IF NOT EXISTS idx_edges_file ON edges(file_path);
CREATE INDEX IF NOT EXISTS idx_hub_scores_total_degree ON hub_scores(total_degree DESC);
CREATE INDEX IF NOT EXISTS idx_bridge_scores_betweenness ON bridge_scores(betweenness DESC);
"#;

const CENTRALITY_SCORE_SCHEMA_SQL: &str = r#"
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

CREATE INDEX IF NOT EXISTS idx_hub_scores_total_degree ON hub_scores(total_degree DESC);
CREATE INDEX IF NOT EXISTS idx_bridge_scores_betweenness ON bridge_scores(betweenness DESC);
"#;

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("system clock error")]
    Clock,
    #[error("invalid embedding data: {0}")]
    InvalidEmbedding(String),
}

pub type Result<T> = std::result::Result<T, GraphError>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeInput {
    pub kind: String,
    pub name: String,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub parent_name: Option<String>,
    #[serde(default)]
    pub params: Option<String>,
    #[serde(default)]
    pub return_type: Option<String>,
    #[serde(default)]
    pub modifiers: Option<String>,
    #[serde(default)]
    pub is_test: bool,
    #[serde(default)]
    pub extra: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EdgeInput {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub file_path: String,
    #[serde(default)]
    pub line: i64,
    #[serde(default)]
    pub extra: Value,
}

#[derive(Clone, Debug)]
pub struct GraphNode {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub language: String,
    pub parent_name: Option<String>,
    pub params: Option<String>,
    pub return_type: Option<String>,
    pub is_test: bool,
    pub file_hash: Option<String>,
    pub extra: Value,
    /// Rendered signature persisted by post-processing. `None` when the column
    /// is absent from the projection or has not been computed yet, matching
    /// Python's `GraphStore._row_to_node`.
    pub signature: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub id: i64,
    pub kind: String,
    pub source_qualified: String,
    pub target_qualified: String,
    pub file_path: String,
    pub line: i64,
    pub extra: Value,
    pub confidence: f64,
    pub confidence_tier: ConfidenceTier,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConfidenceTier {
    Exact,
    #[default]
    Extracted,
    High,
    Medium,
    Low,
    Unknown,
}

impl ConfidenceTier {
    pub fn from_raw(value: Option<&str>) -> Self {
        match value.unwrap_or("EXTRACTED").to_ascii_uppercase().as_str() {
            "EXACT" => Self::Exact,
            "EXTRACTED" => Self::Extracted,
            "HIGH" => Self::High,
            "MEDIUM" => Self::Medium,
            "LOW" => Self::Low,
            "UNKNOWN" => Self::Unknown,
            _ => Self::Extracted,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "EXACT",
            Self::Extracted => "EXTRACTED",
            Self::High => "HIGH",
            Self::Medium => "MEDIUM",
            Self::Low => "LOW",
            Self::Unknown => "UNKNOWN",
        }
    }
}

pub type EdgeEndpointMap = HashMap<String, Vec<GraphEdge>>;
type ChangedRanges = HashMap<String, Vec<(i64, i64)>>;

struct ChangeRiskInputs<'a> {
    node: &'a GraphNode,
    inbound_edges: &'a [GraphEdge],
    flow_criticalities: &'a [f64],
    flow_count: i64,
    node_community_id: Option<i64>,
    caller_community_ids: &'a HashMap<String, Option<i64>>,
    transitive_test_count: i64,
}
#[derive(Clone, Debug)]
pub struct GraphStats {
    pub total_nodes: i64,
    pub total_edges: i64,
    pub nodes_by_kind: HashMap<String, i64>,
    pub edges_by_kind: HashMap<String, i64>,
    pub languages: Box<[String]>,
    pub files_count: i64,
    pub last_updated: Option<String>,
}

fn default_flow_kind() -> String {
    "reachable_set".to_string()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FlowInput {
    pub name: String,
    pub entry_point_id: i64,
    pub depth: i64,
    pub node_count: i64,
    pub file_count: i64,
    pub criticality: f64,
    #[serde(default)]
    pub path: Box<[i64]>,
    #[serde(default = "default_flow_kind")]
    pub kind: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub truncation_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommunityInput {
    pub name: String,
    #[serde(default)]
    pub level: i64,
    #[serde(default)]
    pub cohesion: f64,
    pub size: i64,
    #[serde(default)]
    pub dominant_language: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub members: Box<[String]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlowSortBy {
    Criticality,
    Depth,
    NodeCount,
    FileCount,
    Name,
}

impl FlowSortBy {
    pub fn from_raw(value: &str) -> Self {
        match value {
            "depth" => Self::Depth,
            "node_count" => Self::NodeCount,
            "file_count" => Self::FileCount,
            "name" => Self::Name,
            _ => Self::Criticality,
        }
    }

    pub fn column(self) -> &'static str {
        match self {
            Self::Criticality => "criticality",
            Self::Depth => "depth",
            Self::NodeCount => "node_count",
            Self::FileCount => "file_count",
            Self::Name => "name",
        }
    }

    pub fn order(self) -> &'static str {
        match self {
            Self::Name => "ASC",
            _ => "DESC",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommunitySortBy {
    Size,
    Cohesion,
    Name,
}

impl CommunitySortBy {
    pub fn from_raw(value: &str) -> Self {
        match value {
            "cohesion" => Self::Cohesion,
            "name" => Self::Name,
            _ => Self::Size,
        }
    }

    pub fn column(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Cohesion => "cohesion",
            Self::Name => "name",
        }
    }

    pub fn order(self) -> &'static str {
        match self {
            Self::Name => "ASC",
            _ => "DESC",
        }
    }
}

pub struct GraphStore {
    conn: Connection,
    bulk_load_indexes_suspended: bool,
}

pub type FileBatchItem = (String, Vec<NodeInput>, Vec<EdgeInput>, String, i64);
pub type FlowEdgeData = (HashMap<String, Vec<String>>, HashSet<String>);

#[derive(Debug, Deserialize)]
struct RawCompactNodeInput(
    String,
    String,
    String,
    i64,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    Box<RawValue>,
);

#[derive(Debug, Deserialize)]
struct RawCompactEdgeInput(String, String, String, String, i64, Box<RawValue>);

type RawCompactFileBatchItem = (
    String,
    Vec<RawCompactNodeInput>,
    Vec<RawCompactEdgeInput>,
    String,
    i64,
);

mod analysis;
mod analysis_question_rows;
mod analysis_questions;
mod analysis_stats;
mod bridges;
mod communities;
mod core;
mod edge_lookups;
mod edge_queries;
mod embeddings;
mod flow_trace;
mod flows;
mod fts_sync;
mod helpers;
mod impact;
mod impact_flows;
mod impact_radius;
mod impact_support;
mod japanese_fts;
mod maintenance;
mod node_queries;
mod postprocess_bare_names;
mod postprocess_bridges;
mod postprocess_endpoints;
mod postprocess_manifest;
mod postprocess_terraform;
mod postprocess_tested_by;
mod query;
mod relationship_edges;
mod relationship_traversal;
mod relationships;
mod schema;
mod schema_migrations;
mod search;
mod search_markdown;
mod search_query;
mod subgraph;
mod summaries;
mod summary_communities;
mod summary_flows;
mod summary_risk;
mod upserts;
mod write;

pub use embeddings::{embedding_search, embedding_search_prewarm};
pub use impact_radius::ImpactRadius;
pub use maintenance::ORPHAN_PRUNE_STEPS;
pub use node_queries::NodeSignatureRow;
pub use search_query::FtsQueryResult;
pub use subgraph::LocalSubgraph;

#[cfg(test)]
use helpers::*;

#[cfg(test)]
mod tests;
