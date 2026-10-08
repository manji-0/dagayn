use crate::helpers::*;
use crate::postprocess::external_calls::{
    mark_deref_calls, mark_glob_imported_external_calls, mark_observed_method_calls,
    mark_stdlib_method_calls, resolve_enum_variant_calls,
};
use crate::postprocess::reexports::resolve_reexported_targets;
use crate::postprocess::returned::{
    ResolveContext, resolve_pyo3_methods, resolve_returned_receivers,
};
use crate::postprocess::sync_tested_by_with_calls;
use crate::postprocess::tested_by::reconcile_tested_by_with_calls;
use crate::*;

pub(crate) const DIRECT_IMPORT_CONFIDENCE: f64 = 0.9;

pub(crate) const INFERRED_CONFIDENCE: f64 = 0.6;

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
        let rows = stmt.query_map([], |row| <(String, Option<String>)>::try_from(row))?;
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
        let rows = stmt.query_map([], |row| <(String, String)>::try_from(row))?;
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
        let rows = stmt.query_map([], |row| <(String, String)>::try_from(row))?;
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

/// Files each file can name symbols from through its imports.
///
/// Besides the files it imports directly, a file sees what those files
/// re-export (Rust `pub use child::*` / `pub(crate) use child::item`, marked
/// `re_export` on the edge, and whatever a Python package's `__init__.py`
/// imports, which its importers reach as the package's attributes),
/// transitively, and everything a module it glob-imports (`use super::*`,
/// `glob`) itself imports.
fn import_targets_conn(conn: &rusqlite::Connection) -> Result<HashMap<String, HashSet<String>>> {
    let mut direct: HashMap<String, HashSet<String>> = HashMap::new();
    let mut re_exports: HashMap<String, HashSet<String>> = HashMap::new();
    let mut globs: HashMap<String, HashSet<String>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT file_path, target_qualified, \
                COALESCE(json_extract(extra, '$.re_export'), 0) \
                    OR file_path LIKE '%/__init__.py' OR file_path = '__init__.py', \
                COALESCE(json_extract(extra, '$.glob'), 0) \
         FROM edges WHERE kind = 'IMPORTS_FROM'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? != 0,
            row.get::<_, i64>(3)? != 0,
        ))
    })?;
    for row in rows {
        let (file_path, target, re_export, glob) = row?;
        let target_file = node_file_from_qualified(&target, &target);
        if re_export {
            re_exports
                .entry(file_path.clone())
                .or_default()
                .insert(target_file.clone());
        }
        if glob {
            globs
                .entry(file_path.clone())
                .or_default()
                .insert(target_file.clone());
        }
        direct.entry(file_path).or_default().insert(target_file);
    }
    if re_exports.is_empty() && globs.is_empty() {
        return Ok(direct);
    }
    // Fixpoint: a file sees its direct imports, what they re-export, and
    // everything a module it glob-imports sees (globs chain: `use super::*`
    // in `a/b/c.rs` reaches what `a/b.rs` itself glob-imported).
    let mut import_targets: HashMap<String, HashSet<String>> = direct.clone();
    loop {
        let mut changed = false;
        let files: Vec<String> = import_targets.keys().cloned().collect();
        for file in files {
            let mut seen = import_targets[&file].clone();
            let before = seen.len();
            let mut pending: Vec<String> = seen.iter().cloned().collect();
            for module in globs.get(&file).into_iter().flatten() {
                for inherited in import_targets.get(module).into_iter().flatten() {
                    if seen.insert(inherited.clone()) {
                        pending.push(inherited.clone());
                    }
                }
            }
            while let Some(module) = pending.pop() {
                for exported in re_exports.get(&module).into_iter().flatten() {
                    if seen.insert(exported.clone()) {
                        pending.push(exported.clone());
                    }
                }
            }
            if seen.len() != before {
                changed = true;
                import_targets.insert(file, seen);
            }
        }
        if !changed {
            break;
        }
    }
    Ok(import_targets)
}

pub(crate) fn import_targets_tx(tx: &Transaction<'_>) -> Result<HashMap<String, HashSet<String>>> {
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
    // file nobody imports directly. Only a declaration in the definition's own
    // language counts: a Python `GraphStore` wrapper does not declare the Rust
    // struct of the same name, so importing it must not make the struct's
    // methods visible.
    let family = language_family(target_file);
    visibility
        .declaring_files(target_qualified)
        .is_some_and(|declaring| {
            declaring.iter().any(|file| {
                same_language_family(family, language_family(file))
                    && file_is_visible(source_file, file, import_targets, visibility)
            })
        })
}

