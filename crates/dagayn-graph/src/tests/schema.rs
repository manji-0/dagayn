use super::*;

#[test]
fn creates_current_schema() {
    let path = temp_db("schema");
    let store = GraphStore::open(&path).expect("open graph store");
    assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
    assert!(table_exists(&store.conn, "nodes_fts").unwrap());
    assert!(has_column(&store.conn, "edges", "confidence_tier").unwrap());
    assert!(has_column(&store.conn, "edges", "target_name").unwrap());
    // v18 dropped the stored flows.
    for table in ["flows", "flow_memberships", "flow_snapshots"] {
        assert!(!table_exists(&store.conn, table).unwrap(), "{table}");
    }
    // v19 dropped the centrality scores.
    for table in CENTRALITY_TABLES {
        assert!(!table_exists(&store.conn, table).unwrap(), "{table}");
    }
    // v20 dropped the community summaries; the communities stay.
    assert!(!table_exists(&store.conn, "community_summaries").unwrap());
    assert!(table_exists(&store.conn, "communities").unwrap());
    let _ = std::fs::remove_file(path);
}

const CENTRALITY_TABLES: [&str; 4] = [
    "hub_scores",
    "bridge_scores",
    "hub_scores_code",
    "bridge_scores_code",
];

#[test]
fn migrate_v19_drops_the_centrality_tables_of_an_existing_db() {
    let path = temp_db("migrate-v19");
    {
        let conn = rusqlite::Connection::open(&path).expect("open sqlite db");
        conn.execute_batch(SCHEMA_SQL).unwrap();
        for table in CENTRALITY_TABLES {
            conn.execute_batch(&format!(
                "CREATE TABLE {table} (qualified_name TEXT PRIMARY KEY); \
                 INSERT INTO {table} VALUES ('app.py::main');"
            ))
            .unwrap();
        }
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('schema_version', '18')",
            [],
        )
        .unwrap();
    }

    let store = GraphStore::open(&path).expect("open graph store");

    assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
    for table in CENTRALITY_TABLES {
        assert!(!table_exists(&store.conn, table).unwrap(), "{table}");
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn migrate_v20_drops_the_community_summaries_of_an_existing_db() {
    let path = temp_db("migrate-v20");
    {
        let conn = rusqlite::Connection::open(&path).expect("open sqlite db");
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute_batch(
            "CREATE TABLE community_summaries (community_id INTEGER PRIMARY KEY, name TEXT); \
             INSERT INTO community_summaries VALUES (1, 'auth-cluster'); \
             INSERT OR REPLACE INTO metadata (key, value) VALUES ('schema_version', '19');",
        )
        .unwrap();
    }

    let store = GraphStore::open(&path).expect("open graph store");

    assert_eq!(store.schema_version().unwrap(), 20);
    assert!(!table_exists(&store.conn, "community_summaries").unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn ensure_edge_target_name_backfills_legacy_edges_without_column() {
    let path = temp_db("legacy-target-name");
    {
        let conn = rusqlite::Connection::open(&path).expect("open sqlite db");
        conn.execute_batch(
            r#"
            CREATE TABLE edges (
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
            CREATE TABLE metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO edges
                (kind, source_qualified, target_qualified, file_path, line, extra,
                 confidence, confidence_tier, updated_at)
            VALUES
                ('CALLS', 'app.py::main', 'app.py::helper', 'app.py', 2, '{}', 1.0, 'EXTRACTED', 1.0),
                ('CALLS', 'worker.py::run', 'helper', 'worker.py', 4, '{}', 1.0, 'EXTRACTED', 1.0);
            INSERT INTO metadata (key, value) VALUES ('schema_version', '14');
            "#,
        )
        .unwrap();
    }

    let store = GraphStore::open(&path).expect("open graph store");
    assert!(has_column(&store.conn, "edges", "target_name").unwrap());

    let rows = store
        .conn
        .prepare("SELECT target_qualified, target_name FROM edges ORDER BY id")
        .unwrap()
        .query_map([], |row| <(String, String)>::try_from(row))
        .unwrap()
        .map(|row| row.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            ("app.py::helper".to_string(), "helper".to_string()),
            ("helper".to_string(), "helper".to_string()),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn migration_v17_drops_covered_indexes() {
    const COVERED: [&str; 4] = [
        "idx_edges_source",
        "idx_edges_target",
        "idx_nodes_qualified",
        "idx_edges_composite",
    ];
    let index_exists = |store: &GraphStore, name: &str| -> bool {
        store
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?",
                [name],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some()
    };
    let path = temp_db("migration-v17");
    {
        let store = GraphStore::open(&path).expect("open graph store");
        for name in COVERED {
            assert!(!index_exists(&store, name), "{name} on a fresh graph");
        }
        store
            .conn
            .execute_batch(
                "CREATE INDEX idx_edges_source ON edges(source_qualified);
                 CREATE INDEX idx_edges_target ON edges(target_qualified);
                 CREATE INDEX idx_nodes_qualified ON nodes(qualified_name);
                 CREATE INDEX idx_edges_composite
                     ON edges(kind, source_qualified, target_qualified, file_path, line);",
            )
            .unwrap();
        store.set_metadata("schema_version", "16").unwrap();
    }
    let store = GraphStore::open(&path).expect("reopen graph store");
    assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
    for name in COVERED {
        assert!(!index_exists(&store, name), "{name} survived v17");
    }
    assert!(index_exists(&store, "idx_edges_source_kind"));
    assert!(index_exists(&store, "idx_edges_target_kind"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn reads_legacy_edges_with_default_confidence_metadata() {
    let path = temp_db("legacy-edge-meta");
    let store = GraphStore::open(&path).expect("open graph store");
    store
        .conn
        .execute(
            "INSERT INTO edges \
             (kind, source_qualified, target_qualified, file_path, line, extra, confidence, \
              confidence_tier, updated_at) \
             VALUES ('CALLS', 'app.py::caller', 'app.py::callee', 'app.py', 3, '{}', NULL, NULL, 1.0)",
            [],
        )
        .unwrap();

    let edge: GraphEdge = store
        .conn
        .query_row(
            "SELECT * FROM edges WHERE source_qualified = 'app.py::caller'",
            [],
            edge_from_row,
        )
        .unwrap();
    assert_eq!(edge.confidence, 1.0);
    assert_eq!(edge.confidence_tier, ConfidenceTier::Extracted);
    assert_eq!(
        edge_metadata_from_raw_extra(r#"{"confidence":0.33,"confidence_tier":"exact"}"#).unwrap(),
        (0.33, ConfidenceTier::Exact)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_graph_newer_than_this_build_is_refused_untouched() {
    let path = temp_db("newer-schema");
    {
        let store = GraphStore::open(&path).expect("open graph store");
        store
            .set_metadata("schema_version", &(LATEST_VERSION + 1).to_string())
            .unwrap();
        store.conn.execute_batch("DROP TABLE nodes").unwrap();
    }
    let err = GraphStore::open(&path).err().expect("newer graph refused");
    assert!(
        matches!(err, GraphError::NewerSchema { found, supported }
            if found == LATEST_VERSION + 1 && supported == LATEST_VERSION),
        "{err}"
    );
    let conn = rusqlite::Connection::open(&path).unwrap();
    let nodes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'nodes'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(nodes, 0, "the refused open recreated a dropped table");
    drop(conn);
    let _ = std::fs::remove_file(path);
}
