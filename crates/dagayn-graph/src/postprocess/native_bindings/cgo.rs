use super::*;

/// cgo, which compiles the C files of a Go package directory with it:
///
/// * `C.f(...)` in a Go file -> the C-ABI function `f`, looked for among
///   the C files of the package directory, then the sources of the
///   libraries `#cgo LDFLAGS: -lNAME` links, then the whole repository;
///   the first scope with a match must hold exactly one;
/// * a C call in the package directory to a function a Go `//export`
///   declares -> that Go function.
pub(super) fn bind_cgo(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    let go_files: Vec<(String, Option<String>)> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT e.file_path, n.extra FROM edges e \
             LEFT JOIN nodes n ON n.kind = 'File' AND n.file_path = e.file_path \
             WHERE e.kind = 'IMPORTS_FROM' AND e.target_qualified = 'C' \
               AND e.file_path LIKE '%.go'",
        )?;
        let rows = stmt.query_map([], |row| <(String, Option<String>)>::try_from(row))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    if go_files.is_empty() {
        return Ok(());
    }
    let dir_of = |path: &str| path.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
    // symbol -> (qualified name, file, language) for C-ABI exports.
    let mut c_exports: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    let mut go_exports: HashMap<(String, String), Vec<String>> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, file_path, language, json_extract(extra, '$.ffi_export.name') \
             FROM nodes WHERE json_extract(extra, '$.ffi_export.abi') = 'c'",
        )?;
        let rows = stmt.query_map([], |row| {
            <(String, String, Option<String>, String)>::try_from(row)
        })?;
        for row in rows {
            let (qualified, file_path, language, symbol) = row?;
            let language = language.unwrap_or_else(|| "c".to_string());
            if language == "go" {
                go_exports
                    .entry((dir_of(&file_path), symbol))
                    .or_default()
                    .push(qualified);
            } else {
                c_exports
                    .entry(symbol)
                    .or_default()
                    .push((qualified, file_path, language));
            }
        }
    }
    let libraries = load_native_libraries(tx)?;
    let mut c_dirs: HashSet<String> = HashSet::new();
    for (go_file, extra) in &go_files {
        let package_dir = dir_of(go_file);
        c_dirs.insert(package_dir.clone());
        let linked: HashSet<&String> = parse_json_column(extra.clone())?
            .get("cgo_libraries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .flat_map(|name| {
                let name = name.replace('-', "_");
                libraries
                    .iter()
                    .filter(move |lib| lib.lib_name == name)
                    .flat_map(|lib| &lib.sources)
            })
            .collect();
        for (caller, name, line, receiver) in calls_in_file(tx, go_file)? {
            if receiver.as_deref() != Some("C") {
                continue;
            }
            let Some(candidates) = c_exports.get(&name) else {
                continue;
            };
            // Package directory, then `-l` libraries, then anywhere.
            let scope = |evidence: &'static str, in_scope: &dyn Fn(&String) -> bool| {
                let hits: Vec<&(String, String, String)> = candidates
                    .iter()
                    .filter(|(_, file, _)| in_scope(file))
                    .collect();
                (!hits.is_empty()).then_some((evidence, hits))
            };
            let Some((evidence, hits)) = scope("cgo package", &|file| dir_of(file) == package_dir)
                .or_else(|| scope("cgo LDFLAGS", &|file| linked.contains(file)))
                .or_else(|| scope("symbol", &|_| true))
            else {
                continue;
            };
            let [(target, _, target_language)] = hits.as_slice() else {
                continue;
            };
            bridges.push(NewBridge {
                source: caller,
                target: target.clone(),
                file_path: go_file.clone(),
                line,
                extra: bridge_extra(
                    "calls_native_function",
                    "ffi",
                    if evidence == "symbol" {
                        "syntax"
                    } else {
                        "manifest"
                    },
                    format!("{evidence} {name}"),
                    "go",
                    target_language,
                ),
            });
        }
    }
    // C calling Go: C files in a cgo package directory.
    let mut c_files = tx.prepare_cached(
        "SELECT DISTINCT file_path FROM nodes WHERE kind = 'File' \
           AND language IN ('c', 'cpp', 'objc') AND file_path LIKE ? || '%'",
    )?;
    for dir in &c_dirs {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let files = c_files
            .query_map([&prefix], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for c_file in files.iter().filter(|file| dir_of(file) == *dir) {
            let language = if c_file.ends_with(".c") { "c" } else { "cpp" };
            for (caller, target, line, _) in calls_in_file(tx, c_file)? {
                let name = call_name(&target).to_string();
                let Some(go_function) = unique(go_exports.get(&(dir.clone(), name.clone()))) else {
                    continue;
                };
                bridges.push(NewBridge {
                    source: caller,
                    target: go_function.clone(),
                    file_path: c_file.clone(),
                    line,
                    extra: bridge_extra(
                        "calls_native_function",
                        "ffi",
                        "manifest",
                        format!("cgo //export {name}"),
                        language,
                        "go",
                    ),
                });
            }
        }
    }
    Ok(())
}
