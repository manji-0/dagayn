use crate::*;

impl GraphStore {
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

        let mut found_qualified_direct = false;
        for qn in &input_qns {
            for test_target in self.get_test_targets_for_source(qn)? {
                found_qualified_direct = true;
                if seen.insert(test_target.clone())
                    && let Some(test_node) = self.test_node_json(&test_target, false)?
                {
                    results.push(test_node);
                }
            }
        }

        if !found_qualified_direct {
            let bare = qualified_name
                .rsplit_once("::")
                .map(|(_, name)| name)
                .unwrap_or(qualified_name);
            for test_target in self.get_test_targets_for_source(bare)? {
                if seen.insert(test_target.clone())
                    && let Some(test_node) = self.test_node_json(&test_target, false)?
                {
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
                    if seen.insert(test_target.clone())
                        && let Some(test_node) = self.test_node_json(&test_target, true)?
                    {
                        results.push(test_node);
                    }
                }
            }
            frontier = next_frontier;
        }

        Ok(results)
    }
}
