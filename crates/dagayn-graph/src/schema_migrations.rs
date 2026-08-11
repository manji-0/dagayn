use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub(crate) fn migrate_v2(&self) -> Result<()> {
        if !has_column(&self.conn, "nodes", "signature")? {
            self.conn
                .execute("ALTER TABLE nodes ADD COLUMN signature TEXT", [])?;
        }
        Ok(())
    }

    pub(crate) fn migrate_v3(&self) -> Result<()> {
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

    pub(crate) fn migrate_v4(&self) -> Result<()> {
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

    pub(crate) fn migrate_v5(&self) -> Result<()> {
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

    pub(crate) fn migrate_v6(&self) -> Result<()> {
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

    pub(crate) fn migrate_v7(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target_qualified, kind);
            CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source_qualified, kind);
            "#,
        )?;
        Ok(())
    }

    pub(crate) fn migrate_v8(&self) -> Result<()> {
        self.conn.execute(
            r#"
            CREATE INDEX IF NOT EXISTS idx_edges_composite
            ON edges(kind, source_qualified, target_qualified, file_path, line)
            "#,
            [],
        )?;
        Ok(())
    }

    pub(crate) fn migrate_v9(&self) -> Result<()> {
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

    pub(crate) fn migrate_v10(&self) -> Result<()> {
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_parent_name ON nodes(parent_name, name)",
            [],
        )?;
        Ok(())
    }

    pub(crate) fn migrate_v11(&self) -> Result<()> {
        if !has_column(&self.conn, "nodes", "mtime_ns")? {
            self.conn.execute(
                "ALTER TABLE nodes ADD COLUMN mtime_ns INTEGER DEFAULT 0",
                [],
            )?;
        }
        Ok(())
    }

    pub(crate) fn migrate_v12(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM edges WHERE kind='CROSS_ARTIFACT'", [])?;
        Ok(())
    }

    pub(crate) fn migrate_v13(&self) -> Result<()> {
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

    pub(crate) fn migrate_v14(&self) -> Result<()> {
        self.conn.execute_batch(CENTRALITY_SCORE_SCHEMA_SQL)?;
        Ok(())
    }

    pub(crate) fn migrate_v15(&self) -> Result<()> {
        let needs_rebuild = {
            let tx = self.conn.unchecked_transaction()?;
            let needs = crate::fts_sync::fts_needs_rebuild_tx(&tx)?;
            tx.commit()?;
            needs
        };
        if needs_rebuild {
            let repo_root = self.get_metadata("repo_root")?.map(PathBuf::from);
            crate::fts_sync::rebuild_fts_index_tx(&self.conn, repo_root.as_deref())?;
        }
        Ok(())
    }

    pub(crate) fn ensure_edge_target_name_column(&self) -> Result<()> {
        if !has_column(&self.conn, "edges", "target_name")? {
            self.conn.execute(
                "ALTER TABLE edges ADD COLUMN target_name TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, target_qualified FROM edges WHERE target_name = '' OR target_name IS NULL",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !rows.is_empty() {
            let mut update = self
                .conn
                .prepare("UPDATE edges SET target_name = ? WHERE id = ?")?;
            for (id, target_qualified) in rows {
                update.execute(params![edge_target_name(&target_qualified), id])?;
            }
        }
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_edges_target_name_kind ON edges(target_name, kind)",
            [],
        )?;
        Ok(())
    }
}
