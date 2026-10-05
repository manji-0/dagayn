use super::*;

/// A declaration bound to a C symbol (`ffi_import.abi = "c"`: Rust
/// `extern "C" { fn f(); }`, Zig `extern fn f`, Dart
/// `@Native(symbol: "f")`), or a Zig call through `@cImport` -> the C-ABI
/// function `f` (`calls_native_function`). The symbol is looked for, in
/// order, among the C sources linked into the same crate or Zig build
/// (`build.rs` `cc` / `cxx_build`, `build.zig` `addCSourceFile(s)`), the
/// sources of the library `#[link(name = "...")]` / `extern "lib"` names,
/// and the whole repository; the first scope with a match must hold
/// exactly one.
pub(super) fn bind_c_imports(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    let imports = {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, file_path, line_start, \
                    json_extract(extra, '$.ffi_import.name'), \
                    json_extract(extra, '$.ffi_import.library'), language \
             FROM nodes WHERE json_extract(extra, '$.ffi_import.abi') = 'c'",
        )?;
        let rows = stmt.query_map([], |row| {
            <(
                String,
                String,
                i64,
                Option<String>,
                Option<String>,
                Option<String>,
            )>::try_from(row)
        })?;
        let mut imports = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        // Zig calls through `const c = @cImport(...)`: `c.fast_sum(...)`.
        let mut calls = tx.prepare(
            "SELECT source_qualified, file_path, line, COALESCE(target_name, target_qualified) \
             FROM edges WHERE kind = 'CALLS' AND json_extract(extra, '$.c_import') = 1",
        )?;
        let rows = calls.query_map([], |row| <(String, String, i64, String)>::try_from(row))?;
        for row in rows {
            let (caller, file_path, line, target) = row?;
            let name = target.rsplit('.').next().unwrap_or(&target).to_string();
            imports.push((
                caller,
                file_path,
                line,
                Some(name),
                None,
                Some("zig".to_string()),
            ));
        }
        imports
    };
    if imports.is_empty() {
        return Ok(());
    }
    // symbol -> (qualified name, file, language)
    let mut exports: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
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
            exports.entry(symbol).or_default().push((
                qualified,
                file_path,
                language.unwrap_or_else(|| "c".to_string()),
            ));
        }
    }
    let libraries = load_native_libraries(tx)?;
    for (source, file_path, line, name, library, language) in imports {
        let Some(name) = name else {
            continue;
        };
        let Some(candidates) = exports.get(&name) else {
            continue;
        };
        // Sources `build.rs` links into this crate: the deepest crate with
        // a `cc` / `cxx_build` library that contains the file.
        let linked = crate_linked_sources(&libraries, &file_path, &["cc", "cxx", "zig-c"]);
        let named: HashSet<&String> = library
            .as_deref()
            .map(|library| {
                // `fastsum`, `libfastsum.so`, and `fast-sum` all name `fastsum`.
                let library =
                    shared_library_stem(library).unwrap_or_else(|| library.replace('-', "_"));
                libraries
                    .iter()
                    .filter(|lib| lib.lib_name == library)
                    .flat_map(|lib| &lib.sources)
                    .collect()
            })
            .unwrap_or_default();
        let scopes: [(&str, Option<&HashSet<&String>>); 3] = [
            ("linked", Some(&linked)),
            ("link", Some(&named)),
            ("symbol", None),
        ];
        let mut chosen = None;
        for (evidence, scope) in scopes {
            let hits: Vec<&(String, String, String)> = candidates
                .iter()
                .filter(|(_, file, _)| scope.is_none_or(|scope| scope.contains(file)))
                .collect();
            if !hits.is_empty() {
                chosen = Some((evidence, hits));
                break;
            }
        }
        let Some((evidence, hits)) = chosen else {
            continue;
        };
        let [(target, _, target_language)] = hits.as_slice() else {
            continue;
        };
        bridges.push(NewBridge {
            source,
            target: target.clone(),
            file_path,
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
                language.as_deref().unwrap_or("rust"),
                target_language,
            ),
        });
    }
    Ok(())
}
