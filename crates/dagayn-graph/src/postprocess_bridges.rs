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

/// Import targets that are not file paths, keyed by the file that can be
/// reached through them. Mirrors `dagayn.bare_name_resolution`.
const NAMESPACE_FILE_SUFFIXES: &[&str] = &[
    ".c", ".cpp", ".cs", ".dart", ".go", ".h", ".hpp", ".java", ".jl", ".js", ".json", ".jsx",
    ".kt", ".md", ".php", ".py", ".rb", ".rs", ".scala", ".swift", ".tf", ".ts", ".tsx",
];

fn normalize_namespace(value: &str) -> String {
    value
        .replace('\\', ".")
        .replace("::", ".")
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

fn is_namespace_candidate(target: &str) -> bool {
    if target.contains('/') {
        return false;
    }
    let suffix = target
        .rsplit_once('.')
        .map(|(_, suffix)| format!(".{}", suffix.to_ascii_lowercase()));
    match suffix {
        Some(suffix) => !NAMESPACE_FILE_SUFFIXES.contains(&suffix.as_str()),
        None => true,
    }
}

type StringListMaps = (
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
);

/// Indirect visibility between files: namespaces and declaring classes. Held
/// as per-file maps rather than an expanded file-to-file product, since a
/// single namespace with N files would otherwise cost N^2 entries.
#[derive(Default)]
pub(crate) struct SymbolVisibility {
    /// File -> namespaces it declares.
    declared: HashMap<String, HashSet<String>>,
    /// File -> namespaces its imports name.
    imported: HashMap<String, HashSet<String>>,
    /// Class name -> files declaring that class.
    class_files: HashMap<String, HashSet<String>>,
}

impl SymbolVisibility {
    fn has_namespaces(&self) -> bool {
        !self.declared.is_empty()
    }

    /// True when *source_file* can reach *target_file* without a file-level
    /// import: either they share a namespace, or the source imports one the
    /// target declares.
    fn can_see(&self, source_file: &str, target_file: &str) -> bool {
        let Some(declared) = self.declared.get(target_file) else {
            return false;
        };
        let shares = |other: Option<&HashSet<String>>| {
            other.is_some_and(|other| declared.iter().any(|namespace| other.contains(namespace)))
        };
        shares(self.declared.get(source_file)) || shares(self.imported.get(source_file))
    }

    /// Files declaring the class that owns *target_qualified*.
    ///
    /// A C++ method is defined in a `.cpp` that nobody includes, while its
    /// class is declared in the header that callers do include -- so the
    /// header, not the definition file, is what a caller can see.
    fn declaring_files(&self, target_qualified: &str) -> Option<&HashSet<String>> {
        let (_, symbol) = target_qualified.split_once("::")?;
        let (owner, _) = symbol.rsplit_once('.')?;
        self.class_files.get(owner)
    }

    pub(crate) fn as_string_lists(&self) -> StringListMaps {
        let flatten = |map: &HashMap<String, HashSet<String>>| {
            map.iter()
                .map(|(key, values)| (key.clone(), values.iter().cloned().collect()))
                .collect()
        };
        (
            flatten(&self.declared),
            flatten(&self.imported),
            flatten(&self.class_files),
        )
    }
}

/// Reads declared namespaces from `File` nodes, imported ones from
/// IMPORTS_FROM targets that name a namespace rather than a file, and the
/// files that declare each class.
pub(crate) fn symbol_visibility(conn: &rusqlite::Connection) -> Result<SymbolVisibility> {
    let mut visibility = SymbolVisibility::default();
    {
        let mut stmt = conn.prepare(
            "SELECT file_path, extra FROM nodes WHERE kind = 'File' AND extra LIKE '%namespaces%'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (file_path, extra_raw) = row?;
            let extra = parse_json_column(extra_raw)?;
            let Some(declared) = extra.get("namespaces").and_then(Value::as_array) else {
                continue;
            };
            for namespace in declared.iter().filter_map(Value::as_str) {
                let key = normalize_namespace(namespace);
                if key.is_empty() {
                    continue;
                }
                visibility
                    .declared
                    .entry(file_path.clone())
                    .or_default()
                    .insert(key);
            }
        }
    }
    {
        let mut stmt = conn.prepare("SELECT name, file_path FROM nodes WHERE kind = 'Class'")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (name, file_path) = row?;
            visibility
                .class_files
                .entry(name)
                .or_default()
                .insert(file_path);
        }
    }
    if !visibility.has_namespaces() {
        return Ok(visibility);
    }
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT file_path, target_qualified FROM edges WHERE kind = 'IMPORTS_FROM'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (file_path, target) = row?;
            if !is_namespace_candidate(&target) {
                continue;
            }
            let key = normalize_namespace(&target);
            if key.is_empty() {
                continue;
            }
            let entry = visibility.imported.entry(file_path).or_default();
            // `using A.B.Type` names a symbol inside namespace `A.B`.
            if let Some((parent, _)) = key.rsplit_once('.') {
                entry.insert(parent.to_string());
            }
            entry.insert(key);
        }
    }
    Ok(visibility)
}

