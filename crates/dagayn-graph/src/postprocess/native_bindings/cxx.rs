use super::*;

/// cxx bridges, within the sources the crate's `build.rs` compiles with
/// `cxx_build` (C++ names are mangled, so nothing outside counts):
///
/// * `unsafe extern "C++" { fn f(); }` -> the free C++ function `f`, and
///   `fn m(self: Pin<&mut T>)` -> the method `T::m`;
/// * `extern "Rust" { fn g(); }` -> C++ calls to `g` reach the Rust
///   function `g` of the crate.
pub(super) fn bind_cxx_bridges(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    let declarations = {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, file_path, line_start, \
                    json_extract(extra, '$.ffi_import.name'), \
                    json_extract(extra, '$.ffi_import.class'), \
                    json_extract(extra, '$.ffi_export.name') \
             FROM nodes WHERE json_extract(extra, '$.ffi_import.abi') = 'cxx' \
                OR json_extract(extra, '$.ffi_export.abi') = 'cxx'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    if declarations.is_empty() {
        return Ok(());
    }
    let libraries = load_native_libraries(tx)?;
    let mut functions_named = tx.prepare_cached(
        "SELECT qualified_name, file_path, parent_name, extra FROM nodes \
         WHERE name = ? AND kind = 'Function'",
    )?;
    for (declaration, file_path, line, import, class, export) in declarations {
        let sources = crate_linked_sources(&libraries, &file_path, &["cxx"]);
        if sources.is_empty() {
            continue;
        }
        let Some(name) = import.clone().or(export) else {
            continue;
        };
        let named = functions_named
            .query_map([&name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if import.is_some() {
            let hits: Vec<&String> = named
                .iter()
                .filter(|(_, file, parent, _)| sources.contains(file) && *parent == class)
                .map(|(qualified, _, _, _)| qualified)
                .collect();
            let [target] = hits.as_slice() else {
                continue;
            };
            bridges.push(NewBridge {
                source: declaration,
                target: (*target).clone(),
                file_path,
                line,
                extra: bridge_extra(
                    "calls_native_function",
                    "cxx",
                    "manifest",
                    format!("cxx {name}"),
                    "rust",
                    "cpp",
                ),
            });
            continue;
        }
        // `extern "Rust"`: the implementation is a Rust function of the
        // same name in the crate, outside the bridge module.
        let crate_dir = libraries
            .iter()
            .filter(|lib| lib.build_system == "cxx" && path_under(&lib.dir, &file_path))
            .map(|lib| lib.dir.as_str())
            .max_by_key(|dir| dir.len())
            .unwrap_or_default();
        let implementations: Vec<&String> = named
            .iter()
            .filter(|(qualified, file, _, extra)| {
                *qualified != declaration
                    && file.ends_with(".rs")
                    && path_under(crate_dir, file)
                    && !extra.as_deref().is_some_and(|extra| {
                        extra.contains("\"is_abstract\":true") || extra.contains("ffi_export")
                    })
            })
            .map(|(qualified, _, _, _)| qualified)
            .collect();
        let [implementation] = implementations.as_slice() else {
            continue;
        };
        for source_file in &sources {
            for (caller, target, call_line, _) in calls_in_file(tx, source_file)? {
                if call_name(&target) != name {
                    continue;
                }
                bridges.push(NewBridge {
                    source: caller,
                    target: (*implementation).clone(),
                    file_path: (*source_file).clone(),
                    line: call_line,
                    extra: bridge_extra(
                        "calls_native_function",
                        "cxx",
                        "manifest",
                        format!("cxx extern \"Rust\" {name}"),
                        "cpp",
                        "rust",
                    ),
                });
            }
        }
    }
    Ok(())
}