/// Language family by file extension, for pairing a definition with the
/// files that declare its class (a C++ `.cpp` with its `.h`). `None` when the
/// extension is unknown, which never rules a declaration out.
pub(crate) fn language_family(file_path: &str) -> Option<&'static str> {
    let ext = file_path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "py" | "pyi" | "ipynb" => "python",
        "rs" => "rust",
        "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "m" | "mm" => "c",
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "vue" | "svelte" => {
            "javascript"
        }
        "java" | "kt" | "kts" | "scala" | "sc" => "jvm",
        "cs" => "csharp",
        "go" => "go",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "dart" => "dart",
        "lua" => "lua",
        "pl" | "pm" => "perl",
        "r" => "r",
        "jl" => "julia",
        "ex" | "exs" => "elixir",
        "zig" => "zig",
        "gd" => "gdscript",
        "sh" | "bash" | "zsh" => "bash",
        _ => return None,
    })
}

fn same_language_family(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
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

/// Binds bare call targets by name: through a module path, the receiver's
/// type, or import visibility. Calls on the result of another call
/// (`receiver_from`) wait for that call's return type to type them, so they
/// are bound by name only when `on_returned_values`, after that pass could
/// not (an untyped `makeBox()` in JavaScript). A callee a SCIP index found
/// to be a local or a parameter (`callee_local`) is never bound.
fn bind_bare_call_targets(
    tx: &Transaction<'_>,
    index: &HashMap<String, Vec<String>>,
    import_targets: &HashMap<String, HashSet<String>>,
    visibility: &SymbolVisibility,
    on_returned_values: bool,
) -> Result<i64> {
    let edges = {
        let mut stmt = tx.prepare(
            "SELECT id, source_qualified, target_qualified, file_path, \
                    json_extract(extra, '$.receiver_type'), \
                    json_extract(extra, '$.module_file'), \
                    COALESCE(json_extract(extra, '$.receiver_unknown'), 0) \
             FROM edges \
             WHERE (kind = 'CALLS' \
                    OR (kind = 'REFERENCES' \
                        AND COALESCE(json_extract(extra, '$.value_reference'), 0) = 1)) \
               AND target_qualified NOT LIKE '%::%' \
               AND COALESCE(json_extract(extra, '$.external'), 0) = 0 \
               AND COALESCE(json_extract(extra, '$.callee_local'), 0) = 0 \
               AND (json_extract(extra, '$.receiver_from') IS NULL) = ?",
        )?;
        let mapped = stmt.query_map([!on_returned_values], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)? != 0,
            ))
        })?;
        mapped.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut resolved = 0_i64;
    for (
        edge_id,
        source_qualified,
        target_qualified,
        file_path,
        receiver_type,
        module_file,
        receiver_unknown,
    ) in edges
    {
        if looks_like_file_target(&target_qualified) {
            continue;
        }
        let src_file = node_file_from_qualified(&source_qualified, &file_path);
        let rust = language_family(&src_file) == Some("rust");
        // Rust `x.m()` on a receiver of unknown type: the extractor already
        // typed every receiver the syntax gives away, and binding the rest
        // to whichever `m` is visible is usually wrong (`tx.commit()`).
        if rust && receiver_unknown {
            continue;
        }
        let mut candidates = index.get(&target_qualified).cloned().unwrap_or_default();
        // `x.m()` on a receiver of unknown type is not a call on the
        // caller's own object (`this.m()` / `self.m()` are typed), so a
        // member of a class or function enclosing the caller is not its
        // target: `this.item.show()` is the item's `show`, not the caller
        // class's.
        if receiver_unknown {
            candidates.retain(|qn| {
                let Some((file, symbol)) = qn.split_once("::") else {
                    return true;
                };
                !symbol.rsplit_once('.').is_some_and(|(owner, _)| {
                    source_qualified.starts_with(&format!("{file}::{owner}."))
                })
            });
        }
        // A bare Rust call names a function a `use` brought in, i.e. one at
        // the top of its module file, never a method or a function of an
        // inline module such as `mod tests`.
        if receiver_type.is_none() && rust {
            candidates.retain(|qn| {
                !qn.split_once("::")
                    .is_some_and(|(_, symbol)| symbol.contains('.'))
            });
        }
        if candidates.is_empty() {
            continue;
        }
        let resolution = match (receiver_type.as_deref(), module_file.as_deref()) {
            // A method of a known type (`.map(Dep::finding)`), in the
            // type's module file when the path named it.
            (Some(receiver_type), module_file) => {
                let in_module: Vec<String> = module_file
                    .map(|file| {
                        candidates
                            .iter()
                            .filter(|qn| node_file_from_qualified(qn, "") == file)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                resolve_type_receiver(
                    if in_module.is_empty() {
                        &candidates
                    } else {
                        &in_module
                    },
                    &target_qualified,
                    receiver_type,
                    &src_file,
                    import_targets,
                    visibility,
                )
            }
            (None, Some(module_file)) => {
                resolve_in_module(&candidates, module_file, import_targets)
            }
            (None, None) => resolve_via_imports(&candidates, &src_file, import_targets, visibility),
        };
        let Some((qualified, confidence, tier)) = resolution else {
            continue;
        };
        tx.execute(
            "UPDATE edges SET target_qualified = ?, target_name = ?, \
             confidence = ?, confidence_tier = ? WHERE id = ?",
            params![
                qualified,
                edge_target_name(&qualified),
                confidence,
                tier.as_str(),
                edge_id
            ],
        )?;
        resolved += 1;
    }
    Ok(resolved)
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
        <(String, String)>::try_from(row)
    })?;
    let mut index = HashMap::<String, Vec<String>>::new();
    for row in rows {
        let (name, qualified_name) = row?;
        index.entry(name).or_default().push(qualified_name);
    }
    Ok(index)
}

