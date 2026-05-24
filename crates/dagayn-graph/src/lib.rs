use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{json, Value};
use thiserror::Error;

const LATEST_VERSION: i64 = 14;
const MAX_INSERT_PARAMS: usize = 30_000;
const NODE_INSERT_PARAM_COUNT: usize = 16;
const EDGE_INSERT_PARAM_COUNT: usize = 9;
const NODE_INSERT_ROWS: usize = MAX_INSERT_PARAMS / NODE_INSERT_PARAM_COUNT;
const EDGE_INSERT_ROWS: usize = MAX_INSERT_PARAMS / EDGE_INSERT_PARAM_COUNT;
const SUSPEND_INDEX_FILE_THRESHOLD: usize = 64;
const WRITE_INDEXES: &[(&str, &str)] = &[
    ("idx_nodes_file", "CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(file_path)"),
    ("idx_nodes_kind", "CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind)"),
    (
        "idx_nodes_qualified",
        "CREATE INDEX IF NOT EXISTS idx_nodes_qualified ON nodes(qualified_name)",
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
        "idx_edges_source",
        "CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_qualified)",
    ),
    (
        "idx_edges_target",
        "CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_qualified)",
    ),
    ("idx_edges_kind", "CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind)"),
    (
        "idx_edges_target_kind",
        "CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target_qualified, kind)",
    ),
    (
        "idx_edges_source_kind",
        "CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source_qualified, kind)",
    ),
    ("idx_edges_file", "CREATE INDEX IF NOT EXISTS idx_edges_file ON edges(file_path)"),
    (
        "idx_edges_composite",
        "CREATE INDEX IF NOT EXISTS idx_edges_composite ON edges(kind, source_qualified, target_qualified, file_path, line)",
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
CREATE INDEX IF NOT EXISTS idx_nodes_qualified ON nodes(qualified_name);
CREATE INDEX IF NOT EXISTS idx_nodes_parent_name ON nodes(parent_name, name);
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_qualified);
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_qualified);
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
    pub confidence_tier: String,
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
    pub languages: Vec<String>,
    pub files_count: i64,
    pub last_updated: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FlowInput {
    pub name: String,
    pub entry_point_id: i64,
    pub depth: i64,
    pub node_count: i64,
    pub file_count: i64,
    pub criticality: f64,
    #[serde(default)]
    pub path: Vec<i64>,
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
    pub members: Vec<String>,
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

impl GraphStore {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref();
        if db_path != Path::new(":memory:") {
            if let Some(parent) = db_path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .map_err(|_| rusqlite::Error::InvalidPath(parent.to_path_buf()))?;
                }
            }
        }
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "cache_size", -64000)?;
        conn.pragma_update(None, "mmap_size", 268435456)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        let store = Self {
            conn,
            bulk_load_indexes_suspended: false,
        };
        store.init_schema()?;
        if store.schema_version()? < 1 {
            store.set_metadata("schema_version", "1")?;
        }
        store.run_migrations()?;
        Ok(store)
    }

    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES (?, ?)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_metadata(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM metadata WHERE key = ?", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn schema_version(&self) -> Result<i64> {
        let version = match self.get_metadata("schema_version")? {
            Some(raw) => raw
                .parse::<i64>()
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            None => 1,
        };
        Ok(version)
    }

    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT").or_else(|err| {
            if matches!(err, rusqlite::Error::SqliteFailure(_, Some(ref message)) if message.contains("no transaction is active"))
            {
                Ok(())
            } else {
                Err(err)
            }
        })?;
        Ok(())
    }

    pub fn rollback(&self) -> Result<()> {
        self.conn.execute_batch("ROLLBACK").or_else(|err| {
            if matches!(err, rusqlite::Error::SqliteFailure(_, Some(ref message)) if message.contains("no transaction is active"))
            {
                Ok(())
            } else {
                Err(err)
            }
        })?;
        Ok(())
    }

    pub fn remove_file_data(&mut self, file_path: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        remove_file_data_tx(&tx, file_path)?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_files_data(&mut self, file_paths: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        remove_files_data_tx(&tx, file_paths)?;
        tx.commit()?;
        Ok(())
    }

    pub fn rebuild_fts_index(&mut self) -> Result<i64> {
        let repo_root = self.get_metadata("repo_root")?.map(PathBuf::from);
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            r#"
            DROP TABLE IF EXISTS nodes_fts;
            CREATE VIRTUAL TABLE nodes_fts USING fts5(
                name, qualified_name, file_path, signature, identifier_tokens, doc_text,
                tokenize='porter unicode61'
            );
            "#,
        )?;
        let fts_rows = {
            let mut stmt = tx.prepare(
                "SELECT rowid AS node_rowid, kind, name, qualified_name, file_path, line_start, line_end, \
                 signature, extra FROM nodes",
            )?;
            let mapped = stmt.query_map([], |row| {
                let rowid: i64 = row.get("node_rowid")?;
                let kind: String = row.get("kind")?;
                let name: String = row.get("name")?;
                let qualified_name: String = row.get("qualified_name")?;
                let file_path: String = row.get("file_path")?;
                let line_start: Option<i64> = row.get("line_start")?;
                let line_end: Option<i64> = row.get("line_end")?;
                let signature: Option<String> = row.get("signature")?;
                let extra_raw: Option<String> = row.get("extra")?;
                Ok((
                    rowid,
                    kind,
                    name,
                    qualified_name,
                    file_path,
                    line_start,
                    line_end,
                    signature,
                    extra_raw,
                ))
            })?;
            let mut collected = Vec::new();
            for row in mapped {
                collected.push(row?);
            }
            collected
        };
        {
            let mut insert = tx.prepare(
                "INSERT INTO nodes_fts(rowid, name, qualified_name, file_path, signature, \
                 identifier_tokens, doc_text) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            for (
                rowid,
                kind,
                name,
                qualified_name,
                file_path,
                line_start,
                line_end,
                signature,
                extra_raw,
            ) in fts_rows
            {
                let extra = parse_json_column(extra_raw)?;
                let display_name = extra
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let identifier_tokens =
                    identifier_search_text([&name, &qualified_name, &file_path, display_name]);
                let source_excerpt = read_node_source_excerpt(
                    repo_root.as_deref(),
                    &kind,
                    &file_path,
                    line_start,
                    line_end,
                );
                let doc_text = [display_name, source_excerpt.as_str()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                insert.execute(params![
                    rowid,
                    name,
                    qualified_name,
                    file_path,
                    signature.unwrap_or_default(),
                    identifier_tokens,
                    doc_text
                ])?;
            }
        }
        let count = tx.query_row("SELECT count(*) FROM nodes_fts", [], |row| row.get(0))?;
        tx.commit()?;
        Ok(count)
    }

    pub fn compute_missing_signatures(&mut self) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE nodes \
             SET signature = CASE \
               WHEN kind IN ('Function', 'Test') THEN \
                 substr('def ' || name || '(' || COALESCE(params, '') || ')' || \
                   CASE WHEN return_type IS NOT NULL THEN ' -> ' || return_type ELSE '' END, 1, 512) \
               WHEN kind = 'Class' THEN substr('class ' || name, 1, 512) \
               ELSE substr(name, 1, 512) \
             END \
             WHERE signature IS NULL",
            [],
        )?;
        let count = tx.query_row("SELECT changes()", [], |row| row.get::<_, i64>(0))?;
        tx.commit()?;
        Ok(count)
    }

    pub fn resolve_markdown_artifact_refs(&mut self) -> Result<(i64, i64, i64, i64)> {
        let tx = self.conn.transaction()?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT id, target_qualified, extra FROM edges \
                 WHERE kind='CROSS_ARTIFACT' \
                   AND (extra LIKE '%original_symbol_name%' \
                        OR extra LIKE '%unresolved_target_name%')",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        let mut resolved = 0_i64;
        let mut demoted = 0_i64;
        let mut re_resolved = 0_i64;
        let mut still_unresolved = 0_i64;
        let mut edge_data = Vec::new();
        let mut symbols = HashSet::new();
        for (edge_id, current_target, raw_extra) in rows {
            let Ok(mut extra) = serde_json::from_str::<Value>(&raw_extra) else {
                continue;
            };
            let Some(extra_obj) = extra.as_object_mut() else {
                continue;
            };
            let sym = extra_obj
                .get("original_symbol_name")
                .or_else(|| extra_obj.get("unresolved_target_name"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let Some(sym) = sym else { continue };
            extra_obj.remove("unresolved_target_name");
            extra_obj.insert(
                "original_symbol_name".to_string(),
                Value::String(sym.clone()),
            );
            symbols.insert(sym.clone());
            edge_data.push((edge_id, current_target, raw_extra, sym, extra));
        }

        let mut matches_by_symbol = HashMap::<String, Vec<(String, Option<String>)>>::new();
        let symbols = symbols.into_iter().collect::<Vec<_>>();
        for chunk in symbols.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT name, qualified_name, language \
                 FROM nodes \
                 WHERE name IN ({placeholders}) AND language != 'markdown'"
            );
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            for row in rows {
                let (name, qualified_name, language) = row?;
                matches_by_symbol
                    .entry(name)
                    .or_default()
                    .push((qualified_name, language));
            }
        }

        for (edge_id, current_target, raw_extra, sym, mut extra) in edge_data {
            let matches = matches_by_symbol
                .get(&sym)
                .map(Vec::as_slice)
                .unwrap_or(&[]);

            if matches.len() == 1 {
                let (target, language) = &matches[0];
                let Some(extra_obj) = extra.as_object_mut() else {
                    continue;
                };
                extra_obj.insert(
                    "target_language".to_string(),
                    Value::String(language.clone().unwrap_or_else(|| "unknown".to_string())),
                );
                extra_obj.insert("confidence".to_string(), Value::from(0.8));
                extra_obj.insert(
                    "confidence_tier".to_string(),
                    Value::String("HIGH".to_string()),
                );
                if current_target == *target && !raw_extra.contains("unresolved_target_name") {
                    continue;
                }
                tx.execute(
                    "UPDATE edges \
                     SET target_qualified = ?, extra = ?, confidence = 0.8, confidence_tier = 'HIGH' \
                     WHERE id = ?",
                    params![target, serde_json::to_string(&extra)?, edge_id],
                )?;
                if current_target.starts_with("<unresolved:") {
                    resolved += 1;
                } else if current_target != *target {
                    re_resolved += 1;
                }
            } else {
                let unresolved_target = format!("<unresolved:{sym}>");
                if current_target == unresolved_target
                    && !raw_extra.contains("unresolved_target_name")
                {
                    still_unresolved += 1;
                    continue;
                }
                let Some(extra_obj) = extra.as_object_mut() else {
                    continue;
                };
                extra_obj.remove("target_language");
                extra_obj.insert("confidence".to_string(), Value::from(0.2));
                extra_obj.insert(
                    "confidence_tier".to_string(),
                    Value::String("LOW".to_string()),
                );
                tx.execute(
                    "UPDATE edges \
                     SET target_qualified = ?, extra = ?, confidence = 0.2, confidence_tier = 'LOW' \
                     WHERE id = ?",
                    params![unresolved_target, serde_json::to_string(&extra)?, edge_id],
                )?;
                demoted += 1;
            }
        }

        tx.commit()?;
        Ok((resolved, demoted, re_resolved, still_unresolved))
    }

    pub fn compute_summaries(&mut self) -> Result<()> {
        match self.compute_community_summaries() {
            Ok(()) | Err(GraphError::Sqlite(_)) => {}
            Err(err) => return Err(err),
        }
        match self.compute_flow_snapshots() {
            Ok(()) | Err(GraphError::Sqlite(_)) => {}
            Err(err) => return Err(err),
        }
        match self.compute_risk_index() {
            Ok(()) | Err(GraphError::Sqlite(_)) => {}
            Err(err) => return Err(err),
        }
        Ok(())
    }

    fn compute_community_summaries(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM community_summaries", [])?;

        let mut edge_counts: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT source_qualified, COUNT(*) FROM edges GROUP BY source_qualified",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (qualified, count) = row?;
                *edge_counts.entry(qualified).or_default() += count;
            }
        }
        {
            let mut stmt = tx.prepare(
                "SELECT target_qualified, COUNT(*) FROM edges GROUP BY target_qualified",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (qualified, count) = row?;
                *edge_counts.entry(qualified).or_default() += count;
            }
        }

        let mut nodes_by_comm: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT community_id, name, qualified_name FROM nodes \
                 WHERE community_id IS NOT NULL AND kind != 'File'",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (community_id, name, qualified_name) = row?;
                nodes_by_comm
                    .entry(community_id)
                    .or_default()
                    .push((name, *edge_counts.get(&qualified_name).unwrap_or(&0)));
            }
        }

        let mut files_by_comm: HashMap<i64, Vec<String>> = HashMap::new();
        let mut seen_files: HashMap<i64, HashSet<String>> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT community_id, file_path FROM nodes WHERE community_id IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (community_id, file_path) = row?;
                let seen = seen_files.entry(community_id).or_default();
                if seen.insert(file_path.clone()) {
                    files_by_comm
                        .entry(community_id)
                        .or_default()
                        .push(file_path);
                }
            }
        }

        let community_rows = {
            let mut stmt =
                tx.prepare("SELECT id, name, size, dominant_language FROM communities")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO community_summaries \
             (community_id, name, purpose, key_symbols, size, dominant_language) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )?;
        for (community_id, name, size, dominant_language) in community_rows {
            let mut members = nodes_by_comm.remove(&community_id).unwrap_or_default();
            members.sort_by(|left, right| right.1.cmp(&left.1));
            let key_symbols = members
                .into_iter()
                .take(5)
                .map(|(name, _)| name)
                .collect::<Vec<_>>();
            let paths = files_by_comm
                .get(&community_id)
                .map(|paths| paths.iter().take(20).cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let purpose = community_purpose(&paths);
            insert.execute(params![
                community_id,
                name,
                purpose,
                serde_json::to_string(&key_symbols)?,
                size,
                dominant_language.unwrap_or_default()
            ])?;
        }
        drop(insert);
        tx.commit()?;
        Ok(())
    }

    fn compute_flow_snapshots(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM flow_snapshots", [])?;

        let flow_rows = {
            let mut stmt = tx.prepare(
                "SELECT id, name, entry_point_id, criticality, node_count, \
                 file_count, path_json FROM flows",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut needed_ids: HashSet<i64> = HashSet::new();
        let mut parsed_paths = Vec::with_capacity(flow_rows.len());
        for (_, _, entry_point_id, _, _, _, path_json) in &flow_rows {
            needed_ids.insert(*entry_point_id);
            let path_ids = if path_json.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str::<Vec<i64>>(path_json)?
            };
            for node_id in path_ids.iter().skip(1).take(3) {
                needed_ids.insert(*node_id);
            }
            if let Some(last) = path_ids.last() {
                needed_ids.insert(*last);
            }
            parsed_paths.push(path_ids);
        }

        let mut id_to_name: HashMap<i64, String> = HashMap::new();
        let id_list = needed_ids.into_iter().collect::<Vec<_>>();
        for chunk in id_list.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id, qualified_name FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (node_id, qualified_name) = row?;
                id_to_name.insert(node_id, qualified_name);
            }
        }

        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO flow_snapshots \
             (flow_id, name, entry_point, critical_path, criticality, node_count, file_count) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )?;
        for ((flow_id, name, entry_point_id, criticality, node_count, file_count, _), path_ids) in
            flow_rows.into_iter().zip(parsed_paths)
        {
            let entry_point = id_to_name
                .get(&entry_point_id)
                .cloned()
                .unwrap_or_else(|| entry_point_id.to_string());
            let mut critical_path = Vec::new();
            if !path_ids.is_empty() {
                critical_path.push(entry_point.clone());
                if path_ids.len() > 2 {
                    for node_id in path_ids.iter().skip(1).take(3) {
                        if let Some(name) = id_to_name.get(node_id) {
                            critical_path.push(name.clone());
                        }
                    }
                }
                if path_ids.len() > 1 {
                    if let Some(last) = path_ids.last().and_then(|node_id| id_to_name.get(node_id))
                    {
                        if !critical_path.contains(last) {
                            critical_path.push(last.clone());
                        }
                    }
                }
            }
            insert.execute(params![
                flow_id,
                name,
                entry_point,
                serde_json::to_string(&critical_path)?,
                criticality,
                node_count,
                file_count
            ])?;
        }
        drop(insert);
        tx.commit()?;
        Ok(())
    }

    fn compute_risk_index(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM risk_index", [])?;

        let mut caller_counts: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT target_qualified, COUNT(*) FROM edges \
                 WHERE kind = 'CALLS' GROUP BY target_qualified",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (qualified_name, count) = row?;
                caller_counts.insert(qualified_name, count);
            }
        }

        let mut tested_counts: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT source_qualified, COUNT(*) FROM edges \
                 WHERE kind = 'TESTED_BY' GROUP BY source_qualified",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (qualified_name, count) = row?;
                tested_counts.insert(qualified_name, count);
            }
        }

        let risk_nodes = {
            let mut stmt = tx.prepare(
                "SELECT id, qualified_name, name FROM nodes \
                 WHERE kind IN ('Function', 'Class', 'Test')",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let security_keywords = [
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
            "execute",
        ];
        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO risk_index \
             (node_id, qualified_name, risk_score, caller_count, test_coverage, \
              security_relevant, last_computed) \
             VALUES (?, ?, ?, ?, ?, ?, datetime('now'))",
        )?;
        for (node_id, qualified_name, name) in risk_nodes {
            let caller_count = *caller_counts.get(&qualified_name).unwrap_or(&0);
            let tested = *tested_counts.get(&qualified_name).unwrap_or(&0);
            let coverage = if tested > 0 { "tested" } else { "untested" };
            let name_lower = name.to_lowercase();
            let security_relevant = security_keywords
                .iter()
                .any(|keyword| name_lower.contains(keyword));
            let mut risk = 0.0_f64;
            if caller_count > 10 {
                risk += 0.3;
            } else if caller_count > 3 {
                risk += 0.15;
            }
            if coverage == "untested" {
                risk += 0.3;
            }
            if security_relevant {
                risk += 0.4;
            }
            insert.execute(params![
                node_id,
                qualified_name,
                risk.min(1.0),
                caller_count,
                coverage,
                if security_relevant { 1 } else { 0 }
            ])?;
        }
        drop(insert);
        tx.commit()?;
        Ok(())
    }

    pub fn store_file_nodes_edges(
        &mut self,
        file_path: &str,
        nodes: &[NodeInput],
        edges: &[EdgeInput],
        file_hash: &str,
        mtime_ns: i64,
    ) -> Result<()> {
        self.store_file_batch(&[(
            file_path.to_string(),
            nodes.to_vec(),
            edges.to_vec(),
            file_hash.to_string(),
            mtime_ns,
        )])
    }

    pub fn store_file_batch(&mut self, batch: &[FileBatchItem]) -> Result<()> {
        let suspend_indexes = !self.bulk_load_indexes_suspended;
        let tx = self.conn.transaction()?;
        store_file_batch_tx(&tx, batch, suspend_indexes)?;
        tx.commit()?;
        Ok(())
    }

    pub fn store_file_batch_json(&mut self, batch_json: &str) -> Result<()> {
        let compact: Vec<RawCompactFileBatchItem> = serde_json::from_str(batch_json)?;
        let suspend_indexes = !self.bulk_load_indexes_suspended;
        let tx = self.conn.transaction()?;
        store_raw_compact_file_batch_tx(&tx, &compact, suspend_indexes)?;
        tx.commit()?;
        Ok(())
    }

    pub fn begin_bulk_load(&mut self) -> Result<()> {
        if self.bulk_load_indexes_suspended {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        drop_graph_write_indexes(&tx)?;
        tx.commit()?;
        self.bulk_load_indexes_suspended = true;
        Ok(())
    }

    pub fn finish_bulk_load(&mut self) -> Result<()> {
        if !self.bulk_load_indexes_suspended {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        create_graph_write_indexes(&tx)?;
        tx.commit()?;
        self.bulk_load_indexes_suspended = false;
        Ok(())
    }

    pub fn get_all_files(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT file_path FROM nodes WHERE kind = 'File'")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_file_hashes(&self, file_paths: &[String]) -> Result<HashMap<String, String>> {
        if file_paths.is_empty() {
            return Ok(HashMap::new());
        }
        let mut out = HashMap::new();
        for chunk in file_paths.chunks(900) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT file_path, file_hash FROM nodes \
                 WHERE kind = 'File' AND file_path IN ({placeholders}) \
                   AND file_hash IS NOT NULL"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (file_path, file_hash) = row?;
                out.insert(file_path, file_hash);
            }
        }
        Ok(out)
    }

    pub fn get_file_meta_map(&self) -> Result<HashMap<String, (String, i64)>> {
        let mut out = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT file_path, file_hash, mtime_ns FROM nodes \
             WHERE file_hash IS NOT NULL AND file_hash != ''",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            ))
        })?;
        for row in rows {
            let (file_path, file_hash, mtime_ns) = row?;
            out.insert(file_path, (file_hash, mtime_ns));
        }
        Ok(out)
    }

    pub fn get_file_meta_for_files(
        &self,
        file_paths: &[String],
    ) -> Result<HashMap<String, (String, i64)>> {
        let mut out = HashMap::new();
        for chunk in file_paths.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT file_path, file_hash, mtime_ns FROM nodes \
                 WHERE file_hash IS NOT NULL AND file_hash != '' \
                   AND file_path IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            })?;
            for row in rows {
                let (file_path, file_hash, mtime_ns) = row?;
                out.insert(file_path, (file_hash, mtime_ns));
            }
        }
        Ok(out)
    }

    pub fn update_file_mtime(&self, file_path: &str, mtime_ns: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET mtime_ns = ? WHERE file_path = ?",
            params![mtime_ns, file_path],
        )?;
        Ok(())
    }

    pub fn update_file_mtimes(&mut self, updates: &[(String, i64)]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare("UPDATE nodes SET mtime_ns = ? WHERE file_path = ?")?;
            for (file_path, mtime_ns) in updates {
                stmt.execute(params![mtime_ns, file_path])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_node(&self, qualified_name: &str) -> Result<Option<GraphNode>> {
        for key in self.qualified_key_candidates(qualified_name)? {
            let row = self
                .conn
                .query_row(
                    "SELECT * FROM nodes WHERE qualified_name = ?",
                    [key],
                    node_from_row,
                )
                .optional()?;
            if row.is_some() {
                return Ok(row);
            }
        }
        Ok(None)
    }

    pub fn get_nodes_by_qualified_names(
        &self,
        qualified_names: &[String],
    ) -> Result<HashMap<String, GraphNode>> {
        if qualified_names.is_empty() {
            return Ok(HashMap::new());
        }

        let mut normalized_for = HashMap::new();
        let mut keys = HashSet::new();
        for qualified_name in qualified_names {
            let normalized = self.normalize_qualified_key(qualified_name)?;
            normalized_for.insert(qualified_name.clone(), normalized.clone());
            keys.insert(qualified_name.clone());
            if normalized != *qualified_name {
                keys.insert(normalized);
            }
        }

        let keys = keys.into_iter().collect::<Vec<_>>();
        let mut rows_by_qn = HashMap::new();
        for chunk in keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM nodes WHERE qualified_name IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                let node = row?;
                rows_by_qn
                    .entry(node.qualified_name.clone())
                    .or_insert(node);
            }
        }

        let mut out = HashMap::new();
        for original in qualified_names {
            if let Some(node) = rows_by_qn.get(original) {
                out.insert(original.clone(), node.clone());
                continue;
            }
            if let Some(normalized) = normalized_for.get(original) {
                if let Some(node) = rows_by_qn.get(normalized) {
                    out.insert(original.clone(), node.clone());
                }
            }
        }
        Ok(out)
    }

    pub fn get_nodes_by_ids(&self, node_ids: &[i64]) -> Result<HashMap<i64, GraphNode>> {
        let mut out = HashMap::new();
        if node_ids.is_empty() {
            return Ok(out);
        }

        let mut unique_ids = Vec::new();
        let mut seen = HashSet::new();
        for node_id in node_ids {
            if seen.insert(*node_id) {
                unique_ids.push(*node_id);
            }
        }

        for chunk in unique_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                let node = row?;
                out.insert(node.id, node);
            }
        }
        Ok(out)
    }

    pub fn get_nodes_by_file(&self, file_path: &str) -> Result<Vec<GraphNode>> {
        let mut seen = std::collections::HashSet::<i64>::new();
        let mut nodes = Vec::new();
        for key in self.file_key_candidates(file_path)? {
            let mut stmt = self
                .conn
                .prepare("SELECT * FROM nodes WHERE file_path = ?")?;
            let rows = stmt.query_map([key], node_from_row)?;
            for row in rows {
                let node = row?;
                if seen.insert(node.id) {
                    nodes.push(node);
                }
            }
        }
        Ok(nodes)
    }

    pub fn get_nodes_by_files(
        &self,
        file_paths: &[String],
    ) -> Result<HashMap<String, Vec<GraphNode>>> {
        let mut out = file_paths
            .iter()
            .map(|file_path| (file_path.clone(), Vec::new()))
            .collect::<HashMap<_, _>>();
        if file_paths.is_empty() {
            return Ok(out);
        }

        let mut key_to_originals: HashMap<String, Vec<String>> = HashMap::new();
        for file_path in file_paths {
            for key in self.file_key_candidates(file_path)? {
                key_to_originals
                    .entry(key)
                    .or_default()
                    .push(file_path.clone());
            }
        }

        let mut seen_by_original = file_paths
            .iter()
            .map(|file_path| (file_path.clone(), HashSet::new()))
            .collect::<HashMap<_, _>>();
        let keys = key_to_originals.keys().cloned().collect::<Vec<_>>();
        for chunk in keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM nodes WHERE file_path IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                let node = row?;
                if let Some(originals) = key_to_originals.get(&node.file_path) {
                    for original in originals {
                        if let Some(seen) = seen_by_original.get_mut(original) {
                            if seen.insert(node.id) {
                                out.entry(original.clone()).or_default().push(node.clone());
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn get_nodes_by_kind(
        &self,
        kinds: &[String],
        file_pattern: Option<&str>,
    ) -> Result<Vec<GraphNode>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", kinds.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = if file_pattern.is_some() {
            format!("SELECT * FROM nodes WHERE kind IN ({placeholders}) AND file_path LIKE ?")
        } else {
            format!("SELECT * FROM nodes WHERE kind IN ({placeholders})")
        };
        let mut params = kinds.iter().map(String::as_str).collect::<Vec<_>>();
        let pattern;
        if let Some(file_pattern) = file_pattern {
            pattern = format!("%{file_pattern}%");
            params.push(&pattern);
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_call_targets(&self, include_file_sources: bool) -> Result<HashSet<String>> {
        if include_file_sources {
            let mut stmt = self
                .conn
                .prepare("SELECT DISTINCT target_qualified FROM edges WHERE kind = 'CALLS'")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            return rows
                .collect::<std::result::Result<HashSet<_>, _>>()
                .map_err(Into::into);
        }
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT e.target_qualified FROM edges e \
             LEFT JOIN nodes n ON n.qualified_name = e.source_qualified \
             WHERE e.kind = 'CALLS' \
             AND (n.kind IS NULL OR n.kind != 'File')",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<HashSet<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_nodes(&self) -> Result<Vec<GraphNode>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes")?;
        let rows = stmt.query_map([], node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_nodes_filtered(&self, exclude_files: bool) -> Result<Vec<GraphNode>> {
        if !exclude_files {
            return self.get_all_nodes();
        }
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM nodes WHERE kind != 'File'")?;
        let rows = stmt.query_map([], node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_edges(&self) -> Result<Vec<GraphEdge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM edges")?;
        let rows = stmt.query_map([], edge_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn persist_centrality_scores(&mut self) -> Result<HashMap<String, i64>> {
        self.conn.execute_batch(CENTRALITY_SCORE_SCHEMA_SQL)?;
        let now = now_seconds()?;
        let nodes = self.get_all_nodes_filtered(true)?;
        let edges = self.get_all_edges()?;

        let mut node_by_qn = HashMap::<String, GraphNode>::new();
        for node in nodes {
            node_by_qn.insert(node.qualified_name.clone(), node);
        }

        let mut in_degree = HashMap::<String, i64>::new();
        let mut out_degree = HashMap::<String, i64>::new();
        let mut adjacency = HashMap::<String, Vec<String>>::new();
        let mut graph_nodes = HashSet::<String>::new();
        for edge in &edges {
            *out_degree.entry(edge.source_qualified.clone()).or_insert(0) += 1;
            *in_degree.entry(edge.target_qualified.clone()).or_insert(0) += 1;
            adjacency
                .entry(edge.source_qualified.clone())
                .or_default()
                .push(edge.target_qualified.clone());
            graph_nodes.insert(edge.source_qualified.clone());
            graph_nodes.insert(edge.target_qualified.clone());
        }

        let mut hubs = Vec::new();
        for node in node_by_qn.values() {
            let ind = *in_degree.get(&node.qualified_name).unwrap_or(&0);
            let outd = *out_degree.get(&node.qualified_name).unwrap_or(&0);
            let total = ind + outd;
            if total > 0 {
                hubs.push((
                    node.qualified_name.clone(),
                    sanitize_name(&node.name),
                    node.kind.clone(),
                    node.file_path.clone(),
                    ind,
                    outd,
                    total,
                    self.get_node_community_id(node.id)?,
                    now,
                ));
            }
        }
        hubs.sort_by(|a, b| b.6.cmp(&a.6).then_with(|| a.0.cmp(&b.0)));

        let bridge_scores = betweenness_centrality(&graph_nodes, &adjacency);
        let mut bridges = Vec::new();
        for (qualified_name, score) in bridge_scores {
            if score <= 0.0 {
                continue;
            }
            if let Some(node) = node_by_qn.get(&qualified_name) {
                bridges.push((
                    node.qualified_name.clone(),
                    sanitize_name(&node.name),
                    node.kind.clone(),
                    node.file_path.clone(),
                    (score * 1_000_000.0).round() / 1_000_000.0,
                    self.get_node_community_id(node.id)?,
                    now,
                ));
            }
        }
        bridges.sort_by(|a, b| b.4.total_cmp(&a.4).then_with(|| a.0.cmp(&b.0)));

        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM hub_scores", [])?;
        tx.execute("DELETE FROM bridge_scores", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO hub_scores \
                 (qualified_name, name, kind, file_path, in_degree, out_degree, total_degree, \
                  community_id, computed_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            for hub in &hubs {
                stmt.execute(params![
                    &hub.0, &hub.1, &hub.2, &hub.3, hub.4, hub.5, hub.6, hub.7, hub.8
                ])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO bridge_scores \
                 (qualified_name, name, kind, file_path, betweenness, community_id, computed_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            for bridge in &bridges {
                stmt.execute(params![
                    &bridge.0, &bridge.1, &bridge.2, &bridge.3, bridge.4, bridge.5, bridge.6
                ])?;
            }
        }
        tx.commit()?;

        Ok(HashMap::from([
            ("hub_scores_persisted".to_string(), hubs.len() as i64),
            ("bridge_scores_persisted".to_string(), bridges.len() as i64),
        ]))
    }

    pub fn get_stats(&self) -> Result<GraphStats> {
        let total_nodes = self
            .conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?;
        let total_edges = self
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;

        let mut nodes_by_kind = HashMap::new();
        let mut node_stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) as cnt FROM nodes GROUP BY kind")?;
        let node_rows = node_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in node_rows {
            let (kind, count) = row?;
            nodes_by_kind.insert(kind, count);
        }

        let mut edges_by_kind = HashMap::new();
        let mut edge_stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) as cnt FROM edges GROUP BY kind")?;
        let edge_rows = edge_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in edge_rows {
            let (kind, count) = row?;
            edges_by_kind.insert(kind, count);
        }

        let mut lang_stmt = self.conn.prepare(
            "SELECT DISTINCT language FROM nodes WHERE language IS NOT NULL AND language != ''",
        )?;
        let lang_rows = lang_stmt.query_map([], |row| row.get::<_, String>(0))?;
        let languages = lang_rows.collect::<std::result::Result<Vec<_>, _>>()?;

        let files_count = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE kind = 'File'",
            [],
            |row| row.get(0),
        )?;
        let last_updated = self.get_metadata("last_updated")?;

        Ok(GraphStats {
            total_nodes,
            total_edges,
            nodes_by_kind,
            edges_by_kind,
            languages,
            files_count,
            last_updated,
        })
    }

    pub fn get_nodes_by_community_id(&self, community_id: i64) -> Result<Vec<GraphNode>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM nodes WHERE community_id = ?")?;
        let rows = stmt.query_map([community_id], node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_files_matching(&self, pattern: &str) -> Result<Vec<String>> {
        let like = format!("%{pattern}");
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT file_path FROM nodes WHERE file_path LIKE ?")?;
        let rows = stmt.query_map([like], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn count_flow_memberships(&self, node_id: i64) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) as cnt FROM flow_memberships WHERE node_id = ?",
                [node_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn count_flow_memberships_for_nodes(&self, node_ids: &[i64]) -> Result<HashMap<i64, i64>> {
        let mut out = node_ids
            .iter()
            .map(|node_id| (*node_id, 0))
            .collect::<HashMap<_, _>>();
        if node_ids.is_empty() {
            return Ok(out);
        }

        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT node_id, COUNT(*) FROM flow_memberships \
                 WHERE node_id IN ({placeholders}) GROUP BY node_id"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (node_id, count) = row?;
                out.insert(node_id, count);
            }
        }
        Ok(out)
    }

    pub fn get_flow_criticalities_for_node(&self, node_id: i64) -> Result<Vec<f64>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.criticality FROM flows f \
             JOIN flow_memberships fm ON fm.flow_id = f.id \
             WHERE fm.node_id = ?",
        )?;
        let rows = stmt.query_map([node_id], |row| row.get::<_, f64>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_flow_criticalities_for_nodes(
        &self,
        node_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<f64>>> {
        let mut out = node_ids
            .iter()
            .map(|node_id| (*node_id, Vec::new()))
            .collect::<HashMap<_, _>>();
        if node_ids.is_empty() {
            return Ok(out);
        }

        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT fm.node_id, f.criticality FROM flows f \
                 JOIN flow_memberships fm ON fm.flow_id = f.id \
                 WHERE fm.node_id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })?;
            for row in rows {
                let (node_id, criticality) = row?;
                out.entry(node_id).or_default().push(criticality);
            }
        }
        Ok(out)
    }

    pub fn get_node_community_id(&self, node_id: i64) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT community_id FROM nodes WHERE id = ?",
                [node_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(|row| row.flatten())
            .map_err(Into::into)
    }

    pub fn get_community_ids_by_node_ids(
        &self,
        node_ids: &[i64],
    ) -> Result<HashMap<i64, Option<i64>>> {
        let mut out = node_ids
            .iter()
            .map(|node_id| (*node_id, None))
            .collect::<HashMap<_, _>>();
        if node_ids.is_empty() {
            return Ok(out);
        }

        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id, community_id FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
            })?;
            for row in rows {
                let (node_id, community_id) = row?;
                out.insert(node_id, community_id);
            }
        }
        Ok(out)
    }

    pub fn get_community_ids_by_qualified_names(
        &self,
        qns: &[String],
    ) -> Result<HashMap<String, Option<i64>>> {
        let mut out = HashMap::new();
        for chunk in qns.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT qualified_name, community_id FROM nodes \
                 WHERE qualified_name IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })?;
            for row in rows {
                let (qualified_name, community_id) = row?;
                out.insert(qualified_name, community_id);
            }
        }
        Ok(out)
    }

    pub fn get_transitive_tests(&self, qualified_name: &str, max_depth: i64) -> Result<Vec<Value>> {
        let mut seen = HashSet::new();
        let mut results = Vec::new();

        let mut input_qns = vec![qualified_name.to_string()];
        let node_kind = self
            .conn
            .query_row(
                "SELECT kind FROM nodes WHERE qualified_name = ?",
                [qualified_name],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if node_kind.as_deref() == Some("Class") {
            let mut stmt = self.conn.prepare(
                "SELECT target_qualified FROM edges \
                 WHERE source_qualified = ? AND kind = 'CONTAINS'",
            )?;
            let rows = stmt.query_map([qualified_name], |row| row.get::<_, String>(0))?;
            for row in rows {
                input_qns.push(row?);
            }
        }

        for qn in &input_qns {
            for test_target in self.get_test_targets_for_source(qn)? {
                if seen.insert(test_target.clone()) {
                    if let Some(test_node) = self.test_node_json(&test_target, false)? {
                        results.push(test_node);
                    }
                }
            }
        }

        let bare = qualified_name
            .rsplit_once("::")
            .map(|(_, name)| name)
            .unwrap_or(qualified_name);
        for test_target in self.get_test_targets_for_source(bare)? {
            if seen.insert(test_target.clone()) {
                if let Some(test_node) = self.test_node_json(&test_target, false)? {
                    results.push(test_node);
                }
            }
        }

        let mut frontier = input_qns.into_iter().collect::<HashSet<_>>();
        for _ in 0..max_depth {
            let mut next_frontier = HashSet::new();
            for qn in &frontier {
                let mut stmt = self.conn.prepare(
                    "SELECT target_qualified FROM edges \
                     WHERE source_qualified = ? AND kind = 'CALLS'",
                )?;
                let rows = stmt.query_map([qn], |row| row.get::<_, String>(0))?;
                for row in rows {
                    next_frontier.insert(row?);
                }
            }
            for callee in &next_frontier {
                for test_target in self.get_test_targets_for_source(callee)? {
                    if seen.insert(test_target.clone()) {
                        if let Some(test_node) = self.test_node_json(&test_target, true)? {
                            results.push(test_node);
                        }
                    }
                }
            }
            frontier = next_frontier;
        }

        Ok(results)
    }

    fn get_transitive_test_counts(
        &self,
        qualified_names: &[String],
        max_depth: i64,
    ) -> Result<HashMap<String, i64>> {
        let mut seen_tests = qualified_names
            .iter()
            .map(|qualified_name| (qualified_name.clone(), HashSet::new()))
            .collect::<HashMap<_, HashSet<String>>>();
        if qualified_names.is_empty() {
            return Ok(HashMap::new());
        }

        let node_kinds = self.get_node_kinds_by_qualified_names(qualified_names)?;
        let class_qns = qualified_names
            .iter()
            .filter(|qualified_name| {
                node_kinds
                    .get(*qualified_name)
                    .is_some_and(|kind| kind == "Class")
            })
            .cloned()
            .collect::<Vec<_>>();
        let contains_by_class = self.get_contains_targets_by_sources(&class_qns)?;

        let mut direct_target_to_originals: HashMap<String, Vec<String>> = HashMap::new();
        let mut frontier_source_to_originals: HashMap<String, Vec<String>> = HashMap::new();
        for qualified_name in qualified_names {
            direct_target_to_originals
                .entry(qualified_name.clone())
                .or_default()
                .push(qualified_name.clone());
            frontier_source_to_originals
                .entry(qualified_name.clone())
                .or_default()
                .push(qualified_name.clone());

            if let Some(bare) = qualified_name.rsplit_once("::").map(|(_, name)| name) {
                direct_target_to_originals
                    .entry(bare.to_string())
                    .or_default()
                    .push(qualified_name.clone());
            }

            if let Some(contained) = contains_by_class.get(qualified_name) {
                for target in contained {
                    direct_target_to_originals
                        .entry(target.clone())
                        .or_default()
                        .push(qualified_name.clone());
                    frontier_source_to_originals
                        .entry(target.clone())
                        .or_default()
                        .push(qualified_name.clone());
                }
            }
        }

        let direct_targets = direct_target_to_originals
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for (source, test_target) in self.get_test_targets_by_sources(&direct_targets)? {
            if let Some(originals) = direct_target_to_originals.get(&source) {
                for original in originals {
                    if let Some(seen) = seen_tests.get_mut(original) {
                        seen.insert(test_target.clone());
                    }
                }
            }
        }

        let mut frontier = frontier_source_to_originals;
        for _ in 0..max_depth {
            if frontier.is_empty() {
                break;
            }
            let sources = frontier.keys().cloned().collect::<Vec<_>>();
            let calls_by_source = self.get_call_targets_by_sources(&sources)?;
            let mut callee_to_originals: HashMap<String, Vec<String>> = HashMap::new();
            for (source, callees) in calls_by_source {
                let Some(originals) = frontier.get(&source) else {
                    continue;
                };
                for callee in callees {
                    callee_to_originals
                        .entry(callee)
                        .or_default()
                        .extend(originals.iter().cloned());
                }
            }

            let callees = callee_to_originals.keys().cloned().collect::<Vec<_>>();
            for (source, test_target) in self.get_test_targets_by_sources(&callees)? {
                if let Some(originals) = callee_to_originals.get(&source) {
                    for original in originals {
                        if let Some(seen) = seen_tests.get_mut(original) {
                            seen.insert(test_target.clone());
                        }
                    }
                }
            }
            frontier = callee_to_originals;
        }

        Ok(seen_tests
            .into_iter()
            .map(|(qualified_name, seen)| (qualified_name, seen.len() as i64))
            .collect())
    }

    fn get_node_kinds_by_qualified_names(
        &self,
        qualified_names: &[String],
    ) -> Result<HashMap<String, String>> {
        let mut out = HashMap::new();
        for chunk in qualified_names.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT qualified_name, kind FROM nodes WHERE qualified_name IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (qualified_name, kind) = row?;
                out.insert(qualified_name, kind);
            }
        }
        Ok(out)
    }

    fn get_contains_targets_by_sources(
        &self,
        source_qualified_names: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        self.get_edge_targets_by_sources(source_qualified_names, "CONTAINS")
    }

    fn get_call_targets_by_sources(
        &self,
        source_qualified_names: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        self.get_edge_targets_by_sources(source_qualified_names, "CALLS")
    }

    fn get_edge_targets_by_sources(
        &self,
        source_qualified_names: &[String],
        kind: &str,
    ) -> Result<HashMap<String, Vec<String>>> {
        let mut out = HashMap::new();
        for chunk in source_qualified_names.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT source_qualified, target_qualified FROM edges \
                 WHERE kind = ? AND source_qualified IN ({placeholders})"
            );
            let mut params = Vec::with_capacity(chunk.len() + 1);
            params.push(kind.to_string());
            params.extend(chunk.iter().cloned());
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (source, target) = row?;
                out.entry(source).or_insert_with(Vec::new).push(target);
            }
        }
        Ok(out)
    }

    fn get_test_targets_by_sources(&self, sources: &[String]) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        for chunk in sources.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT e.source_qualified, e.target_qualified FROM edges e \
                 JOIN nodes n ON n.qualified_name = e.target_qualified \
                 WHERE e.kind = 'TESTED_BY' AND e.source_qualified IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn count_affected_communities(&self, file_paths: &[String]) -> Result<i64> {
        let mut community_ids = HashSet::new();
        for chunk in file_paths.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT community_id FROM nodes \
                 WHERE community_id IS NOT NULL AND file_path IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, i64>(0)
            })?;
            for row in rows {
                community_ids.insert(row?);
            }
        }
        Ok(community_ids.len() as i64)
    }

    pub fn get_flow_edge_data(&self) -> Result<FlowEdgeData> {
        let mut calls_out: HashMap<String, Vec<String>> = HashMap::new();
        let mut has_tested_by: HashSet<String> = HashSet::new();
        let mut stmt = self.conn.prepare(
            "SELECT kind, source_qualified, target_qualified FROM edges \
             WHERE kind IN ('CALLS', 'TESTED_BY')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (kind, source, target) = row?;
            if kind == "CALLS" {
                calls_out.entry(source).or_default().push(target);
            } else {
                has_tested_by.insert(source);
            }
        }
        Ok((calls_out, has_tested_by))
    }

    pub fn store_flows(&mut self, flows: &[FlowInput]) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM flow_snapshots", [])?;
        tx.execute("DELETE FROM flow_memberships", [])?;
        tx.execute("DELETE FROM flows", [])?;
        store_flows_tx(&tx, flows)?;
        tx.commit()?;
        Ok(flows.len() as i64)
    }

    pub fn store_flows_json(&mut self, flows_json: &str) -> Result<i64> {
        let flows: Vec<FlowInput> = serde_json::from_str(flows_json)?;
        self.store_flows(&flows)
    }

    pub fn insert_flows_json(&mut self, flows_json: &str) -> Result<i64> {
        let flows: Vec<FlowInput> = serde_json::from_str(flows_json)?;
        let tx = self.conn.transaction()?;
        store_flows_tx(&tx, &flows)?;
        tx.commit()?;
        Ok(flows.len() as i64)
    }

    pub fn get_flows_json(&self, sort_by: &str, limit: i64) -> Result<String> {
        let sort_by = match sort_by {
            "criticality" | "depth" | "node_count" | "file_count" | "name" => sort_by,
            _ => "criticality",
        };
        let order = if matches!(
            sort_by,
            "criticality" | "depth" | "node_count" | "file_count"
        ) {
            "DESC"
        } else {
            "ASC"
        };
        let sql = format!("SELECT * FROM flows ORDER BY {sort_by} {order} LIMIT ?");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([limit], flow_json_from_row)?;
        let flows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        serde_json::to_string(&flows).map_err(Into::into)
    }

    pub fn get_flow_by_id_json(&self, flow_id: i64) -> Result<Option<String>> {
        self.get_flow_values_by_ids(&[flow_id])?
            .into_iter()
            .next()
            .map(|flow| serde_json::to_string(&flow).map_err(Into::into))
            .transpose()
    }

    pub fn get_affected_flows_json(&self, changed_files: &[String]) -> Result<String> {
        let flows = self.get_affected_flow_values(changed_files)?;
        serde_json::to_string(&flows).map_err(Into::into)
    }

    fn get_affected_flow_values(&self, changed_files: &[String]) -> Result<Vec<Value>> {
        if changed_files.is_empty() {
            return Ok(Vec::new());
        }
        let node_ids = self.get_node_ids_by_files(changed_files)?;
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let flow_ids = self.get_flow_ids_by_node_ids(&node_ids)?;
        if flow_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut flows = self.get_flow_values_by_ids(&flow_ids)?;
        flows.sort_by(|left, right| {
            let left = left
                .get("criticality")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let right = right
                .get("criticality")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            right
                .partial_cmp(&left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(flows)
    }

    pub fn analyze_changes_json(
        &self,
        changed_files: &[String],
        changed_ranges_json: Option<&str>,
    ) -> Result<String> {
        let changed_ranges = match changed_ranges_json {
            Some(raw) if !raw.is_empty() => serde_json::from_str::<ChangedRanges>(raw)?,
            _ => HashMap::new(),
        };
        let changed_nodes = if changed_ranges.is_empty() {
            self.changed_nodes_by_files(changed_files)?
        } else {
            self.changed_nodes_by_ranges(&changed_ranges)?
        };
        let changed_funcs = changed_nodes
            .into_iter()
            .filter(|node| matches!(node.kind.as_str(), "Function" | "Test" | "Class"))
            .collect::<Vec<_>>();

        let func_ids = changed_funcs.iter().map(|node| node.id).collect::<Vec<_>>();
        let func_qns = changed_funcs
            .iter()
            .map(|node| node.qualified_name.clone())
            .collect::<Vec<_>>();

        let flow_crit_map = self.get_flow_criticalities_for_nodes(&func_ids)?;
        let nodes_needing_count = flow_crit_map
            .iter()
            .filter_map(|(node_id, values)| {
                if values.is_empty() {
                    Some(*node_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let flow_count_map = if nodes_needing_count.is_empty() {
            HashMap::new()
        } else {
            self.count_flow_memberships_for_nodes(&nodes_needing_count)?
        };
        let node_cid_map = self.get_community_ids_by_node_ids(&func_ids)?;
        let (outbound_map, inbound_map) = self.get_edges_by_endpoints(&func_qns)?;

        let mut caller_qns = HashSet::new();
        for edges in inbound_map.values() {
            for edge in edges {
                if edge.kind == "CALLS" {
                    caller_qns.insert(edge.source_qualified.clone());
                }
            }
        }
        let caller_qns = caller_qns.into_iter().collect::<Vec<_>>();
        let caller_cid_map = if caller_qns.is_empty() {
            HashMap::new()
        } else {
            self.get_community_ids_by_qualified_names(&caller_qns)?
        };
        let transitive_test_counts = self.get_transitive_test_counts(&func_qns, 1)?;

        let mut node_risks = Vec::new();
        for node in &changed_funcs {
            let inbound_edges = inbound_map
                .get(&node.qualified_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let flow_criticalities = flow_crit_map
                .get(&node.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let flow_count = *flow_count_map.get(&node.id).unwrap_or(&0);
            let risk = self.compute_change_risk_score(ChangeRiskInputs {
                node,
                inbound_edges,
                flow_criticalities,
                flow_count,
                node_community_id: node_cid_map.get(&node.id).copied().flatten(),
                caller_community_ids: &caller_cid_map,
                transitive_test_count: *transitive_test_counts
                    .get(&node.qualified_name)
                    .unwrap_or(&0),
            })?;
            let mut value = node_to_value(node);
            if let Some(obj) = value.as_object_mut() {
                obj.insert("risk_score".to_string(), json!(risk));
            }
            node_risks.push(value);
        }

        let overall_risk = node_risks
            .iter()
            .filter_map(|value| value.get("risk_score").and_then(Value::as_f64))
            .fold(0.0, f64::max);
        let affected_flows = self.get_affected_flow_values(changed_files)?;

        let mut test_gaps = Vec::new();
        for node in &changed_funcs {
            if node.is_test {
                continue;
            }
            let tested = outbound_map
                .get(&node.qualified_name)
                .map(|edges| edges.iter().any(|edge| edge.kind == "TESTED_BY"))
                .unwrap_or(false);
            if !tested {
                test_gaps.push(json!({
                    "name": sanitize_name(&node.name),
                    "qualified_name": sanitize_name(&node.qualified_name),
                    "file": node.file_path,
                    "line_start": node.line_start,
                    "line_end": node.line_end,
                }));
            }
        }

        let mut review_priorities = node_risks.clone();
        review_priorities.sort_by(|left, right| {
            let left = left
                .get("risk_score")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let right = right
                .get("risk_score")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            right
                .partial_cmp(&left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        review_priorities.truncate(10);

        let mut summary_parts = vec![
            format!("Analyzed {} changed file(s):", changed_files.len()),
            format!("  - {} changed function(s)/class(es)", changed_funcs.len()),
            format!("  - {} affected flow(s)", affected_flows.len()),
            format!("  - {} test gap(s)", test_gaps.len()),
            format!("  - Overall risk score: {overall_risk:.2}"),
        ];
        if !test_gaps.is_empty() {
            let gap_names = test_gaps
                .iter()
                .take(5)
                .filter_map(|gap| gap.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ");
            summary_parts.push(format!("  - Untested: {gap_names}"));
        }

        serde_json::to_string(&json!({
            "summary": summary_parts.join("\n"),
            "risk_score": overall_risk,
            "changed_functions": node_risks,
            "affected_flows": affected_flows,
            "test_gaps": test_gaps,
            "review_priorities": review_priorities,
        }))
        .map_err(Into::into)
    }

    pub fn delete_affected_flows(&mut self, changed_files: &[String]) -> Result<Vec<i64>> {
        if changed_files.is_empty() {
            return Ok(Vec::new());
        }
        let node_ids = self.get_node_ids_by_files(changed_files)?;
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let flow_ids = self.get_flow_ids_by_node_ids(&node_ids)?;
        if flow_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut entry_point_ids = Vec::new();
        let mut seen_entry_points = HashSet::new();
        for chunk in flow_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT entry_point_id FROM flows WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, i64>(0)
            })?;
            for row in rows {
                let entry_point_id = row?;
                if seen_entry_points.insert(entry_point_id) {
                    entry_point_ids.push(entry_point_id);
                }
            }
        }

        let tx = self.conn.transaction()?;
        {
            let mut delete_snapshot = tx.prepare("DELETE FROM flow_snapshots WHERE flow_id = ?")?;
            let mut delete_membership =
                tx.prepare("DELETE FROM flow_memberships WHERE flow_id = ?")?;
            let mut delete_flow = tx.prepare("DELETE FROM flows WHERE id = ?")?;
            for flow_id in flow_ids {
                delete_snapshot.execute([flow_id])?;
                delete_membership.execute([flow_id])?;
                delete_flow.execute([flow_id])?;
            }
        }
        tx.commit()?;
        Ok(entry_point_ids)
    }

    pub fn get_node_kind_by_id(&self, node_id: i64) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT kind FROM nodes WHERE id = ?", [node_id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn store_communities_json(&mut self, communities_json: &str) -> Result<i64> {
        let communities: Vec<CommunityInput> = serde_json::from_str(communities_json)?;
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM community_summaries", [])?;
        tx.execute("DELETE FROM communities", [])?;
        tx.execute("UPDATE nodes SET community_id = NULL", [])?;
        let mut insert = tx.prepare(
            "INSERT INTO communities \
             (name, level, cohesion, size, dominant_language, description) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )?;
        for community in &communities {
            insert.execute(params![
                community.name,
                community.level,
                community.cohesion,
                community.size,
                community.dominant_language,
                community.description
            ])?;
            let community_id = tx.last_insert_rowid();
            for chunk in community.members.chunks(450) {
                if chunk.is_empty() {
                    continue;
                }
                let placeholders = std::iter::repeat_n("?", chunk.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "UPDATE nodes SET community_id = ? WHERE qualified_name IN ({placeholders})"
                );
                let mut params = Vec::with_capacity(chunk.len() + 1);
                params.push(rusqlite::types::Value::Integer(community_id));
                params.extend(chunk.iter().cloned().map(rusqlite::types::Value::Text));
                tx.execute(&sql, rusqlite::params_from_iter(params))?;
            }
        }
        drop(insert);
        tx.commit()?;
        Ok(communities.len() as i64)
    }

    pub fn get_communities_json(&self, sort_by: &str, min_size: i64) -> Result<String> {
        let sort_by = match sort_by {
            "size" | "cohesion" | "name" => sort_by,
            _ => "size",
        };
        let order = if matches!(sort_by, "size" | "cohesion") {
            "DESC"
        } else {
            "ASC"
        };
        let sql = format!("SELECT * FROM communities WHERE size >= ? ORDER BY {sort_by} {order}");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([min_size], community_json_from_row)?;
        let mut communities = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        let community_ids = communities
            .iter()
            .filter_map(|community| community.get("id").and_then(Value::as_i64))
            .collect::<Vec<_>>();
        let members_by_community = self.get_community_member_qns_by_ids(&community_ids)?;
        for community in &mut communities {
            let id = community.get("id").and_then(Value::as_i64).unwrap_or(0);
            let members = members_by_community.get(&id).cloned().unwrap_or_default();
            if let Some(obj) = community.as_object_mut() {
                obj.insert(
                    "members".to_string(),
                    Value::Array(
                        members
                            .into_iter()
                            .map(|member| Value::String(sanitize_name(&member)))
                            .collect(),
                    ),
                );
            }
        }
        serde_json::to_string(&communities).map_err(Into::into)
    }

    fn get_community_member_qns_by_ids(
        &self,
        community_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<String>>> {
        let mut out = HashMap::new();
        for chunk in community_ids.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT community_id, qualified_name FROM nodes \
                 WHERE community_id IN ({placeholders}) ORDER BY community_id"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (community_id, qualified_name) = row?;
                out.entry(community_id)
                    .or_insert_with(Vec::new)
                    .push(qualified_name);
            }
        }
        Ok(out)
    }

    pub fn get_all_community_member_qns(&self) -> Result<HashMap<i64, Vec<String>>> {
        let mut out = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT community_id, qualified_name FROM nodes \
             WHERE community_id IS NOT NULL ORDER BY community_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (community_id, qualified_name) = row?;
            out.entry(community_id)
                .or_insert_with(Vec::new)
                .push(qualified_name);
        }
        Ok(out)
    }

    fn get_test_targets_for_source(&self, source_qualified: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_qualified FROM edges \
             WHERE source_qualified = ? AND kind = 'TESTED_BY'",
        )?;
        let rows = stmt.query_map([source_qualified], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn test_node_json(&self, qualified_name: &str, indirect: bool) -> Result<Option<Value>> {
        self.conn
            .query_row(
                "SELECT name, qualified_name, file_path, kind FROM nodes \
                 WHERE qualified_name = ?",
                [qualified_name],
                |row| {
                    Ok(json!({
                        "name": row.get::<_, String>(0)?,
                        "qualified_name": row.get::<_, String>(1)?,
                        "file_path": row.get::<_, String>(2)?,
                        "kind": row.get::<_, String>(3)?,
                        "indirect": indirect,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn get_node_ids_by_files(&self, file_paths: &[String]) -> Result<HashSet<i64>> {
        let mut out = HashSet::new();
        let file_keys = self.expand_file_keys(file_paths)?;
        for chunk in file_keys.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id FROM nodes WHERE file_path IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| row.get(0))?;
            for row in rows {
                out.insert(row?);
            }
        }
        Ok(out)
    }

    fn expand_file_keys(&self, file_paths: &[String]) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut seen = HashSet::new();
        for file_path in file_paths {
            for key in self.file_key_candidates(file_path)? {
                if seen.insert(key.clone()) {
                    keys.push(key);
                }
            }
        }
        Ok(keys)
    }

    fn changed_nodes_by_files(&self, changed_files: &[String]) -> Result<Vec<GraphNode>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let nodes_by_file = self.get_nodes_by_files(changed_files)?;
        for file_path in changed_files {
            if let Some(nodes) = nodes_by_file.get(file_path) {
                for node in nodes {
                    if seen.insert(node.qualified_name.clone()) {
                        out.push(node.clone());
                    }
                }
            }
        }
        Ok(out)
    }

    fn changed_nodes_by_ranges(&self, changed_ranges: &ChangedRanges) -> Result<Vec<GraphNode>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let file_paths = changed_ranges.keys().cloned().collect::<Vec<_>>();
        let nodes_by_file = self.get_nodes_by_files(&file_paths)?;
        for (file_path, ranges) in changed_ranges {
            let mut nodes = nodes_by_file.get(file_path).cloned().unwrap_or_default();
            if nodes.is_empty() {
                let matched_paths = self.get_files_matching(file_path)?;
                let matched_nodes = self.get_nodes_by_files(&matched_paths)?;
                for matched_path in matched_paths {
                    if let Some(found) = matched_nodes.get(&matched_path) {
                        nodes.extend(found.iter().cloned());
                    }
                }
            }
            for node in nodes {
                if seen.contains(&node.qualified_name) {
                    continue;
                }
                if ranges
                    .iter()
                    .any(|(start, end)| node.line_start <= *end && node.line_end >= *start)
                    && seen.insert(node.qualified_name.clone())
                {
                    out.push(node);
                }
            }
        }
        Ok(out)
    }

    fn compute_change_risk_score(&self, inputs: ChangeRiskInputs<'_>) -> Result<f64> {
        let mut score = 0.0_f64;

        if inputs.flow_criticalities.is_empty() {
            score += (inputs.flow_count as f64 * 0.05).min(0.25);
        } else {
            score += inputs.flow_criticalities.iter().sum::<f64>().min(0.25);
        }

        let caller_edges = inputs
            .inbound_edges
            .iter()
            .filter(|edge| edge.kind == "CALLS")
            .collect::<Vec<_>>();
        if let Some(node_cid) = inputs.node_community_id {
            let cross_community = caller_edges
                .iter()
                .filter(|edge| {
                    inputs
                        .caller_community_ids
                        .get(&edge.source_qualified)
                        .and_then(|cid| *cid)
                        .is_some_and(|cid| cid != node_cid)
                })
                .count();
            score += (cross_community as f64 * 0.05).min(0.15);
        }

        score += 0.30 - ((inputs.transitive_test_count as f64 / 5.0).min(1.0) * 0.25);

        let name_lower = inputs.node.name.to_lowercase();
        let qn_lower = inputs.node.qualified_name.to_lowercase();
        if SECURITY_KEYWORDS
            .iter()
            .any(|keyword| name_lower.contains(keyword) || qn_lower.contains(keyword))
        {
            score += 0.20;
        }

        score += (caller_edges.len() as f64 / 20.0).min(0.10);
        Ok((score.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0)
    }

    fn get_flow_ids_by_node_ids(&self, node_ids: &HashSet<i64>) -> Result<Vec<i64>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let node_ids = node_ids.iter().copied().collect::<Vec<_>>();
        for chunk in node_ids.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT flow_id FROM flow_memberships WHERE node_id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| row.get(0))?;
            for row in rows {
                let flow_id = row?;
                if seen.insert(flow_id) {
                    out.push(flow_id);
                }
            }
        }
        Ok(out)
    }

    pub fn get_flow_qualified_names_for_flows(
        &self,
        flow_ids: &[i64],
    ) -> Result<HashMap<i64, HashSet<String>>> {
        let mut out = flow_ids
            .iter()
            .map(|flow_id| (*flow_id, HashSet::new()))
            .collect::<HashMap<_, _>>();
        if flow_ids.is_empty() {
            return Ok(out);
        }

        let mut unique_ids = Vec::new();
        let mut seen = HashSet::new();
        for flow_id in flow_ids {
            if seen.insert(*flow_id) {
                unique_ids.push(*flow_id);
            }
        }

        for chunk in unique_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT fm.flow_id, n.qualified_name \
                 FROM flow_memberships fm \
                 JOIN nodes n ON fm.node_id = n.id \
                 WHERE fm.flow_id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (flow_id, qualified_name) = row?;
                out.entry(flow_id).or_default().insert(qualified_name);
            }
        }
        Ok(out)
    }

    fn get_flow_values_by_ids(&self, flow_ids: &[i64]) -> Result<Vec<Value>> {
        if flow_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut flows = Vec::new();
        for chunk in flow_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM flows WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), flow_value_from_row)?;
            for row in rows {
                flows.push(row?);
            }
        }

        let mut path_node_ids = HashSet::new();
        for flow in &flows {
            path_node_ids.extend(flow.path_ids.iter().copied());
        }
        let path_node_ids = path_node_ids.into_iter().collect::<Vec<_>>();
        let nodes_by_id = self.get_nodes_by_ids(&path_node_ids)?;

        for flow in &mut flows {
            let steps = flow_steps_from_nodes(&flow.path_ids, &nodes_by_id);
            if let Some(obj) = flow.value.as_object_mut() {
                obj.insert("steps".to_string(), Value::Array(steps));
            }
        }
        Ok(flows.into_iter().map(|flow| flow.value).collect())
    }

    pub fn get_edges_by_source(&self, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        self.get_edges_by_endpoint("source_qualified", qualified_name)
    }

    pub fn get_edges_by_target(&self, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        self.get_edges_by_endpoint("target_qualified", qualified_name)
    }

    pub fn get_edges_by_endpoints(
        &self,
        qualified_names: &[String],
    ) -> Result<(EdgeEndpointMap, EdgeEndpointMap)> {
        let mut outgoing = qualified_names
            .iter()
            .map(|qn| (qn.clone(), Vec::new()))
            .collect::<HashMap<_, _>>();
        let mut incoming = qualified_names
            .iter()
            .map(|qn| (qn.clone(), Vec::new()))
            .collect::<HashMap<_, _>>();
        if qualified_names.is_empty() {
            return Ok((outgoing, incoming));
        }

        let mut keys = HashSet::new();
        let mut normalized_to_originals: HashMap<String, Vec<String>> = HashMap::new();
        for qn in qualified_names {
            keys.insert(qn.clone());
            let normalized = self.normalize_qualified_key(qn)?;
            keys.insert(normalized.clone());
            normalized_to_originals
                .entry(normalized)
                .or_default()
                .push(qn.clone());
        }

        let mut seen_out = qualified_names
            .iter()
            .map(|qn| (qn.clone(), HashSet::new()))
            .collect::<HashMap<_, _>>();
        let mut seen_in = qualified_names
            .iter()
            .map(|qn| (qn.clone(), HashSet::new()))
            .collect::<HashMap<_, _>>();
        let keys = keys.into_iter().collect::<Vec<_>>();
        for chunk in keys.chunks(225) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT * FROM edges \
                 WHERE source_qualified IN ({placeholders}) \
                 OR target_qualified IN ({placeholders})"
            );
            let mut params = Vec::with_capacity(chunk.len() * 2);
            params.extend(chunk.iter().cloned());
            params.extend(chunk.iter().cloned());
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), edge_from_row)?;
            for row in rows {
                let edge = row?;
                if let Some(source_originals) = normalized_to_originals.get(&edge.source_qualified)
                {
                    for original in source_originals {
                        if let Some(seen) = seen_out.get_mut(original) {
                            if seen.insert(edge.id) {
                                outgoing
                                    .entry(original.clone())
                                    .or_default()
                                    .push(edge.clone());
                            }
                        }
                    }
                } else if outgoing.contains_key(&edge.source_qualified) {
                    if let Some(seen) = seen_out.get_mut(&edge.source_qualified) {
                        if seen.insert(edge.id) {
                            outgoing
                                .entry(edge.source_qualified.clone())
                                .or_default()
                                .push(edge.clone());
                        }
                    }
                }
                if let Some(target_originals) = normalized_to_originals.get(&edge.target_qualified)
                {
                    for original in target_originals {
                        if let Some(seen) = seen_in.get_mut(original) {
                            if seen.insert(edge.id) {
                                incoming
                                    .entry(original.clone())
                                    .or_default()
                                    .push(edge.clone());
                            }
                        }
                    }
                } else if incoming.contains_key(&edge.target_qualified) {
                    if let Some(seen) = seen_in.get_mut(&edge.target_qualified) {
                        if seen.insert(edge.id) {
                            incoming
                                .entry(edge.target_qualified.clone())
                                .or_default()
                                .push(edge.clone());
                        }
                    }
                }
            }
        }
        Ok((outgoing, incoming))
    }

    pub fn get_direct_dependents(&self, file_paths: &[String]) -> Result<Vec<String>> {
        if file_paths.is_empty() {
            return Ok(Vec::new());
        }

        let mut dependents = HashSet::new();
        let mut fp_keys = Vec::new();
        let mut seen_keys = HashSet::new();
        for file_path in file_paths {
            for key in self.qualified_key_candidates(file_path)? {
                if seen_keys.insert(key.clone()) {
                    fp_keys.push(key);
                }
            }
        }

        for chunk in fp_keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT file_path FROM edges \
                 WHERE target_qualified IN ({placeholders}) AND kind = 'IMPORTS_FROM'"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                dependents.insert(row?);
            }
        }

        let file_keys = self.expand_file_keys(file_paths)?;
        let mut node_qns = Vec::new();
        for chunk in file_keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT qualified_name FROM nodes WHERE file_path IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                node_qns.push(row?);
            }
        }

        for chunk in node_qns.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT file_path FROM edges \
                 WHERE target_qualified IN ({placeholders}) \
                   AND kind IN ('CALLS', 'IMPORTS_FROM', 'INHERITS', 'IMPLEMENTS')"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                dependents.insert(row?);
            }
        }

        for file_path in file_paths {
            dependents.remove(file_path);
        }
        let mut out = dependents.into_iter().collect::<Vec<_>>();
        out.sort();
        Ok(out)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        Ok(())
    }

    fn run_migrations(&self) -> Result<()> {
        let current = self.schema_version()?;
        if current >= LATEST_VERSION {
            return Ok(());
        }
        for version in (current + 1)..=LATEST_VERSION {
            match version {
                2 => self.migrate_v2()?,
                3 => self.migrate_v3()?,
                4 => self.migrate_v4()?,
                5 => self.migrate_v5()?,
                6 => self.migrate_v6()?,
                7 => self.migrate_v7()?,
                8 => self.migrate_v8()?,
                9 => self.migrate_v9()?,
                10 => self.migrate_v10()?,
                11 => self.migrate_v11()?,
                12 => self.migrate_v12()?,
                13 => self.migrate_v13()?,
                14 => self.migrate_v14()?,
                _ => {}
            }
            self.set_metadata("schema_version", &version.to_string())?;
        }
        Ok(())
    }

    fn migrate_v2(&self) -> Result<()> {
        if !has_column(&self.conn, "nodes", "signature")? {
            self.conn
                .execute("ALTER TABLE nodes ADD COLUMN signature TEXT", [])?;
        }
        Ok(())
    }

    fn migrate_v3(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS flows (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                entry_point_id INTEGER NOT NULL,
                depth INTEGER NOT NULL,
                node_count INTEGER NOT NULL,
                file_count INTEGER NOT NULL,
                criticality REAL NOT NULL DEFAULT 0.0,
                path_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS flow_memberships (
                flow_id INTEGER NOT NULL,
                node_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (flow_id, node_id)
            );
            CREATE INDEX IF NOT EXISTS idx_flows_criticality ON flows(criticality DESC);
            CREATE INDEX IF NOT EXISTS idx_flows_entry ON flows(entry_point_id);
            CREATE INDEX IF NOT EXISTS idx_flow_memberships_node ON flow_memberships(node_id);
            "#,
        )?;
        Ok(())
    }

    fn migrate_v4(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS communities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                level INTEGER NOT NULL DEFAULT 0,
                parent_id INTEGER,
                cohesion REAL NOT NULL DEFAULT 0.0,
                size INTEGER NOT NULL DEFAULT 0,
                dominant_language TEXT,
                description TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )?;
        if !has_column(&self.conn, "nodes", "community_id")? {
            self.conn
                .execute("ALTER TABLE nodes ADD COLUMN community_id INTEGER", [])?;
        }
        self.conn.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_nodes_community ON nodes(community_id);
            CREATE INDEX IF NOT EXISTS idx_communities_parent ON communities(parent_id);
            CREATE INDEX IF NOT EXISTS idx_communities_cohesion ON communities(cohesion DESC);
            "#,
        )?;
        Ok(())
    }

    fn migrate_v5(&self) -> Result<()> {
        if !table_exists(&self.conn, "nodes_fts")? {
            self.conn.execute(
                r#"
                CREATE VIRTUAL TABLE nodes_fts USING fts5(
                    name, qualified_name, file_path, signature, identifier_tokens, doc_text,
                    tokenize='porter unicode61'
                )
                "#,
                [],
            )?;
        }
        Ok(())
    }

    fn migrate_v6(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS community_summaries (
                community_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                purpose TEXT DEFAULT '',
                key_symbols TEXT DEFAULT '[]',
                risk TEXT DEFAULT 'unknown',
                size INTEGER DEFAULT 0,
                dominant_language TEXT DEFAULT '',
                FOREIGN KEY (community_id) REFERENCES communities(id)
            );
            CREATE TABLE IF NOT EXISTS flow_snapshots (
                flow_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                entry_point TEXT NOT NULL,
                critical_path TEXT DEFAULT '[]',
                criticality REAL DEFAULT 0.0,
                node_count INTEGER DEFAULT 0,
                file_count INTEGER DEFAULT 0,
                FOREIGN KEY (flow_id) REFERENCES flows(id)
            );
            CREATE TABLE IF NOT EXISTS risk_index (
                node_id INTEGER PRIMARY KEY,
                qualified_name TEXT NOT NULL,
                risk_score REAL DEFAULT 0.0,
                caller_count INTEGER DEFAULT 0,
                test_coverage TEXT DEFAULT 'unknown',
                security_relevant INTEGER DEFAULT 0,
                last_computed TEXT DEFAULT '',
                FOREIGN KEY (node_id) REFERENCES nodes(id)
            );
            CREATE INDEX IF NOT EXISTS idx_risk_index_score ON risk_index(risk_score DESC);
            "#,
        )?;
        Ok(())
    }

    fn migrate_v7(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target_qualified, kind);
            CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source_qualified, kind);
            "#,
        )?;
        Ok(())
    }

    fn migrate_v8(&self) -> Result<()> {
        self.conn.execute(
            r#"
            CREATE INDEX IF NOT EXISTS idx_edges_composite
            ON edges(kind, source_qualified, target_qualified, file_path, line)
            "#,
            [],
        )?;
        Ok(())
    }

    fn migrate_v9(&self) -> Result<()> {
        if !has_column(&self.conn, "edges", "confidence")? {
            self.conn.execute(
                "ALTER TABLE edges ADD COLUMN confidence REAL DEFAULT 1.0",
                [],
            )?;
        }
        if !has_column(&self.conn, "edges", "confidence_tier")? {
            self.conn.execute(
                "ALTER TABLE edges ADD COLUMN confidence_tier TEXT DEFAULT 'EXTRACTED'",
                [],
            )?;
        }
        Ok(())
    }

    fn migrate_v10(&self) -> Result<()> {
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_parent_name ON nodes(parent_name, name)",
            [],
        )?;
        Ok(())
    }

    fn migrate_v11(&self) -> Result<()> {
        if !has_column(&self.conn, "nodes", "mtime_ns")? {
            self.conn.execute(
                "ALTER TABLE nodes ADD COLUMN mtime_ns INTEGER DEFAULT 0",
                [],
            )?;
        }
        Ok(())
    }

    fn migrate_v12(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM edges WHERE kind='CROSS_ARTIFACT'", [])?;
        Ok(())
    }

    fn migrate_v13(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            DROP TABLE IF EXISTS nodes_fts;
            CREATE VIRTUAL TABLE nodes_fts USING fts5(
                name, qualified_name, file_path, signature, identifier_tokens, doc_text,
                tokenize='porter unicode61'
            );
            INSERT INTO nodes_fts(rowid, name, qualified_name, file_path, signature,
                                  identifier_tokens, doc_text)
            SELECT rowid, name, qualified_name, file_path, COALESCE(signature, ''), '', ''
            FROM nodes;
            "#,
        )?;
        Ok(())
    }

    fn migrate_v14(&self) -> Result<()> {
        self.conn.execute_batch(CENTRALITY_SCORE_SCHEMA_SQL)?;
        Ok(())
    }

    fn get_edges_by_endpoint(&self, column: &str, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        let mut seen = std::collections::HashSet::<i64>::new();
        let mut edges = Vec::new();
        let sql = format!("SELECT * FROM edges WHERE {column} = ?");
        for key in self.qualified_key_candidates(qualified_name)? {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([key], edge_from_row)?;
            for row in rows {
                let edge = row?;
                if seen.insert(edge.id) {
                    edges.push(edge);
                }
            }
        }
        Ok(edges)
    }

    fn file_key_candidates(&self, file_path: &str) -> Result<Vec<String>> {
        let normalized = self.normalize_file_path_key(file_path)?;
        if normalized == file_path {
            Ok(vec![file_path.to_string()])
        } else {
            Ok(vec![file_path.to_string(), normalized])
        }
    }

    fn qualified_key_candidates(&self, qualified_name: &str) -> Result<Vec<String>> {
        let normalized = self.normalize_qualified_key(qualified_name)?;
        if normalized == qualified_name {
            Ok(vec![qualified_name.to_string()])
        } else {
            Ok(vec![qualified_name.to_string(), normalized])
        }
    }

    fn normalize_qualified_key(&self, qualified_name: &str) -> Result<String> {
        if let Some((file_path, rest)) = qualified_name.split_once("::") {
            Ok(format!(
                "{}::{rest}",
                self.normalize_file_path_key(file_path)?
            ))
        } else {
            self.normalize_file_path_key(qualified_name)
        }
    }

    fn normalize_file_path_key(&self, file_path: &str) -> Result<String> {
        let path = Path::new(file_path);
        if !path.is_absolute() {
            return Ok(file_path.to_string());
        }
        let Some(repo_root) = self.get_metadata("repo_root")? else {
            return Ok(file_path.to_string());
        };
        let repo_root = Path::new(&repo_root);
        if let Ok(rel) = path.strip_prefix(repo_root) {
            return Ok(rel.to_string_lossy().to_string());
        }
        if let (Ok(path), Ok(repo_root)) = (path.canonicalize(), repo_root.canonicalize()) {
            if let Ok(rel) = path.strip_prefix(repo_root) {
                return Ok(rel.to_string_lossy().to_string());
            }
        }
        Ok(file_path.to_string())
    }
}

