use crate::helpers::*;
use crate::postprocess_bridges::extra_json;
use crate::postprocess_tested_by::sync_tested_by_with_calls;
use crate::*;

const INFERRED_CONFIDENCE: f64 = 0.6;

const BARE_UNRESOLVED_CONFIDENCE: f64 = 0.3;

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
        ".mjs",
        ".cjs",
        ".ts",
        ".mts",
        ".cts",
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

/// Import targets that are not file paths, keyed by the file that can be
/// reached through them. Mirrors `dagayn.bare_name_resolution`.
const NAMESPACE_FILE_SUFFIXES: &[&str] = &[
    ".c", ".cjs", ".cpp", ".cs", ".cts", ".dart", ".go", ".h", ".hpp", ".java", ".jl", ".js",
    ".json", ".jsx", ".kt", ".md", ".mjs", ".mts", ".php", ".py", ".rb", ".rs", ".scala", ".swift",
    ".tf", ".ts", ".tsx",
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
    // Members of JavaScript object-literal containers (`const api = { get() {} }`,
    // `type_role: "object"`) are reachable only through the container
    // (`api.get()`), so a bare `get` call elsewhere must never bind to them.
    let sql = format!(
        "SELECT n.name, n.qualified_name FROM nodes n \
         WHERE n.kind IN ({placeholders}) \
           AND NOT (n.parent_name IS NOT NULL AND EXISTS ( \
               SELECT 1 FROM nodes p \
               WHERE p.qualified_name = n.file_path || '::' || n.parent_name \
                 AND p.kind = 'Class' \
                 AND json_extract(p.extra, '$.type_role') = 'object'))"
    );
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
        sync_tested_by_with_calls(&tx)?;
        tx.commit()?;
        Ok(resolved)
    }

    pub fn resolve_bare_inheritance_targets(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        let import_targets = import_targets_tx(&tx)?;
        let visibility = symbol_visibility(&tx)?;
        let index = load_bare_name_index(&tx, &["Class"])?;
        // TypeScript `interface X extends Alias` / `class C implements Alias`
        // may name an object-shaped type alias (a `Type` node). Classes are
        // tried first so the alias index only adds resolutions.
        let alias_index = load_bare_name_index(&tx, &["Type"])?;
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
            let resolved_target =
                resolve_via_imports(&candidates, &src_file, &import_targets, &visibility).or_else(
                    || {
                        let aliases = alias_index.get(&target_qualified)?;
                        resolve_via_imports(aliases, &src_file, &import_targets, &visibility)
                    },
                );
            if let Some(qualified) = resolved_target {
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
}