/// The one visible candidate for a bare name, with the confidence its
/// evidence supports: a top-level symbol in the same file or in a file the
/// caller imports directly is `HIGH`. A method is `MEDIUM` even then, since
/// the receiver's type is unknown, as is anything reached only through a
/// namespace or a class declaration.
pub(crate) fn resolve_via_imports(
    candidates: &[String],
    source_file: &str,
    import_targets: &HashMap<String, HashSet<String>>,
    visibility: &SymbolVisibility,
) -> Option<(String, f64, ConfidenceTier)> {
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
    let [only] = imported.as_slice() else {
        return None;
    };
    let target_file = node_file_from_qualified(only, "");
    let top_level = !only
        .split_once("::")
        .is_some_and(|(_, symbol)| symbol.contains('.'));
    let direct = top_level
        && (source_file == target_file
            || import_targets
                .get(source_file)
                .is_some_and(|targets| targets.contains(&target_file)));
    let (confidence, tier) = if direct {
        (DIRECT_IMPORT_CONFIDENCE, ConfidenceTier::High)
    } else {
        (INFERRED_CONFIDENCE, ConfidenceTier::Medium)
    };
    Some(((*only).clone(), confidence, tier))
}

/// A call through a module path (Rust `crate::util::f()`, `module_file` on
/// the edge): a top-level symbol of that module's file, else of a file the
/// module re-exports or imports.
fn resolve_in_module(
    candidates: &[String],
    module_file: &str,
    import_targets: &HashMap<String, HashSet<String>>,
) -> Option<(String, f64, ConfidenceTier)> {
    let top_level = |qn: &&String| {
        !qn.split_once("::")
            .is_some_and(|(_, symbol)| symbol.contains('.'))
    };
    let in_file = |file: &str| {
        candidates
            .iter()
            .filter(top_level)
            .filter(|qn| node_file_from_qualified(qn, "") == file)
            .collect::<Vec<_>>()
    };
    if let [only] = in_file(module_file).as_slice() {
        return Some((
            (*only).clone(),
            DIRECT_IMPORT_CONFIDENCE,
            ConfidenceTier::High,
        ));
    }
    let reachable: Vec<&String> = candidates
        .iter()
        .filter(top_level)
        .filter(|qn| {
            import_targets
                .get(module_file)
                .is_some_and(|files| files.contains(&node_file_from_qualified(qn, "")))
        })
        .collect();
    let [only] = reachable.as_slice() else {
        return None;
    };
    Some((
        (*only).clone(),
        DIRECT_IMPORT_CONFIDENCE,
        ConfidenceTier::High,
    ))
}