fn now_seconds() -> Result<f64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| GraphError::Clock)?;
    Ok(duration.as_secs_f64())
}

fn make_qualified_parts(
    kind: &str,
    name: &str,
    file_path: &str,
    parent_name: Option<&str>,
) -> String {
    if kind == "File" {
        file_path.to_string()
    } else if let Some(parent) = parent_name {
        format!("{file_path}::{parent}.{name}")
    } else {
        format!("{file_path}::{name}")
    }
}

fn remove_file_data_tx(tx: &Transaction<'_>, file_path: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM risk_index WHERE node_id IN (SELECT id FROM nodes WHERE file_path = ?)",
        [file_path],
    )?;
    tx.execute("DELETE FROM edges WHERE file_path = ?", [file_path])?;
    tx.execute("DELETE FROM nodes WHERE file_path = ?", [file_path])?;
    Ok(())
}

fn remove_files_data_tx(tx: &Transaction<'_>, file_paths: &[String]) -> Result<()> {
    tx.execute("DELETE FROM hub_scores", [])?;
    tx.execute("DELETE FROM bridge_scores", [])?;
    for chunk in file_paths.chunks(450) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let risk_sql = format!(
            "DELETE FROM risk_index \
             WHERE node_id IN (SELECT id FROM nodes WHERE file_path IN ({placeholders}))"
        );
        tx.execute(&risk_sql, rusqlite::params_from_iter(chunk))?;
        let edges_sql = format!("DELETE FROM edges WHERE file_path IN ({placeholders})");
        tx.execute(&edges_sql, rusqlite::params_from_iter(chunk))?;
        let nodes_sql = format!("DELETE FROM nodes WHERE file_path IN ({placeholders})");
        tx.execute(&nodes_sql, rusqlite::params_from_iter(chunk))?;
    }
    Ok(())
}

