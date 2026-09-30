use super::*;

/// `fetch("app.wasm")` and friends (`loads_wasm_module`) -> the crate whose
/// build writes that file; then calls in the loading file to its exports
/// (`instance.exports.add(...)`) or to the globals a Go module defines.
pub(super) fn bind_wasm_loaders(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    bridges: &mut Vec<NewBridge>,
) -> Result<HashMap<String, HashSet<usize>>> {
    let mut loaded: HashMap<String, HashSet<usize>> = HashMap::new();
    if crates.iter().all(|krate| krate.wasm_outputs.is_empty()) {
        return Ok(loaded);
    }
    let loaders = {
        let mut stmt = tx.prepare(
            "SELECT source_qualified, target_qualified, file_path, line, \
                    json_extract(extra, '$.source_language') FROM edges \
             WHERE kind = 'CROSS_ARTIFACT' \
               AND json_extract(extra, '$.relationship_role') = 'loads_wasm_module' \
               AND COALESCE(json_extract(extra, '$.extractor'), '') != ?",
        )?;
        let rows = stmt.query_map([NATIVE_BINDINGS_EXTRACTOR], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (source, target, file_path, line, source_language) in loaders {
        let Some(index) = wasm_output_crate(crates, &file_path, &target) else {
            continue;
        };
        let language = source_language.as_deref().unwrap_or("unknown");
        let mut extra = bridge_extra(
            "loads_native_module",
            "wasm",
            "config",
            format!("wasm output {target}"),
            language,
            crates[index].language,
        );
        extra["module"] = Value::String(target);
        bridges.push(NewBridge {
            source,
            target: crates[index].root.clone(),
            file_path: file_path.clone(),
            line,
            extra,
        });
        loaded.entry(file_path).or_default().insert(index);
    }
    for (file_path, indexes) in &loaded {
        // `instance.exports.add(...)` is how JavaScript calls an export; a
        // host in another language looks exports up by name
        // (`calls_wasm_export`) instead.
        let Some(language) = javascript_language(file_path) else {
            continue;
        };
        for (caller, target, line, _) in calls_in_file(tx, file_path)? {
            let name = call_name(&target);
            let hits: Vec<(&String, usize)> = indexes
                .iter()
                .filter_map(|index| {
                    unique(exports[*index].js.get(name))
                        .or_else(|| unique(exports[*index].js_global.get(name)))
                        .map(|qn| (qn, *index))
                })
                .collect();
            let [(export, index)] = hits.as_slice() else {
                continue;
            };
            bridges.push(NewBridge {
                source: caller,
                target: (*export).clone(),
                file_path: file_path.clone(),
                line,
                extra: bridge_extra(
                    "calls_native_function",
                    "wasm",
                    "syntax",
                    format!("wasm export {name}"),
                    language,
                    crates[*index].language,
                ),
            });
        }
    }
    Ok(loaded)
}

/// An export looked up by name (`calls_wasm_export`) in a file that loads
/// the module -> the function behind it: Emscripten's `ccall("add")` /
/// `cwrap("add")` (the C function behind `_add`), and a WebAssembly host's
/// `get_typed_func(&mut store, "add")` / `ExportedFunction("add")` / ... (a
/// Rust `#[no_mangle]` function, Go `//go:wasmexport`, or AssemblyScript
/// `export`).
pub(super) fn bind_wasm_export_calls(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    loaded: [&HashMap<String, HashSet<usize>>; 2],
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    let mut stmt = tx.prepare(
        "SELECT source_qualified, target_qualified, file_path, line, \
                json_extract(extra, '$.source_language') FROM edges \
         WHERE kind = 'CROSS_ARTIFACT' \
           AND json_extract(extra, '$.relationship_role') = 'calls_wasm_export'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    for row in rows {
        let (source, name, file_path, line, language) = row?;
        let indexes: HashSet<usize> = loaded
            .iter()
            .filter_map(|files| files.get(&file_path))
            .flatten()
            .copied()
            .collect();
        let hits: Vec<(&String, usize)> = indexes
            .iter()
            .filter_map(|index| {
                let table = &exports[*index];
                if crates[*index].emscripten.is_some() {
                    unique(table.js.get(&format!("_{name}")))
                } else {
                    unique(table.js.get(&name)).or_else(|| unique(table.c.get(&name)))
                }
                .map(|qn| (qn, *index))
            })
            .collect();
        let [(export, index)] = hits.as_slice() else {
            continue;
        };
        bridges.push(NewBridge {
            source,
            target: (*export).clone(),
            file_path: file_path.clone(),
            line,
            extra: bridge_extra(
                "calls_native_function",
                "wasm",
                "syntax",
                format!("export {name}"),
                language.as_deref().unwrap_or("unknown"),
                crates[*index].language,
            ),
        });
    }
    Ok(())
}

/// WebAssembly code calling into JavaScript: a foreign declaration -> the
/// JavaScript function that implements it (`wraps_foreign_api`).
///
/// * Rust `#[wasm_bindgen(module = "/js/util.js")] extern "C" { fn f(); }`:
///   the function `f` (or `js_name`) of that file, a path from the crate
///   root (the nearest directory with a `Cargo.toml`).
/// * Go `//go:wasmimport env f`: a JavaScript function `f` defined in an
///   object literal under the key `env` (the import object passed to
///   `WebAssembly.instantiate`), when exactly one exists.
pub(super) fn bind_wasm_imports(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    let imports = {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, file_path, line_start, language, \
                    json_extract(extra, '$.ffi_import.abi'), \
                    json_extract(extra, '$.ffi_import.module'), \
                    json_extract(extra, '$.ffi_import.name') \
             FROM nodes WHERE json_extract(extra, '$.ffi_import.abi') IN ('wasm', 'wasmimport')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    if imports.is_empty() {
        return Ok(());
    }
    let cargo_dirs: HashSet<String> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT file_path FROM nodes \
             WHERE file_path = 'Cargo.toml' OR file_path LIKE '%/Cargo.toml'",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            row.map(|path| {
                path.strip_suffix("Cargo.toml")
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string()
            })
        })
        .collect::<std::result::Result<_, _>>()?
    };
    let mut in_file = tx.prepare_cached(
        "SELECT qualified_name FROM nodes \
         WHERE file_path = ? AND name = ? AND kind = 'Function' AND parent_name IS NULL",
    )?;
    let mut in_object = tx.prepare_cached(
        "SELECT qualified_name, file_path FROM nodes \
         WHERE name = ? AND kind = 'Function' \
           AND (parent_name = ? OR parent_name LIKE '%.' || ?)",
    )?;
    for (source, file_path, line, language, abi, module, name) in imports {
        let (Some(module), Some(name)) = (module, name) else {
            continue;
        };
        let targets: Vec<(String, String)> = if abi == "wasm" {
            let Some(rel) = module.strip_prefix('/') else {
                continue;
            };
            let Some(crate_dir) = nearest_dir(&file_path, &cargo_dirs) else {
                continue;
            };
            let js_file = if crate_dir.is_empty() {
                rel.to_string()
            } else {
                format!("{crate_dir}/{rel}")
            };
            let rows = in_file.query_map(params![js_file, name], |row| row.get::<_, String>(0))?;
            rows.map(|row| row.map(|qualified| (qualified, js_file.clone())))
                .collect::<std::result::Result<_, _>>()?
        } else {
            let rows = in_object.query_map(params![name, module, module], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|(_, path)| javascript_language(path).is_some())
                .collect()
        };
        let [(target, target_file)] = targets.as_slice() else {
            continue;
        };
        bridges.push(NewBridge {
            source,
            target: target.clone(),
            file_path,
            line,
            extra: bridge_extra(
                "wraps_foreign_api",
                "wasm",
                "syntax",
                format!("{module}::{name}"),
                language.as_deref().unwrap_or("unknown"),
                javascript_language(target_file).unwrap_or("javascript"),
            ),
        });
    }
    Ok(())
}