/// A call on a named type (`Fast.fast_sum(...)`, `Native.Total(...)`,
/// `receiver_type` on the edge): only that type's methods, in the caller's
/// language, are candidates. The visible one wins as usual; when the caller
/// cannot see any (Ruby `require` and namespace-less C# files do not make a
/// file visible), a single such method is still the one the call names, at
/// `MEDIUM`. A type with no such method in the repository (`File.read`)
/// binds to nothing rather than to an unrelated same-named function.
fn resolve_type_receiver(
    candidates: &[String],
    target: &str,
    receiver_type: &str,
    source_file: &str,
    import_targets: &HashMap<String, HashSet<String>>,
    visibility: &SymbolVisibility,
) -> Option<(String, f64, ConfidenceTier)> {
    let family = language_family(source_file);
    let names_type = |path: &str| {
        path == receiver_type
            || path
                .strip_suffix(receiver_type)
                .is_some_and(|prefix| prefix.ends_with('.'))
    };
    let typed: Vec<String> = candidates
        .iter()
        .filter(|qn| {
            let Some((file, symbol)) = qn.split_once("::") else {
                return false;
            };
            if !same_language_family(family, language_family(file)) {
                return false;
            }
            match symbol.rsplit_once('.') {
                // A method of the type.
                Some((parent, _)) if names_type(parent) => true,
                // `new Native()`: the call names the type itself.
                _ => target == receiver_type && names_type(symbol),
            }
        })
        .cloned()
        .collect();
    if let Some(resolved) = resolve_via_imports(&typed, source_file, import_targets, visibility) {
        return Some(resolved);
    }
    let [only] = typed.as_slice() else {
        return None;
    };
    Some((only.clone(), INFERRED_CONFIDENCE, ConfidenceTier::Medium))
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

    /// The `kind` edges that name `target` only by its bare name and can
    /// plausibly mean it: `callers_of`'s (`CALLS`) and `inheritors_of`'s
    /// (`INHERITS`, `IMPLEMENTS`) fallback when nothing reaches it by its
    /// qualified name (`dagayn.tools.query_graph_support.filter_bare_name_fallback_edges`).
    /// Edges into external packages never qualify; a name only one function
    /// or class carries needs no import evidence; otherwise the source file
    /// must see the target's file (or its class declaration) by import or
    /// namespace.
    pub fn bare_name_edges(&self, target: &GraphNode, kind: &str) -> Result<Vec<GraphEdge>> {
        let edges = self.search_edges_by_target_name(&target.name, kind)?;
        if edges.is_empty() {
            return Ok(edges);
        }
        let edges: Vec<GraphEdge> = edges
            .into_iter()
            .filter(|edge| edge.extra.get("external") != Some(&Value::Bool(true)))
            .collect();
        if !target.name.is_empty() {
            let counts =
                self.count_nodes_by_name(&["Function".to_string(), "Class".to_string()], false)?;
            if counts.get(&target.name) == Some(&1) {
                return Ok(edges);
            }
        }
        let import_targets = import_targets_conn(&self.conn)?;
        let visibility = symbol_visibility(&self.conn)?;
        Ok(edges
            .into_iter()
            .filter(|edge| {
                is_plausible_bare_edge(
                    &node_file_from_qualified(&edge.source_qualified, &edge.file_path),
                    &target.file_path,
                    &import_targets,
                    &visibility,
                    &target.qualified_name,
                )
            })
            .collect())
    }

    /// Resolves the call targets a single file could not: symbols named
    /// through a module that re-exports them (`pkg/__init__.py::NodeInfo`),
    /// then bare names (by import visibility, declaring class, receiver
    /// type, or module), then calls on the result of another call (by its
    /// declared return type) and on a class a Rust extension exports to
    /// Python, then methods of receivers of unknown type that only the
    /// standard library defines. `TESTED_BY` then follows the calls.
    /// Returns how many targets were resolved (bare names and re-exports).
    pub fn resolve_bare_call_targets(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        let re_exported = resolve_reexported_targets(&tx)?;
        let context = ResolveContext::load(&tx, import_targets_tx(&tx)?)?;
        let import_targets = context.import_targets();
        let visibility = symbol_visibility(&tx)?;
        let index = load_bare_name_index(&tx, &["Function", "Test", "Class"])?;
        let mut resolved = bind_bare_call_targets(&tx, &index, import_targets, &visibility, false)?;
        resolved += resolve_enum_variant_calls(&tx)?;
        // Twice: a call these passes type (`repo_root.join(x)` as `std`) is
        // the origin a call on its result (`.exists()`) waits for.
        for _ in 0..2 {
            resolve_returned_receivers(&tx, &context)?;
            resolve_pyo3_methods(&tx)?;
            mark_glob_imported_external_calls(&tx)?;
            mark_deref_calls(&tx)?;
            mark_stdlib_method_calls(&tx, &context)?;
        }
        resolved += bind_bare_call_targets(&tx, &index, import_targets, &visibility, true)?;
        // Last: what every other pass typed is what it learns from.
        mark_observed_method_calls(&tx, &context)?;
        // A call it typed (`conn.prepare(..)` as `rusqlite`'s) is the origin
        // a call on its result (`.query_map(..)`) waits for.
        resolve_returned_receivers(&tx, &context)?;
        sync_tested_by_with_calls(&tx)?;
        reconcile_tested_by_with_calls(&tx)?;
        tx.commit()?;
        Ok(resolved + re_exported)
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
                <(i64, String, String, String, Option<String>)>::try_from(row)
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
            if let Some((qualified, confidence, tier)) = resolved_target {
                tx.execute(
                    "UPDATE edges SET target_qualified = ?, target_name = ?, \
                     confidence = ?, confidence_tier = ? WHERE id = ?",
                    params![
                        qualified,
                        edge_target_name(&qualified),
                        confidence,
                        tier.as_str(),
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