fn betweenness_centrality(
    graph_nodes: &HashSet<String>,
    adjacency: &HashMap<String, Vec<String>>,
) -> HashMap<String, f64> {
    let mut nodes = graph_nodes.iter().cloned().collect::<Vec<_>>();
    nodes.sort();
    let node_count = nodes.len();
    if node_count == 0 {
        return HashMap::new();
    }
    let sources = if node_count > 5000 {
        nodes.iter().take(500).cloned().collect::<Vec<_>>()
    } else {
        nodes.clone()
    };
    let scale = if node_count > 5000 {
        node_count as f64 / sources.len() as f64
    } else {
        1.0
    };

    let mut centrality = nodes
        .iter()
        .map(|node| (node.clone(), 0.0_f64))
        .collect::<HashMap<_, _>>();

    for source in sources {
        let mut stack = Vec::<String>::new();
        let mut predecessors = nodes
            .iter()
            .map(|node| (node.clone(), Vec::<String>::new()))
            .collect::<HashMap<_, _>>();
        let mut sigma = nodes
            .iter()
            .map(|node| (node.clone(), 0.0_f64))
            .collect::<HashMap<_, _>>();
        let mut distance = nodes
            .iter()
            .map(|node| (node.clone(), -1_i64))
            .collect::<HashMap<_, _>>();
        sigma.insert(source.clone(), 1.0);
        distance.insert(source.clone(), 0);

        let mut queue = VecDeque::from([source.clone()]);
        while let Some(vertex) = queue.pop_front() {
            stack.push(vertex.clone());
            let vertex_distance = *distance.get(&vertex).unwrap_or(&-1);
            let vertex_sigma = *sigma.get(&vertex).unwrap_or(&0.0);
            for successor in adjacency.get(&vertex).into_iter().flatten() {
                if !distance.contains_key(successor) {
                    continue;
                }
                if *distance.get(successor).unwrap_or(&-1) < 0 {
                    queue.push_back(successor.clone());
                    distance.insert(successor.clone(), vertex_distance + 1);
                }
                if *distance.get(successor).unwrap_or(&-1) == vertex_distance + 1 {
                    *sigma.entry(successor.clone()).or_insert(0.0) += vertex_sigma;
                    predecessors
                        .entry(successor.clone())
                        .or_default()
                        .push(vertex.clone());
                }
            }
        }

        let mut dependency = nodes
            .iter()
            .map(|node| (node.clone(), 0.0_f64))
            .collect::<HashMap<_, _>>();
        while let Some(w) = stack.pop() {
            let sigma_w = *sigma.get(&w).unwrap_or(&0.0);
            if sigma_w != 0.0 {
                for v in predecessors.get(&w).into_iter().flatten() {
                    let sigma_v = *sigma.get(v).unwrap_or(&0.0);
                    let delta_w = *dependency.get(&w).unwrap_or(&0.0);
                    *dependency.entry(v.clone()).or_insert(0.0) +=
                        (sigma_v / sigma_w) * (1.0 + delta_w);
                }
            }
            if w != source {
                *centrality.entry(w.clone()).or_insert(0.0) +=
                    *dependency.get(&w).unwrap_or(&0.0) * scale;
            }
        }
    }

    if node_count > 2 {
        let norm = 1.0 / ((node_count as f64 - 1.0) * (node_count as f64 - 2.0));
        for value in centrality.values_mut() {
            *value *= norm;
        }
    }

    centrality
}

