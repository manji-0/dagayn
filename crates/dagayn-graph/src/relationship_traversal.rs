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

    pub(crate) fn get_transitive_test_counts(
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
}
