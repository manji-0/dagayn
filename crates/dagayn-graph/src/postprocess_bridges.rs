use crate::helpers::*;
use crate::*;

const NODE_QUALIFIED_EDGE_KINDS: [&str; 6] = [
    "CALLS",
    "INHERITS",
    "IMPLEMENTS",
    "CONTAINS",
    "REFERENCES",
    "TESTED_BY",
];
const INFERRED_CONFIDENCE: f64 = 0.6;
const BARE_UNRESOLVED_CONFIDENCE: f64 = 0.3;

fn extra_json(extra: &Value) -> Result<String> {
    Ok(serde_json::to_string(extra)?)
}

fn looks_like_file_target(target: &str) -> bool {
    let path = target
        .split_once("::")
        .map(|(file, _)| file)
        .unwrap_or(target);
    if path.contains('/') || path.contains('\\') {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    [
        ".md",
        ".markdown",
        ".py",
        ".tf",
        ".tfvars",
        ".rs",
        ".js",
        ".ts",
        ".tsx",
        ".jsx",
        ".java",
        ".go",
        ".rb",
        ".php",
        ".cs",
        ".cpp",
        ".hpp",
        ".c",
        ".h",
        ".swift",
        ".kt",
        ".scala",
        ".dart",
        ".ipynb",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn node_file_from_qualified(qualified: &str, fallback: &str) -> String {
    qualified
        .split_once("::")
        .map(|(file, _)| file.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn terraform_module_matches_file(module: &str, file_path: &str) -> bool {
    if file_path.is_empty() {
        return false;
    }
    let path = file_path.replace('\\', "/");
    let stem = path.rsplit_once('/').map(|(_, name)| name).unwrap_or(&path);
    let stem = stem.rsplit_once('.').map(|(name, _)| name).unwrap_or(stem);
    if stem == module {
        return true;
    }
    path.split('/').rev().skip(1).any(|part| part == module)
}

fn import_targets_tx(tx: &Transaction<'_>) -> Result<HashMap<String, HashSet<String>>> {
    let mut import_targets: HashMap<String, HashSet<String>> = HashMap::new();
    let mut stmt = tx.prepare(
        "SELECT DISTINCT file_path, target_qualified FROM edges WHERE kind = 'IMPORTS_FROM'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (file_path, target) = row?;
        let target_file = node_file_from_qualified(&target, &target);
        import_targets
            .entry(file_path)
            .or_default()
            .insert(target_file);
    }
    Ok(import_targets)
}

fn is_plausible_bare_edge(
    source_file: &str,
    target_file: &str,
    import_targets: &HashMap<String, HashSet<String>>,
) -> bool {
    if source_file.is_empty() || target_file.is_empty() {
        return false;
    }
    source_file == target_file
        || import_targets
            .get(source_file)
            .is_some_and(|targets| targets.contains(target_file))
}

fn bare_name_candidates(
    tx: &Transaction<'_>,
    bare_name: &str,
    kinds: &[&str],
) -> Result<Vec<String>> {
    let placeholders = std::iter::repeat_n("?", kinds.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql =
        format!("SELECT qualified_name FROM nodes WHERE name = ? AND kind IN ({placeholders})");
    let mut params: Vec<SqlValue> = Vec::with_capacity(kinds.len() + 1);
    params.push(SqlValue::Text(bare_name.to_string()));
    params.extend(kinds.iter().map(|kind| SqlValue::Text((*kind).to_string())));
    let mut stmt = tx.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| row.get(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn resolve_via_imports(
    candidates: &[String],
    source_file: &str,
    import_targets: &HashMap<String, HashSet<String>>,
) -> Option<String> {
    let imported: Vec<&String> = candidates
        .iter()
        .filter(|qn| {
            is_plausible_bare_edge(
                source_file,
                &node_file_from_qualified(qn, ""),
                import_targets,
            )
        })
        .collect();
    match imported.as_slice() {
        [only] => Some((*only).clone()),
        _ => None,
    }
}

impl GraphStore {
    pub fn demote_unresolved_endpoint_edges(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        let placeholders = std::iter::repeat_n("?", NODE_QUALIFIED_EDGE_KINDS.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE edges
             SET confidence = MIN(confidence, 0.2),
                 confidence_tier = 'LOW'
             WHERE kind IN ({placeholders})
               AND UPPER(COALESCE(confidence_tier, 'EXTRACTED')) NOT IN ('LOW', 'UNKNOWN')
               AND (
                 target_qualified LIKE '<unresolved:%'
                 OR source_qualified LIKE '<unresolved:%'
                 OR NOT EXISTS (
                     SELECT 1 FROM nodes n WHERE n.qualified_name = edges.target_qualified
                 )
                 OR NOT EXISTS (
                     SELECT 1 FROM nodes n WHERE n.qualified_name = edges.source_qualified
                 )
               )"
        );
        let updated = tx.execute(&sql, rusqlite::params_from_iter(NODE_QUALIFIED_EDGE_KINDS))?;
        tx.commit()?;
        Ok(updated as i64)
    }

    pub fn resolve_terraform_artifact_refs(&mut self) -> Result<(i64, i64)> {
        let tx = write_tx(&mut self.conn)?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT id, target_qualified, extra FROM edges \
                 WHERE kind='CROSS_ARTIFACT' AND extra LIKE '%original_symbol_name%'",
            )?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut resolved = 0_i64;
        let mut still_unresolved = 0_i64;
        for (edge_id, current_target, extra_raw) in rows {
            let extra = parse_json_column(extra_raw)?;
            let obj = match extra.as_object() {
                Some(obj) => obj,
                None => continue,
            };
            if obj.get("source_language").and_then(Value::as_str) != Some("terraform") {
                continue;
            }
            if !matches!(
                obj.get("evidence_source").and_then(Value::as_str),
                Some("handler" | "entry_point")
            ) {
                continue;
            }
            if obj.get("relationship_role").and_then(Value::as_str) != Some("maps_entrypoint") {
                continue;
            }
            let Some(sym) = obj.get("original_symbol_name").and_then(Value::as_str) else {
                continue;
            };
            if sym.is_empty() {
                continue;
            }
            match terraform_entrypoint_match(&tx, sym)? {
                None => still_unresolved += 1,
                Some((qname, _)) if qname == current_target => {}
                Some((qname, lang)) => {
                    let mut new_extra = extra.clone();
                    if let Some(obj) = new_extra.as_object_mut() {
                        obj.insert("target_language".to_string(), Value::String(lang));
                        obj.insert("confidence".to_string(), Value::from(0.8));
                        obj.insert(
                            "confidence_tier".to_string(),
                            Value::String(ConfidenceTier::High.as_str().to_string()),
                        );
                    }
                    tx.execute(
                        "UPDATE edges
                         SET target_qualified=?, target_name=?, extra=?, confidence=?, confidence_tier=?
                         WHERE id=?",
                        params![
                            qname,
                            edge_target_name(&qname),
                            extra_json(&new_extra)?,
                            0.8,
                            ConfidenceTier::High.as_str(),
                            edge_id
                        ],
                    )?;
                    resolved += 1;
                }
            }
        }
        tx.commit()?;
        Ok((resolved, still_unresolved))
    }

    pub fn import_targets_by_file(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut import_targets: HashMap<String, Vec<String>> = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT file_path, target_qualified FROM edges WHERE kind = 'IMPORTS_FROM'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (file_path, target) = row?;
            let target_file = node_file_from_qualified(&target, &target);
            import_targets
                .entry(file_path)
                .or_default()
                .push(target_file);
        }
        Ok(import_targets)
    }

    pub fn resolve_bare_call_targets(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        let import_targets = import_targets_tx(&tx)?;
        let edges = {
            let mut stmt = tx.prepare(
                "SELECT id, source_qualified, target_qualified, file_path \
                 FROM edges WHERE kind = 'CALLS' AND target_qualified NOT LIKE '%::%'",
            )?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut resolved = 0_i64;
        for (edge_id, source_qualified, target_qualified, file_path) in edges {
            if looks_like_file_target(&target_qualified) {
                continue;
            }
            let candidates =
                bare_name_candidates(&tx, &target_qualified, &["Function", "Test", "Class"])?;
            if candidates.is_empty() {
                continue;
            }
            let src_file = node_file_from_qualified(&source_qualified, &file_path);
            let Some(qualified) = resolve_via_imports(&candidates, &src_file, &import_targets)
            else {
                continue;
            };
            tx.execute(
                "UPDATE edges SET target_qualified = ?, target_name = ?, \
                 confidence = ?, confidence_tier = ? WHERE id = ?",
                params![
                    qualified,
                    edge_target_name(&qualified),
                    INFERRED_CONFIDENCE,
                    ConfidenceTier::Medium.as_str(),
                    edge_id
                ],
            )?;
            resolved += 1;
        }
        tx.commit()?;
        Ok(resolved)
    }

    pub fn resolve_bare_inheritance_targets(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        let import_targets = import_targets_tx(&tx)?;
        let edges = {
            let mut stmt = tx.prepare(
                "SELECT id, source_qualified, target_qualified, file_path, extra \
                 FROM edges WHERE kind IN ('INHERITS', 'IMPLEMENTS') \
                 AND target_qualified NOT LIKE '%::%'",
            )?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut resolved = 0_i64;
        for (edge_id, source_qualified, target_qualified, file_path, extra_raw) in edges {
            if looks_like_file_target(&target_qualified) {
                continue;
            }
            let candidates = bare_name_candidates(&tx, &target_qualified, &["Class"])?;
            let src_file = node_file_from_qualified(&source_qualified, &file_path);
            if let Some(qualified) = resolve_via_imports(&candidates, &src_file, &import_targets) {
                tx.execute(
                    "UPDATE edges SET target_qualified = ?, target_name = ?, \
                     confidence = ?, confidence_tier = ? WHERE id = ?",
                    params![
                        qualified,
                        edge_target_name(&qualified),
                        INFERRED_CONFIDENCE,
                        ConfidenceTier::Medium.as_str(),
                        edge_id
                    ],
                )?;
                resolved += 1;
                continue;
            }
            let mut extra = parse_json_column(extra_raw)?;
            if extra
                .get("bare_name_unresolved")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            extra
                .as_object_mut()
                .map(|obj| obj.insert("bare_name_unresolved".to_string(), Value::Bool(true)));
            tx.execute(
                "UPDATE edges SET extra = ?, confidence = ?, confidence_tier = ? WHERE id = ?",
                params![
                    extra_json(&extra)?,
                    BARE_UNRESOLVED_CONFIDENCE,
                    ConfidenceTier::Low.as_str(),
                    edge_id
                ],
            )?;
        }
        tx.commit()?;
        Ok(resolved)
    }

    pub fn replace_manifest_bridges(
        &mut self,
        extractor_id: &str,
        nodes: &[NodeInput],
        edges: &[EdgeInput],
    ) -> Result<i64> {
        let now = now_seconds()?;
        let tx = write_tx(&mut self.conn)?;
        tx.execute(
            "DELETE FROM edges WHERE kind='CROSS_ARTIFACT' \
             AND json_extract(extra, '$.extractor') = ?",
            [extractor_id],
        )?;
        tx.execute(
            "DELETE FROM nodes WHERE kind='File' \
             AND json_extract(extra, '$.extractor') = ?",
            [extractor_id],
        )?;

        let mut nodes_upserted = 0_i64;
        let mut touched_files: HashSet<String> = HashSet::new();
        for node in nodes {
            let qualified = make_qualified_parts(
                &node.kind,
                &node.name,
                &node.file_path,
                node.parent_name.as_deref(),
            );
            if node.kind == "File" {
                let exists: bool = tx
                    .query_row(
                        "SELECT 1 FROM nodes WHERE qualified_name = ?",
                        [&qualified],
                        |_| Ok(true),
                    )
                    .optional()?
                    .unwrap_or(false);
                if exists {
                    continue;
                }
            }
            let extra = extra_json(&node.extra)?;
            tx.execute(
                "INSERT INTO nodes
                    (kind, name, qualified_name, file_path, line_start, line_end,
                     language, parent_name, params, return_type, modifiers, is_test,
                     file_hash, mtime_ns, extra, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(qualified_name) DO UPDATE SET
                    kind=excluded.kind, name=excluded.name,
                    file_path=excluded.file_path, line_start=excluded.line_start,
                    line_end=excluded.line_end, language=excluded.language,
                    parent_name=excluded.parent_name, params=excluded.params,
                    return_type=excluded.return_type, modifiers=excluded.modifiers,
                    is_test=excluded.is_test, extra=excluded.extra, updated_at=excluded.updated_at",
                params![
                    node.kind,
                    node.name,
                    qualified,
                    node.file_path,
                    node.line_start,
                    node.line_end,
                    node.language,
                    node.parent_name,
                    node.params,
                    node.return_type,
                    node.modifiers,
                    i64::from(node.is_test),
                    "",
                    0_i64,
                    extra,
                    now
                ],
            )?;
            touched_files.insert(node.file_path.clone());
            nodes_upserted += 1;
        }

        for edge in edges {
            let extra_val = &edge.extra;
            let extra = extra_json(extra_val)?;
            let (confidence, tier) = edge_metadata_from_raw_extra(&extra)?;
            let (confidence, tier) =
                normalize_edge_confidence(&edge.source, &edge.target, confidence, tier);
            let target_name = edge_target_name(&edge.target);
            let updated = tx.execute(
                "UPDATE edges
                 SET target_name=?, extra=?, confidence=?, confidence_tier=?, updated_at=?
                 WHERE kind=? AND source_qualified=? AND target_qualified=?
                       AND file_path=? AND line=?",
                params![
                    target_name,
                    extra,
                    confidence,
                    tier.as_str(),
                    now,
                    edge.kind,
                    edge.source,
                    edge.target,
                    edge.file_path,
                    edge.line
                ],
            )?;
            if updated == 0 {
                tx.execute(
                    "INSERT INTO edges
                        (kind, source_qualified, target_qualified, target_name, file_path, line, extra,
                         confidence, confidence_tier, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        edge.kind,
                        edge.source,
                        edge.target,
                        target_name,
                        edge.file_path,
                        edge.line,
                        extra,
                        confidence,
                        tier.as_str(),
                        now
                    ],
                )?;
            }
        }

        let files: Vec<String> = touched_files.into_iter().collect();
        crate::fts_sync::sync_fts_for_file_paths_tx(&tx, &files, None)?;
        tx.commit()?;
        Ok(nodes_upserted)
    }

    pub fn replace_manifest_bridges_json(
        &mut self,
        extractor_id: &str,
        nodes_json: &str,
        edges_json: &str,
    ) -> Result<i64> {
        let nodes: Vec<NodeInput> = serde_json::from_str(nodes_json)?;
        let edges: Vec<EdgeInput> = serde_json::from_str(edges_json)?;
        self.replace_manifest_bridges(extractor_id, &nodes, &edges)
    }
}

fn terraform_entrypoint_match(
    tx: &Transaction<'_>,
    symbol: &str,
) -> Result<Option<(String, String)>> {
    let symbol = symbol.trim();
    if symbol.is_empty() || symbol.starts_with('<') {
        return Ok(None);
    }
    let mut stmt = tx.prepare(
        "SELECT qualified_name, language, file_path FROM nodes \
         WHERE name = ? AND kind IN ('Function', 'Test') AND language != 'markdown'",
    )?;
    if let Some((module, attr)) = symbol.rsplit_once('.') {
        if module.is_empty() || attr.is_empty() {
            return Ok(None);
        }
        let rows = stmt.query_map([attr], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?
                    .unwrap_or_else(|| "unknown".to_string()),
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ))
        })?;
        let matches: Vec<(String, String)> = rows
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, _, file_path)| terraform_module_matches_file(module, file_path))
            .map(|(qn, lang, _)| (qn, lang))
            .collect();
        return Ok(unique_pair(matches));
    }
    let rows = stmt.query_map([symbol], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?
                .unwrap_or_else(|| "unknown".to_string()),
        ))
    })?;
    let matches: Vec<(String, String)> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(unique_pair(matches))
}

fn unique_pair(matches: Vec<(String, String)>) -> Option<(String, String)> {
    let mut iter = matches.into_iter();
    let first = iter.next()?;
    if iter.next().is_some() {
        None
    } else {
        Some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::PathBuf;

    fn temp_db(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "dagayn-postprocess-{}-{}.db",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn function_node(name: &str, file_path: &str) -> NodeInput {
        NodeInput {
            kind: "Function".to_string(),
            name: name.to_string(),
            file_path: file_path.to_string(),
            line_start: 1,
            line_end: 2,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        }
    }

    fn file_node(file_path: &str) -> NodeInput {
        NodeInput {
            kind: "File".to_string(),
            name: file_path.to_string(),
            file_path: file_path.to_string(),
            line_start: 1,
            line_end: 1,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        }
    }

    #[test]
    fn demotes_missing_call_targets() {
        let path = temp_db("demote");
        let mut store = GraphStore::open(&path).expect("open");
        store
            .store_file_nodes_edges(
                "app.py",
                &[file_node("app.py"), function_node("main", "app.py")],
                &[EdgeInput {
                    kind: "CALLS".to_string(),
                    source: "app.py::main".to_string(),
                    target: "missing".to_string(),
                    file_path: "app.py".to_string(),
                    line: 1,
                    extra: json!({}),
                }],
                "",
                0,
            )
            .expect("store");
        assert_eq!(store.demote_unresolved_endpoint_edges().unwrap(), 1);
        let tier: String = store
            .conn
            .query_row(
                "SELECT confidence_tier FROM edges WHERE kind='CALLS'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tier, "LOW");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn resolves_unique_terraform_handler() {
        let path = temp_db("terraform");
        let mut store = GraphStore::open(&path).expect("open");
        store
            .store_file_nodes_edges(
                "app/hello.py",
                &[
                    file_node("app/hello.py"),
                    function_node("main", "app/hello.py"),
                ],
                &[],
                "",
                0,
            )
            .expect("store");
        let extra = json!({
            "source_language": "terraform",
            "evidence_source": "handler",
            "relationship_role": "maps_entrypoint",
            "original_symbol_name": "hello.main",
        });
        store
            .conn
            .execute(
                "INSERT INTO edges (kind, source_qualified, target_qualified, target_name,
                     file_path, line, extra, confidence, confidence_tier, updated_at)
                 VALUES ('CROSS_ARTIFACT', 'infra/main.tf', '<unresolved:hello.main>',
                         'hello.main', 'infra/main.tf', 1, ?, 0.8, 'HIGH', 0)",
                [extra.to_string()],
            )
            .unwrap();
        assert_eq!(store.resolve_terraform_artifact_refs().unwrap(), (1, 0));
        let target: String = store
            .conn
            .query_row(
                "SELECT target_qualified FROM edges WHERE kind='CROSS_ARTIFACT'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target, "app/hello.py::main");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn resolves_bare_call_via_import() {
        let path = temp_db("bare-call");
        let mut store = GraphStore::open(&path).expect("open");
        store
            .store_file_nodes_edges(
                "a.py",
                &[file_node("a.py"), function_node("helper", "a.py")],
                &[],
                "",
                0,
            )
            .expect("store a");
        store
            .store_file_nodes_edges(
                "b.py",
                &[file_node("b.py"), function_node("run", "b.py")],
                &[
                    EdgeInput {
                        kind: "IMPORTS_FROM".to_string(),
                        source: "b.py".to_string(),
                        target: "a.py".to_string(),
                        file_path: "b.py".to_string(),
                        line: 1,
                        extra: json!({}),
                    },
                    EdgeInput {
                        kind: "CALLS".to_string(),
                        source: "b.py::run".to_string(),
                        target: "helper".to_string(),
                        file_path: "b.py".to_string(),
                        line: 2,
                        extra: json!({}),
                    },
                ],
                "",
                0,
            )
            .expect("store b");
        assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
        let target: String = store
            .conn
            .query_row(
                "SELECT target_qualified FROM edges WHERE kind='CALLS'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target, "a.py::helper");
        let _ = std::fs::remove_file(path);
    }
}