fn extra_json(value: &Value) -> Result<String> {
    if value.is_null() || value.as_object().is_some_and(|object| object.is_empty()) {
        Ok("{}".to_string())
    } else {
        Ok(serde_json::to_string(value)?)
    }
}

fn store_flows_tx(tx: &Transaction<'_>, flows: &[FlowInput]) -> Result<()> {
    let mut insert_flow = tx.prepare(
        "INSERT INTO flows \
         (name, entry_point_id, depth, node_count, file_count, criticality, path_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )?;
    let mut insert_membership = tx.prepare(
        "INSERT OR IGNORE INTO flow_memberships (flow_id, node_id, position) \
         VALUES (?, ?, ?)",
    )?;
    for flow in flows {
        insert_flow.execute(params![
            flow.name,
            flow.entry_point_id,
            flow.depth,
            flow.node_count,
            flow.file_count,
            flow.criticality,
            serde_json::to_string(&flow.path)?,
        ])?;
        let flow_id = tx.last_insert_rowid();
        for (position, node_id) in flow.path.iter().enumerate() {
            insert_membership.execute(params![flow_id, node_id, position as i64])?;
        }
    }
    Ok(())
}

fn flow_json_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let path_json: String = row.get("path_json")?;
    let path = serde_json::from_str::<Vec<i64>>(&path_json).unwrap_or_default();
    let name: String = row.get("name")?;
    flow_json_value_from_parts(row, &name, &path)
}

