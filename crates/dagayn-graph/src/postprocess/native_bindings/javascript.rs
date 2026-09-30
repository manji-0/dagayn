use super::*;

/// JavaScript / TypeScript imports of a wasm-bindgen or Node.js addon
/// crate, and the calls the extractor qualified as `specifier::name` into
/// it. Returns the files that import a Node.js addon or Emscripten glue,
/// whose other calls [`bind_namespace_calls`] matches by name.
pub(super) fn bind_js_modules(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    bridges: &mut Vec<NewBridge>,
) -> Result<HashMap<String, HashSet<usize>>> {
    let mut module_files: HashMap<String, HashSet<usize>> = HashMap::new();
    if crates.iter().all(|krate| {
        krate.js_packages.is_empty() && krate.wasm_outputs.is_empty() && krate.node_addon.is_none()
    }) {
        return Ok(module_files);
    }
    let mut stmt = tx.prepare(
        "SELECT kind, source_qualified, target_qualified, file_path, line FROM edges \
         WHERE kind IN ('IMPORTS_FROM', 'CALLS') ORDER BY file_path, line",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut linked: HashSet<(String, usize)> = HashSet::new();
    for row in rows {
        let (kind, source, target, file_path, line) = row?;
        let Some(language) = javascript_language(&file_path) else {
            continue;
        };
        if kind == "IMPORTS_FROM" {
            let Some(index) = js_crate_for(crates, &file_path, &target) else {
                continue;
            };
            if crates[index].node_addon.is_some() || crates[index].emscripten.is_some() {
                module_files
                    .entry(file_path.clone())
                    .or_default()
                    .insert(index);
            }
            if linked.insert((file_path.clone(), index)) {
                bridges.push(NewBridge {
                    source,
                    target: crates[index].root.clone(),
                    file_path,
                    line,
                    extra: bridge_extra(
                        "loads_native_module",
                        crates[index].js_bridge_kind(),
                        "manifest",
                        format!("import {target}"),
                        language,
                        crates[index].language,
                    ),
                });
            }
            continue;
        }
        let Some((spec, path)) = target.split_once("::") else {
            continue;
        };
        let Some(index) = js_crate_for(crates, &file_path, spec) else {
            continue;
        };
        let name = path.split('.').next().unwrap_or(path);
        let Some(export) = unique(exports[index].js.get(name)) else {
            continue;
        };
        bridges.push(NewBridge {
            source,
            target: export.clone(),
            file_path,
            line,
            extra: bridge_extra(
                "calls_native_function",
                crates[index].js_bridge_kind(),
                "syntax",
                format!("export {name}"),
                language,
                crates[index].language,
            ),
        });
    }
    Ok(module_files)
}

/// `require("bindings")("addon")` (`loads_node_addon`) -> the node-gyp
/// target of that name.
pub(super) fn bind_node_addon_loaders(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    module_files: &mut HashMap<String, HashSet<usize>>,
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    let mut stmt = tx.prepare(
        "SELECT source_qualified, target_qualified, file_path, line FROM edges \
         WHERE kind = 'CROSS_ARTIFACT' \
           AND json_extract(extra, '$.relationship_role') = 'loads_node_addon'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (source, target, file_path, line) = row?;
        let name = target.strip_suffix(".node").unwrap_or(&target);
        let matches: Vec<usize> = crates
            .iter()
            .enumerate()
            .filter(|(_, krate)| krate.node_addon.is_some() && krate.lib_name == name)
            .map(|(index, _)| index)
            .collect();
        let [index] = matches.as_slice() else {
            continue;
        };
        module_files
            .entry(file_path.clone())
            .or_default()
            .insert(*index);
        bridges.push(NewBridge {
            source,
            target: crates[*index].root.clone(),
            file_path: file_path.clone(),
            line,
            extra: bridge_extra(
                "loads_native_module",
                "node_addon",
                "config",
                format!("{} target {name}", crates[*index].build_system),
                javascript_language(&file_path).unwrap_or("javascript"),
                crates[*index].language,
            ),
        });
    }
    Ok(())
}

/// In a file that loads a Node.js addon or Emscripten glue, a call the
/// extractor left as a bare name (`addon.hello()` on a CommonJS `require`,
/// `Module._add()`, whose receiver it cannot type) that exactly one of the
/// loaded modules exports.
pub(super) fn bind_namespace_calls(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    module_files: &HashMap<String, HashSet<usize>>,
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    for (file_path, indexes) in module_files {
        let language = javascript_language(file_path).unwrap_or("javascript");
        for (caller, target, line, _) in calls_in_file(tx, file_path)? {
            if target.contains("::") {
                continue;
            }
            let hits: Vec<(&String, usize)> = indexes
                .iter()
                .filter_map(|index| unique(exports[*index].js.get(&target)).map(|qn| (qn, *index)))
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
                    crates[*index].js_bridge_kind(),
                    "syntax",
                    format!("export {target}"),
                    language,
                    crates[*index].language,
                ),
            });
        }
    }
    Ok(())
}

/// A call to an ambient `declare function name` in JavaScript / TypeScript,
/// when exactly one Go module defines the global `name`.
pub(super) fn bind_js_globals(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    if exports.iter().all(|table| table.js_global.is_empty()) {
        return Ok(());
    }
    let mut stmt = tx.prepare(
        "SELECT e.source_qualified, e.file_path, e.line, t.name FROM edges e \
         JOIN nodes t ON t.qualified_name = e.target_qualified \
         WHERE e.kind = 'CALLS' AND t.extra LIKE '%\"ambient\":true%'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (caller, file_path, line, name) = row?;
        let Some(language) = javascript_language(&file_path) else {
            continue;
        };
        let hits: Vec<(&String, usize)> = exports
            .iter()
            .enumerate()
            .filter_map(|(index, table)| unique(table.js_global.get(&name)).map(|qn| (qn, index)))
            .collect();
        let [(export, index)] = hits.as_slice() else {
            continue;
        };
        bridges.push(NewBridge {
            source: caller,
            target: (*export).clone(),
            file_path,
            line,
            extra: bridge_extra(
                "calls_native_function",
                "wasm",
                "syntax",
                format!("js global {name}"),
                language,
                crates[*index].language,
            ),
        });
    }
    Ok(())
}
