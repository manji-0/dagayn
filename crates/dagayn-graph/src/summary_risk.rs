use crate::*;

impl GraphStore {
    pub(crate) fn compute_risk_index(&mut self) -> Result<()> {
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
}