struct FlowValue {
    value: Value,
    path_ids: Vec<i64>,
}

fn flow_value_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FlowValue> {
    let path_json: String = row.get("path_json")?;
    let path_ids = serde_json::from_str::<Vec<i64>>(&path_json).unwrap_or_default();
    let name: String = row.get("name")?;
    let value = flow_json_value_from_parts(row, &name, &path_ids)?;
    Ok(FlowValue { value, path_ids })
}

fn flow_json_value_from_parts(
    row: &rusqlite::Row<'_>,
    name: &str,
    path: &[i64],
) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, i64>("id")?,
        "name": sanitize_name(name),
        "entry_point_id": row.get::<_, i64>("entry_point_id")?,
        "depth": row.get::<_, i64>("depth")?,
        "node_count": row.get::<_, i64>("node_count")?,
        "file_count": row.get::<_, i64>("file_count")?,
        "criticality": row.get::<_, f64>("criticality")?,
        "path": path,
        "created_at": row.get::<_, String>("created_at")?,
        "updated_at": row.get::<_, String>("updated_at")?,
    }))
}

fn flow_steps_from_nodes(path_ids: &[i64], nodes_by_id: &HashMap<i64, GraphNode>) -> Vec<Value> {
    let mut steps = Vec::new();
    for node_id in path_ids {
        if let Some(node) = nodes_by_id.get(node_id) {
            steps.push(json!({
                "node_id": node.id,
                "name": sanitize_name(&node.name),
                "kind": node.kind,
                "file": node.file_path,
                "line_start": node.line_start,
                "line_end": node.line_end,
                "qualified_name": sanitize_name(&node.qualified_name),
            }));
        }
    }
    steps
}

