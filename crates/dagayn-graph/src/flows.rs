use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn get_flow_edge_data(&self) -> Result<FlowEdgeData> {
        let mut calls_out: HashMap<String, Vec<String>> = HashMap::new();
        let mut has_tested_by: HashSet<String> = HashSet::new();

        // Prefetch node qualified names so reportable CROSS_ARTIFACT targets
        // can be validated without an N+1 lookup.
        let mut node_qns: HashSet<String> = HashSet::new();
        {
            let mut node_stmt = self.conn.prepare("SELECT qualified_name FROM nodes")?;
            let rows = node_stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                node_qns.insert(row?);
            }
        }

        // Include reportable CROSS_ARTIFACT hops so flow tracing can cross
        // artifact boundaries. Low-confidence / unresolved bridges stay out.
        let mut stmt = self.conn.prepare(
            "SELECT kind, source_qualified, target_qualified, confidence_tier, extra \
             FROM edges \
             WHERE kind IN ('CALLS', 'TESTED_BY', 'CROSS_ARTIFACT')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        for row in rows {
            let (kind, source, target, confidence_tier, extra_json) = row?;
            if kind == "CALLS" {
                calls_out.entry(source).or_default().push(target);
            } else if kind == "TESTED_BY" {
                has_tested_by.insert(source);
            } else if kind == "CROSS_ARTIFACT"
                && node_qns.contains(&target)
                && is_reportable_cross_artifact(
                    &target,
                    confidence_tier.as_deref(),
                    extra_json.as_deref(),
                )
            {
                calls_out.entry(source).or_default().push(target);
            }
        }
        Ok((calls_out, has_tested_by))
    }

    pub fn store_flows(&mut self, flows: &[FlowInput]) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
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
        let tx = write_tx(&mut self.conn)?;
        delete_flows_for_entry_point_ids(&tx, &flows)?;
        store_flows_tx(&tx, &flows)?;
        tx.commit()?;
        Ok(flows.len() as i64)
    }

    pub fn get_flows_json(&self, sort_by: &str, limit: i64) -> Result<String> {
        let sort_by = FlowSortBy::from_raw(sort_by);
        let sql = format!(
            "SELECT * FROM flows ORDER BY {} {} LIMIT ?",
            sort_by.column(),
            sort_by.order()
        );
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

    pub fn get_node_kind_by_id(&self, node_id: i64) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT kind FROM nodes WHERE id = ?", [node_id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }
}

fn extra_confidence_tier(extra_json: Option<&str>) -> Option<String> {
    let raw = extra_json?;
    let Value::Object(map) = serde_json::from_str::<Value>(raw).ok()? else {
        return None;
    };
    let tier = map.get("confidence_tier").and_then(Value::as_str)?;
    let tier = tier.trim();
    if tier.is_empty() {
        None
    } else {
        Some(tier.to_ascii_uppercase())
    }
}

/// Reportable when the target is resolved and the effective confidence tier is
/// EXACT/HIGH/EXTRACTED.
///
/// Prefer the normalized column tier (Python `is_reportable_bridge` /
/// `confidence_tier_of`). Only fall back to `extra.confidence_tier` when the
/// column is absent or empty — never let extra override a non-reportable
/// column value such as LOW/MEDIUM.
fn is_reportable_cross_artifact(
    target: &str,
    confidence_tier: Option<&str>,
    extra_json: Option<&str>,
) -> bool {
    if target.starts_with("<unresolved:") {
        return false;
    }
    let column_tier = confidence_tier
        .map(str::trim)
        .filter(|tier| !tier.is_empty())
        .map(|tier| tier.to_ascii_uppercase());
    let tier = match column_tier {
        Some(tier) => tier,
        None => match extra_confidence_tier(extra_json) {
            Some(tier) => tier,
            // Align with SQL COALESCE(confidence_tier, 'EXTRACTED') when both
            // column and extra are absent.
            None => "EXTRACTED".to_string(),
        },
    };
    matches!(tier.as_str(), "EXACT" | "HIGH" | "EXTRACTED")
}

#[cfg(test)]
mod reportability_tests {
    use super::is_reportable_cross_artifact;

    #[test]
    fn column_tier_wins_over_extra() {
        // Column LOW must not become reportable via extra HIGH.
        assert!(!is_reportable_cross_artifact(
            "native.py::main",
            Some("LOW"),
            Some(r#"{"confidence_tier":"HIGH"}"#),
        ));
        assert!(!is_reportable_cross_artifact(
            "native.py::main",
            Some("MEDIUM"),
            Some(r#"{"confidence_tier":"EXACT"}"#),
        ));
        assert!(is_reportable_cross_artifact(
            "native.py::main",
            Some("HIGH"),
            Some(r#"{"confidence_tier":"LOW"}"#),
        ));
    }

    #[test]
    fn empty_column_falls_back_to_extra() {
        assert!(is_reportable_cross_artifact(
            "native.py::main",
            None,
            Some(r#"{"confidence_tier":"HIGH"}"#),
        ));
        assert!(is_reportable_cross_artifact(
            "native.py::main",
            Some("  "),
            Some(r#"{"confidence_tier":"EXTRACTED"}"#),
        ));
        assert!(!is_reportable_cross_artifact(
            "native.py::main",
            None,
            Some(r#"{"confidence_tier":"LOW"}"#),
        ));
    }

    #[test]
    fn unresolved_targets_are_never_reportable() {
        assert!(!is_reportable_cross_artifact(
            "<unresolved:cli>",
            Some("HIGH"),
            Some(r#"{"confidence_tier":"HIGH"}"#),
        ));
    }
}
