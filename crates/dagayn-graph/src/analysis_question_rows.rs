use crate::helpers::*;
use crate::*;

impl GraphStore {
    /// SQL fragment matching the Python-side test-file heuristic in
    /// ``_is_analysis_excluded_from_test_gap``: nodes living in test files
    /// (or marked ``is_test`` / ``kind='Test'``) are excluded from review-facing
    /// hub / bridge rankings even when the ``is_test`` flag was not set at parse
    /// time.
    const EXCLUDE_TEST_NODES_SQL: &'static str = "COALESCE(n.is_test, 0) = 0 \
         AND n.kind != 'Test' \
         AND COALESCE(n.language, '') != 'markdown' \
         AND instr('/' || n.file_path || '/', '/tests/') = 0 \
         AND instr('/' || n.file_path || '/', '/test/') = 0 \
         AND instr('/' || n.file_path || '/', '/__tests__/') = 0 \
         AND instr('/' || n.file_path, '/test_') = 0 \
         AND instr(n.file_path, '.test.') = 0 \
         AND instr(n.file_path, '.spec.') = 0 \
         AND n.file_path NOT LIKE '%_test.py' \
         AND n.file_path NOT LIKE '%_tests.py' \
         AND n.file_path NOT LIKE '%_test.rs' \
         AND n.file_path NOT LIKE '%_tests.rs'";

    pub(crate) fn get_all_node_community_ids(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT qualified_name, community_id FROM nodes WHERE community_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(Into::into)
    }

    pub(crate) fn get_question_nodes(&self) -> Result<Vec<QuestionNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, name, qualified_name, file_path, language, is_test \
             FROM nodes WHERE kind != 'File'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(QuestionNode {
                kind: row.get(0)?,
                name: row.get(1)?,
                qualified_name: row.get(2)?,
                file_path: row.get(3)?,
                language: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                is_test: row.get::<_, i64>(5)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub(crate) fn get_question_edges(&self) -> Result<Vec<QuestionEdge>> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, source_qualified, target_qualified FROM edges")?;
        let rows = stmt.query_map([], |row| {
            Ok(QuestionEdge {
                kind: row.get(0)?,
                source_qualified: row.get(1)?,
                target_qualified: row.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub(crate) fn get_persisted_bridge_rows(&self, limit: i64) -> Result<Vec<PersistedBridgeRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT bs.name, bs.qualified_name FROM bridge_scores bs \
             JOIN nodes n ON n.qualified_name = bs.qualified_name \
             WHERE ({}) \
             ORDER BY bs.betweenness DESC, bs.qualified_name LIMIT ?",
            Self::EXCLUDE_TEST_NODES_SQL
        ))?;
        let rows = stmt.query_map([limit], |row| {
            Ok(PersistedBridgeRow {
                name: row.get(0)?,
                qualified_name: row.get(1)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub(crate) fn get_persisted_hub_rows(&self, limit: i64) -> Result<Vec<PersistedHubRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT hs.name, hs.qualified_name, hs.total_degree FROM hub_scores hs \
             JOIN nodes n ON n.qualified_name = hs.qualified_name \
             WHERE ({}) \
             ORDER BY hs.total_degree DESC, hs.qualified_name LIMIT ?",
            Self::EXCLUDE_TEST_NODES_SQL
        ))?;
        let rows = stmt.query_map([limit], |row| {
            Ok(PersistedHubRow {
                name: row.get(0)?,
                qualified_name: row.get(1)?,
                total_degree: row.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}