fn community_json_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let name: String = row.get("name")?;
    let description = row
        .get::<_, Option<String>>("description")?
        .unwrap_or_default();
    Ok(json!({
        "id": row.get::<_, i64>("id")?,
        "name": sanitize_name(&name),
        "level": row.get::<_, i64>("level")?,
        "cohesion": row.get::<_, f64>("cohesion")?,
        "size": row.get::<_, i64>("size")?,
        "dominant_language": row.get::<_, Option<String>>("dominant_language")?.unwrap_or_default(),
        "description": sanitize_name(&description),
        "members": [],
    }))
}

fn store_file_batch_tx(
    tx: &Transaction<'_>,
    batch: &[FileBatchItem],
    suspend_indexes: bool,
) -> Result<()> {
    let now = now_seconds()?;
    let suspend_indexes = suspend_indexes && should_suspend_write_indexes(tx, batch.len())?;
    if suspend_indexes {
        drop_graph_write_indexes(tx)?;
    }
    let file_paths = batch
        .iter()
        .map(|(file_path, _, _, _, _)| file_path.clone())
        .collect::<Vec<_>>();
    remove_files_data_tx(tx, &file_paths)?;

    let mut seen_edges = HashSet::new();
    let mut node_params =
        Vec::<SqlValue>::with_capacity(NODE_INSERT_ROWS * NODE_INSERT_PARAM_COUNT);
    let mut node_rows = 0_usize;
    let mut edge_params =
        Vec::<SqlValue>::with_capacity(EDGE_INSERT_ROWS * EDGE_INSERT_PARAM_COUNT);
    let mut edge_rows = 0_usize;

    for (_file_path, nodes, edges, file_hash, mtime_ns) in batch {
        for node in nodes {
            let qualified = make_qualified_parts(
                &node.kind,
                &node.name,
                &node.file_path,
                node.parent_name.as_deref(),
            );
            let extra = extra_json(&node.extra)?;
            push_text(&mut node_params, &node.kind);
            push_text(&mut node_params, &node.name);
            node_params.push(SqlValue::Text(qualified));
            push_text(&mut node_params, &node.file_path);
            node_params.push(SqlValue::Integer(node.line_start));
            node_params.push(SqlValue::Integer(node.line_end));
            push_text(&mut node_params, &node.language);
            push_optional_text(&mut node_params, node.parent_name.as_deref());
            push_optional_text(&mut node_params, node.params.as_deref());
            push_optional_text(&mut node_params, node.return_type.as_deref());
            push_optional_text(&mut node_params, node.modifiers.as_deref());
            node_params.push(SqlValue::Integer(i64::from(node.is_test)));
            push_text(&mut node_params, file_hash);
            node_params.push(SqlValue::Integer(*mtime_ns));
            node_params.push(SqlValue::Text(extra));
            node_params.push(SqlValue::Real(now));
            node_rows += 1;
            if node_rows == NODE_INSERT_ROWS {
                insert_compact_node_rows(tx, node_rows, &node_params)?;
                node_params.clear();
                node_rows = 0;
            }
        }

        for edge in edges {
            let key = (
                edge.kind.as_str(),
                edge.source.as_str(),
                edge.target.as_str(),
                edge.file_path.as_str(),
                edge.line,
            );
            if !seen_edges.insert(key) {
                continue;
            }
            let confidence = edge
                .extra
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let confidence_tier = edge
                .extra
                .get("confidence_tier")
                .and_then(Value::as_str)
                .unwrap_or("EXTRACTED");
            let extra_json = extra_json(&edge.extra)?;
            push_text(&mut edge_params, &edge.kind);
            push_text(&mut edge_params, &edge.source);
            push_text(&mut edge_params, &edge.target);
            push_text(&mut edge_params, &edge.file_path);
            edge_params.push(SqlValue::Integer(edge.line));
            edge_params.push(SqlValue::Text(extra_json));
            edge_params.push(SqlValue::Real(confidence));
            push_text(&mut edge_params, confidence_tier);
            edge_params.push(SqlValue::Real(now));
            edge_rows += 1;
            if edge_rows == EDGE_INSERT_ROWS {
                insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
                edge_params.clear();
                edge_rows = 0;
            }
        }
    }
    if node_rows > 0 {
        insert_compact_node_rows(tx, node_rows, &node_params)?;
    }
    if edge_rows > 0 {
        insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
    }
    if suspend_indexes {
        create_graph_write_indexes(tx)?;
    }
    Ok(())
}

