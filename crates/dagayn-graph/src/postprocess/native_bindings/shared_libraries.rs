use super::*;

/// `ctypes.CDLL("…/libNAME.so")` and friends -> the cdylib crate named NAME,
/// then calls in the same file to the crate's C symbols.
pub(super) fn bind_shared_libraries(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    let loaders = {
        let mut stmt = tx.prepare(
            "SELECT source_qualified, target_qualified, file_path, line, extra FROM edges \
             WHERE kind = 'CROSS_ARTIFACT' \
               AND json_extract(extra, '$.relationship_role') = 'loads_shared_library' \
               AND COALESCE(json_extract(extra, '$.extractor'), '') != ?",
        )?;
        let rows = stmt.query_map([NATIVE_BINDINGS_EXTRACTOR], |row| {
            <(String, String, String, i64, Option<String>)>::try_from(row)
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    // File -> crates it loads, for loaders that look symbols up by the name
    // they call: Python `ctypes` / cffi (`lib.fast_sum(...)`), Deno / Bun
    // (`lib.symbols.fast_sum(...)`), LuaJIT (`lib.fast_sum(...)`). A Java
    // file that loads a library calls `native` methods whose C symbols are
    // `Java_<class>_<method>`, so a bare-name match there is a coincidence.
    let mut loaded: HashMap<String, HashSet<(usize, String)>> = HashMap::new();
    for (source, target, file_path, line, extra) in loaders {
        let Some(stem) = shared_library_stem(&target) else {
            continue;
        };
        let matches: Vec<usize> = crates
            .iter()
            .enumerate()
            .filter(|(_, krate)| krate.cdylib && krate.lib_name == stem)
            .map(|(index, _)| index)
            .collect();
        let [index] = matches.as_slice() else {
            continue;
        };
        let loader_extra = parse_json_column(extra)?;
        let source_language = loader_extra
            .get("source_language")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let krate = &crates[*index];
        // A declaration that names its symbol (C# `[DllImport("lib",
        // EntryPoint = "sym")]`) binds straight to the native function.
        if let Some(symbol) = loader_extra.get("symbol").and_then(Value::as_str)
            && let Some(function) = unique(exports[*index].c.get(symbol))
        {
            bridges.push(NewBridge {
                source: source.clone(),
                target: function.clone(),
                file_path: file_path.clone(),
                line,
                extra: bridge_extra(
                    "calls_native_function",
                    "ffi",
                    "syntax",
                    format!("{}::{symbol}", krate.lib_name),
                    &source_language,
                    krate.language,
                ),
            });
        }
        let mut extra = bridge_extra(
            "loads_shared_library",
            "ffi",
            "manifest",
            format!("{} lib name {}", krate.build_system, krate.lib_name),
            &source_language,
            krate.language,
        );
        extra["library"] = Value::String(target);
        bridges.push(NewBridge {
            source,
            target: krate.root.clone(),
            file_path: file_path.clone(),
            line,
            extra,
        });
        if matches!(
            source_language.as_str(),
            "python" | "javascript" | "typescript" | "lua"
        ) {
            loaded
                .entry(file_path)
                .or_default()
                .insert((*index, source_language));
        }
    }
    for (file_path, indexes) in loaded {
        for (caller, target, line, _) in calls_in_file(tx, &file_path)? {
            let name = call_name(&target);
            let hits: Vec<(&String, usize, &str)> = indexes
                .iter()
                .filter_map(|(index, language)| {
                    unique(exports[*index].c.get(name)).map(|qn| (qn, *index, language.as_str()))
                })
                .collect();
            let [(target, index, language)] = hits.as_slice() else {
                continue;
            };
            let extra = bridge_extra(
                "calls_native_function",
                "ffi",
                "syntax",
                format!("{}::{name}", crates[*index].lib_name),
                language,
                crates[*index].language,
            );
            bridges.push(NewBridge {
                source: caller,
                target: (*target).clone(),
                file_path: file_path.clone(),
                line,
                extra,
            });
        }
    }
    Ok(())
}