fn import_targets_conn(conn: &rusqlite::Connection) -> Result<HashMap<String, HashSet<String>>> {
    let mut import_targets: HashMap<String, HashSet<String>> = HashMap::new();
    let mut stmt = conn.prepare(
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

fn import_targets_tx(tx: &Transaction<'_>) -> Result<HashMap<String, HashSet<String>>> {
    import_targets_conn(tx)
}

fn is_plausible_bare_edge(
    source_file: &str,
    target_file: &str,
    import_targets: &HashMap<String, HashSet<String>>,
    visibility: &SymbolVisibility,
    target_qualified: &str,
) -> bool {
    if source_file.is_empty() || target_file.is_empty() {
        return false;
    }
    if file_is_visible(source_file, target_file, import_targets, visibility) {
        return true;
    }
    // Reaching the class declaration is enough; the definition may live in a
    // file nobody imports directly.
    visibility
        .declaring_files(target_qualified)
        .is_some_and(|declaring| {
            declaring
                .iter()
                .any(|file| file_is_visible(source_file, file, import_targets, visibility))
        })
}

fn file_is_visible(
    source_file: &str,
    target_file: &str,
    import_targets: &HashMap<String, HashSet<String>>,
    visibility: &SymbolVisibility,
) -> bool {
    source_file == target_file
        || import_targets
            .get(source_file)
            .is_some_and(|targets| targets.contains(target_file))
        || visibility.can_see(source_file, target_file)
}

fn load_bare_name_index(
    tx: &Transaction<'_>,
    kinds: &[&str],
) -> Result<HashMap<String, Vec<String>>> {
    if kinds.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = std::iter::repeat_n("?", kinds.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT name, qualified_name FROM nodes WHERE kind IN ({placeholders})");
    let mut stmt = tx.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(kinds), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut index = HashMap::<String, Vec<String>>::new();
    for row in rows {
        let (name, qualified_name) = row?;
        index.entry(name).or_default().push(qualified_name);
    }
    Ok(index)
}

fn resolve_via_imports(
    candidates: &[String],
    source_file: &str,
    import_targets: &HashMap<String, HashSet<String>>,
    visibility: &SymbolVisibility,
) -> Option<String> {
    let imported: Vec<&String> = candidates
        .iter()
        .filter(|qn| {
            is_plausible_bare_edge(
                source_file,
                &node_file_from_qualified(qn, ""),
                import_targets,
                visibility,
                qn,
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
        Ok(import_targets_conn(&self.conn)?
            .into_iter()
            .map(|(file_path, targets)| (file_path, targets.into_iter().collect()))
            .collect())
    }

    /// `(declared namespaces, imported namespaces, class files)` for
    /// query-time bare-name resolution.
    pub fn symbol_visibility_by_file(&self) -> Result<StringListMaps> {
        Ok(symbol_visibility(&self.conn)?.as_string_lists())
    }

    pub fn resolve_bare_call_targets(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        let import_targets = import_targets_tx(&tx)?;
        let visibility = symbol_visibility(&tx)?;
        let index = load_bare_name_index(&tx, &["Function", "Test", "Class"])?;
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
            let candidates = index.get(&target_qualified).cloned().unwrap_or_default();
            if candidates.is_empty() {
                continue;
            }
            let src_file = node_file_from_qualified(&source_qualified, &file_path);
            let Some(qualified) =
                resolve_via_imports(&candidates, &src_file, &import_targets, &visibility)
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
        let visibility = symbol_visibility(&tx)?;
        let index = load_bare_name_index(&tx, &["Class"])?;
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
            let candidates = index.get(&target_qualified).cloned().unwrap_or_default();
            let src_file = node_file_from_qualified(&source_qualified, &file_path);
            if let Some(qualified) =
                resolve_via_imports(&candidates, &src_file, &import_targets, &visibility)
            {
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

    fn class_node(name: &str, file_path: &str) -> NodeInput {
        NodeInput {
            kind: "Class".to_string(),
            ..function_node(name, file_path)
        }
    }

    fn method_node(name: &str, file_path: &str, owner: &str) -> NodeInput {
        NodeInput {
            parent_name: Some(owner.to_string()),
            ..function_node(name, file_path)
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

    fn namespaced_file_node(file_path: &str, namespace: &str) -> NodeInput {
        let mut node = file_node(file_path);
        node.extra = json!({"namespaces": [namespace]});
        node
    }

    /// Issue #154: C# files in one namespace need no `using` between them.
    #[test]
    fn resolves_bare_call_via_shared_namespace() {
        let path = temp_db("bare-call-namespace");
        let mut store = GraphStore::open(&path).expect("open");
        store
            .store_file_nodes_edges(
                "Factory.cs",
                &[
                    namespaced_file_node("Factory.cs", "Repro.Infra"),
                    function_node("CreateCriteria", "Factory.cs"),
                ],
                &[],
                "",
                0,
            )
            .expect("store factory");
        // Same method name in another namespace must not win the resolution.
        store
            .store_file_nodes_edges(
                "Decoy.cs",
                &[
                    namespaced_file_node("Decoy.cs", "Repro.Other"),
                    function_node("CreateCriteria", "Decoy.cs"),
                ],
                &[],
                "",
                0,
            )
            .expect("store decoy");
        store
            .store_file_nodes_edges(
                "Broker.cs",
                &[
                    namespaced_file_node("Broker.cs", "Repro.Infra"),
                    function_node("Resolve", "Broker.cs"),
                ],
                &[EdgeInput {
                    kind: "CALLS".to_string(),
                    source: "Broker.cs::Resolve".to_string(),
                    target: "CreateCriteria".to_string(),
                    file_path: "Broker.cs".to_string(),
                    line: 2,
                    extra: json!({}),
                }],
                "",
                0,
            )
            .expect("store broker");
        assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
        let target: String = store
            .conn
            .query_row(
                "SELECT target_qualified FROM edges WHERE kind='CALLS'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target, "Factory.cs::CreateCriteria");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn resolves_bare_call_via_imported_namespace() {
        let path = temp_db("bare-call-imported-namespace");
        let mut store = GraphStore::open(&path).expect("open");
        store
            .store_file_nodes_edges(
                "Broker.php",
                &[
                    namespaced_file_node("Broker.php", "App\\Util"),
                    function_node("phpBuild", "Broker.php"),
                ],
                &[],
                "",
                0,
            )
            .expect("store broker");
        store
            .store_file_nodes_edges(
                "Factory.php",
                &[
                    namespaced_file_node("Factory.php", "App\\Infra"),
                    function_node("make", "Factory.php"),
                ],
                &[
                    EdgeInput {
                        kind: "IMPORTS_FROM".to_string(),
                        source: "Factory.php".to_string(),
                        // `use App\Util\Broker` names a symbol in the namespace.
                        target: "App\\Util\\Broker".to_string(),
                        file_path: "Factory.php".to_string(),
                        line: 1,
                        extra: json!({}),
                    },
                    EdgeInput {
                        kind: "CALLS".to_string(),
                        source: "Factory.php::make".to_string(),
                        target: "phpBuild".to_string(),
                        file_path: "Factory.php".to_string(),
                        line: 2,
                        extra: json!({}),
                    },
                ],
                "",
                0,
            )
            .expect("store factory");
        assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
        let target: String = store
            .conn
            .query_row(
                "SELECT target_qualified FROM edges WHERE kind='CALLS'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target, "Broker.php::phpBuild");
        let _ = std::fs::remove_file(path);
    }

    /// A C++ definition lives in a `.cpp` nobody includes; only its class
    /// declaration is reachable from the caller's header.
    #[test]
    fn resolves_bare_call_via_declaring_class() {
        let path = temp_db("bare-call-declaring-class");
        let mut store = GraphStore::open(&path).expect("open");
        store
            .store_file_nodes_edges(
                "include/factory.hpp",
                &[
                    file_node("include/factory.hpp"),
                    class_node("Factory", "include/factory.hpp"),
                ],
                &[],
                "",
                0,
            )
            .expect("store header");
        store
            .store_file_nodes_edges(
                "src/factory.cpp",
                &[
                    file_node("src/factory.cpp"),
                    method_node("createAllowed", "src/factory.cpp", "Factory"),
                ],
                &[EdgeInput {
                    kind: "IMPORTS_FROM".to_string(),
                    source: "src/factory.cpp".to_string(),
                    target: "include/factory.hpp".to_string(),
                    file_path: "src/factory.cpp".to_string(),
                    line: 1,
                    extra: json!({}),
                }],
                "",
                0,
            )
            .expect("store definition");
        // An unrelated class with a same-named method must not win.
        store
            .store_file_nodes_edges(
                "src/other.cpp",
                &[
                    file_node("src/other.cpp"),
                    class_node("Unrelated", "src/other.cpp"),
                    method_node("createAllowed", "src/other.cpp", "Unrelated"),
                ],
                &[],
                "",
                0,
            )
            .expect("store other");
        store
            .store_file_nodes_edges(
                "src/broker.cpp",
                &[
                    file_node("src/broker.cpp"),
                    function_node("use", "src/broker.cpp"),
                ],
                &[
                    EdgeInput {
                        kind: "IMPORTS_FROM".to_string(),
                        source: "src/broker.cpp".to_string(),
                        target: "include/factory.hpp".to_string(),
                        file_path: "src/broker.cpp".to_string(),
                        line: 1,
                        extra: json!({}),
                    },
                    EdgeInput {
                        kind: "CALLS".to_string(),
                        source: "src/broker.cpp::use".to_string(),
                        target: "createAllowed".to_string(),
                        file_path: "src/broker.cpp".to_string(),
                        line: 3,
                        extra: json!({}),
                    },
                ],
                "",
                0,
            )
            .expect("store caller");
        assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
        let target: String = store
            .conn
            .query_row(
                "SELECT target_qualified FROM edges WHERE kind='CALLS'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target, "src/factory.cpp::Factory.createAllowed");
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