fn store_raw_compact_file_batch_tx(
    tx: &Transaction<'_>,
    batch: &[RawCompactFileBatchItem],
    suspend_indexes: bool,
) -> Result<()> {
    let now = now_seconds()?;
    let suspend_indexes = suspend_indexes && should_suspend_write_indexes(tx, batch.len())?;
    if suspend_indexes {
        drop_graph_write_indexes(tx)?;
    }
    let file_paths = batch
        .iter()
        .map(|(file_path, _, _, _, _)| file_path.clone())
        .collect::<Vec<_>>();
    remove_files_data_tx(tx, &file_paths)?;

    let mut seen_edges = HashSet::new();
    let mut node_params =
        Vec::<SqlValue>::with_capacity(NODE_INSERT_ROWS * NODE_INSERT_PARAM_COUNT);
    let mut node_rows = 0_usize;
    let mut edge_params =
        Vec::<SqlValue>::with_capacity(EDGE_INSERT_ROWS * EDGE_INSERT_PARAM_COUNT);
    let mut edge_rows = 0_usize;

    for (_file_path, nodes, edges, file_hash, mtime_ns) in batch {
        for node in nodes {
            let RawCompactNodeInput(
                kind,
                name,
                file_path,
                line_start,
                line_end,
                language,
                parent_name,
                params,
                return_type,
                modifiers,
                is_test,
                extra,
            ) = node;
            let qualified = make_qualified_parts(kind, name, file_path, parent_name.as_deref());
            push_text(&mut node_params, kind);
            push_text(&mut node_params, name);
            node_params.push(SqlValue::Text(qualified));
            push_text(&mut node_params, file_path);
            node_params.push(SqlValue::Integer(*line_start));
            node_params.push(SqlValue::Integer(*line_end));
            push_text(&mut node_params, language);
            push_optional_text(&mut node_params, parent_name.as_deref());
            push_optional_text(&mut node_params, params.as_deref());
            push_optional_text(&mut node_params, return_type.as_deref());
            push_optional_text(&mut node_params, modifiers.as_deref());
            node_params.push(SqlValue::Integer(i64::from(*is_test)));
            push_text(&mut node_params, file_hash);
            node_params.push(SqlValue::Integer(*mtime_ns));
            node_params.push(SqlValue::Text(extra.get().to_string()));
            node_params.push(SqlValue::Real(now));
            node_rows += 1;
            if node_rows == NODE_INSERT_ROWS {
                insert_compact_node_rows(tx, node_rows, &node_params)?;
                node_params.clear();
                node_rows = 0;
            }
        }

        for edge in edges {
            let RawCompactEdgeInput(kind, source, target, file_path, line, extra) = edge;
            let key = (
                kind.as_str(),
                source.as_str(),
                target.as_str(),
                file_path.as_str(),
                *line,
            );
            if !seen_edges.insert(key) {
                continue;
            }
            let (confidence, confidence_tier) = edge_metadata_from_raw_extra(extra.get())?;
            push_text(&mut edge_params, kind);
            push_text(&mut edge_params, source);
            push_text(&mut edge_params, target);
            push_text(&mut edge_params, file_path);
            edge_params.push(SqlValue::Integer(*line));
            edge_params.push(SqlValue::Text(extra.get().to_string()));
            edge_params.push(SqlValue::Real(confidence));
            edge_params.push(SqlValue::Text(confidence_tier));
            edge_params.push(SqlValue::Real(now));
            edge_rows += 1;
            if edge_rows == EDGE_INSERT_ROWS {
                insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
                edge_params.clear();
                edge_rows = 0;
            }
        }
    }
    if node_rows > 0 {
        insert_compact_node_rows(tx, node_rows, &node_params)?;
    }
    if edge_rows > 0 {
        insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
    }
    if suspend_indexes {
        create_graph_write_indexes(tx)?;
    }
    Ok(())
}

fn should_suspend_write_indexes(tx: &Transaction<'_>, file_count: usize) -> Result<bool> {
    if file_count < SUSPEND_INDEX_FILE_THRESHOLD {
        return Ok(false);
    }
    let has_nodes: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes LIMIT 1)", [], |row| {
        row.get(0)
    })?;
    if has_nodes != 0 {
        return Ok(false);
    }
    let has_edges: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM edges LIMIT 1)", [], |row| {
        row.get(0)
    })?;
    Ok(has_edges == 0)
}

fn drop_graph_write_indexes(tx: &Transaction<'_>) -> Result<()> {
    for (name, _) in WRITE_INDEXES {
        tx.execute(&format!("DROP INDEX IF EXISTS {name}"), [])?;
    }
    Ok(())
}

fn create_graph_write_indexes(tx: &Transaction<'_>) -> Result<()> {
    for (_, sql) in WRITE_INDEXES {
        tx.execute(sql, [])?;
    }
    Ok(())
}

fn edge_metadata_from_raw_extra(raw: &str) -> Result<(f64, String)> {
    if raw == "{}" {
        return Ok((1.0, "EXTRACTED".to_string()));
    }
    let extra: Value = serde_json::from_str(raw)?;
    let confidence = extra
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let confidence_tier = extra
        .get("confidence_tier")
        .and_then(Value::as_str)
        .unwrap_or("EXTRACTED")
        .to_string();
    Ok((confidence, confidence_tier))
}

fn push_text(params: &mut Vec<SqlValue>, value: &str) {
    params.push(SqlValue::Text(value.to_string()));
}

fn push_optional_text(params: &mut Vec<SqlValue>, value: Option<&str>) {
    match value {
        Some(value) => params.push(SqlValue::Text(value.to_string())),
        None => params.push(SqlValue::Null),
    }
}

fn insert_compact_node_rows(tx: &Transaction<'_>, rows: usize, values: &[SqlValue]) -> Result<()> {
    let sql = format!(
        r#"
        INSERT INTO nodes
            (kind, name, qualified_name, file_path, line_start, line_end,
             language, parent_name, params, return_type, modifiers, is_test,
             file_hash, mtime_ns, extra, updated_at)
        VALUES {}
        ON CONFLICT(qualified_name) DO UPDATE SET
            kind=excluded.kind, name=excluded.name,
            file_path=excluded.file_path, line_start=excluded.line_start,
            line_end=excluded.line_end, language=excluded.language,
            parent_name=excluded.parent_name, params=excluded.params,
            return_type=excluded.return_type, modifiers=excluded.modifiers,
            is_test=excluded.is_test, file_hash=excluded.file_hash,
            mtime_ns=excluded.mtime_ns, extra=excluded.extra, updated_at=excluded.updated_at
        "#,
        value_placeholders(NODE_INSERT_PARAM_COUNT, rows)
    );
    tx.execute(&sql, rusqlite::params_from_iter(values.iter()))?;
    Ok(())
}

fn insert_compact_edge_rows(tx: &Transaction<'_>, rows: usize, values: &[SqlValue]) -> Result<()> {
    let sql = format!(
        r#"
        INSERT INTO edges
            (kind, source_qualified, target_qualified, file_path, line, extra,
             confidence, confidence_tier, updated_at)
        VALUES {}
        "#,
        value_placeholders(EDGE_INSERT_PARAM_COUNT, rows)
    );
    tx.execute(&sql, rusqlite::params_from_iter(values.iter()))?;
    Ok(())
}

fn value_placeholders(width: usize, rows: usize) -> String {
    let row = format!(
        "({})",
        std::iter::repeat_n("?", width)
            .collect::<Vec<_>>()
            .join(",")
    );
    std::iter::repeat_n(row, rows).collect::<Vec<_>>().join(",")
}

fn node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphNode> {
    let extra: Option<String> = row.get("extra")?;
    Ok(GraphNode {
        id: row.get("id")?,
        kind: row.get("kind")?,
        name: row.get("name")?,
        qualified_name: row.get("qualified_name")?,
        file_path: row.get("file_path")?,
        line_start: row.get("line_start")?,
        line_end: row.get("line_end")?,
        language: row
            .get::<_, Option<String>>("language")?
            .unwrap_or_default(),
        parent_name: row.get("parent_name")?,
        params: row.get("params")?,
        return_type: row.get("return_type")?,
        is_test: row.get::<_, i64>("is_test")? != 0,
        file_hash: row.get("file_hash")?,
        extra: parse_json_column(extra).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?,
    })
}

fn edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphEdge> {
    let extra: Option<String> = row.get("extra")?;
    Ok(GraphEdge {
        id: row.get("id")?,
        kind: row.get("kind")?,
        source_qualified: row.get("source_qualified")?,
        target_qualified: row.get("target_qualified")?,
        file_path: row.get("file_path")?,
        line: row.get("line")?,
        extra: parse_json_column(extra).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?,
        confidence: row.get::<_, Option<f64>>("confidence")?.unwrap_or(1.0),
        confidence_tier: row
            .get::<_, Option<String>>("confidence_tier")?
            .unwrap_or_else(|| "EXTRACTED".to_string()),
    })
}

fn node_to_value(node: &GraphNode) -> Value {
    json!({
        "id": node.id,
        "kind": node.kind,
        "name": sanitize_name(&node.name),
        "qualified_name": sanitize_name(&node.qualified_name),
        "file_path": node.file_path,
        "line_start": node.line_start,
        "line_end": node.line_end,
        "language": node.language,
        "parent_name": node.parent_name.as_deref().map(sanitize_name),
        "is_test": node.is_test,
    })
}

fn parse_json_column(raw: Option<String>) -> serde_json::Result<Value> {
    match raw {
        Some(raw) if !raw.is_empty() => serde_json::from_str(&raw),
        _ => Ok(Value::Object(Default::default())),
    }
}

fn identifier_search_text<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let mut tokens = Vec::new();
    for value in values {
        let mut chunk = String::new();
        for ch in value.chars() {
            if ch.is_ascii_alphanumeric() {
                chunk.push(ch);
            } else if !chunk.is_empty() {
                push_identifier_parts(&chunk, &mut tokens);
                chunk.clear();
            }
        }
        if !chunk.is_empty() {
            push_identifier_parts(&chunk, &mut tokens);
        }
    }
    tokens.join(" ")
}

fn push_identifier_parts(chunk: &str, tokens: &mut Vec<String>) {
    let chars = chunk.chars().collect::<Vec<_>>();
    let mut start = 0;
    for idx in 1..chars.len() {
        let prev = chars[idx - 1];
        let current = chars[idx];
        let next = chars.get(idx + 1).copied();
        let lower_to_upper =
            (prev.is_ascii_lowercase() || prev.is_ascii_digit()) && current.is_ascii_uppercase();
        let acronym_boundary = prev.is_ascii_uppercase()
            && current.is_ascii_uppercase()
            && next.is_some_and(|ch| ch.is_ascii_lowercase());
        if lower_to_upper || acronym_boundary {
            tokens.push(
                chars[start..idx]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase(),
            );
            start = idx;
        }
    }
    if start < chars.len() {
        tokens.push(
            chars[start..]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase(),
        );
    }
}

fn read_node_source_excerpt(
    repo_root: Option<&Path>,
    kind: &str,
    file_path: &str,
    line_start: Option<i64>,
    line_end: Option<i64>,
) -> String {
    let mut path = PathBuf::from(file_path);
    if !path.is_absolute() {
        let Some(root) = repo_root else {
            return String::new();
        };
        path = root.join(path);
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let lines = text.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return String::new();
    }
    let start = line_start.unwrap_or(1).saturating_sub(1).max(0) as usize;
    let mut end = line_end
        .unwrap_or(line_start.unwrap_or(1))
        .max(line_start.unwrap_or(1)) as usize;
    let start = start.min(lines.len().saturating_sub(1));
    end = end.min(lines.len());
    if kind == "DocSection" {
        let level = markdown_heading_level(lines[start]);
        end = lines.len();
        for (idx, line) in lines.iter().enumerate().skip(start + 1) {
            if let Some(candidate_level) = markdown_heading_level(line) {
                if level.is_none_or(|current_level| candidate_level <= current_level) {
                    end = idx;
                    break;
                }
            }
        }
    }
    lines[start..end].join("\n").chars().take(4096).collect()
}

fn markdown_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&level) && trimmed.chars().nth(level).is_some_and(|ch| ch == ' ') {
        Some(level)
    } else {
        None
    }
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?",
        [table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn common_prefix(values: &[String]) -> String {
    let Some((first, rest)) = values.split_first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for value in rest {
        while !value.starts_with(&prefix) {
            if prefix.pop().is_none() {
                return String::new();
            }
        }
    }
    prefix
}

fn community_purpose(paths: &[String]) -> String {
    let prefix = common_prefix(paths);
    if !prefix.contains('/') {
        return String::new();
    }
    prefix
        .rsplit_once('/')
        .map(|(before_last, _)| before_last.rsplit('/').next().unwrap_or(""))
        .unwrap_or("")
        .to_string()
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| *ch == '\t' || *ch == '\n' || (*ch as u32) >= 0x20)
        .take(256)
        .collect()
}

#[cfg(test)]
mod tests;
