use super::*;

/// UniFFI's foreign-language name for a Rust function: `fast_sum` ->
/// `fastSum` (Kotlin and Swift use lowerCamelCase).
fn lower_camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for ch in name.chars() {
        if ch == '_' {
            upper = !out.is_empty();
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Kotlin / Swift using UniFFI bindings: a file importing the crate's
/// Kotlin package (`uniffi.<namespace>` or below) or Swift module ->
/// the crate root (`loads_native_module`), and its bare calls to a
/// function's lowerCamelCase name or a type's name -> the Rust item
/// (`calls_native_function`, `bridge_kind: uniffi`).
pub(super) fn bind_uniffi(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    if crates.iter().all(|krate| krate.uniffi.is_none()) {
        return Ok(());
    }
    // Foreign name -> Rust item, per crate.
    let names: Vec<HashMap<String, Vec<String>>> = exports
        .iter()
        .map(|table| {
            let mut names: HashMap<String, Vec<String>> = HashMap::new();
            for (name, items) in &table.uniffi_functions {
                names
                    .entry(lower_camel_case(name))
                    .or_default()
                    .extend(items.iter().cloned());
            }
            for (name, items) in &table.uniffi_types {
                names
                    .entry(name.clone())
                    .or_default()
                    .extend(items.iter().cloned());
            }
            names
        })
        .collect();
    let mut stmt = tx.prepare(
        "SELECT source_qualified, target_qualified, file_path, line FROM edges \
         WHERE kind = 'IMPORTS_FROM' \
           AND (file_path LIKE '%.kt' OR file_path LIKE '%.kts' OR file_path LIKE '%.swift')",
    )?;
    let rows = stmt.query_map([], |row| <(String, String, String, i64)>::try_from(row))?;
    let mut importing: HashMap<(String, &'static str), HashSet<usize>> = HashMap::new();
    for row in rows {
        let (source, target, file_path, line) = row?;
        let language = if file_path.ends_with(".swift") {
            "swift"
        } else {
            "kotlin"
        };
        for (index, krate) in crates.iter().enumerate() {
            let Some(uniffi) = &krate.uniffi else {
                continue;
            };
            let matched = if language == "swift" {
                target == uniffi.swift_module
            } else {
                target == uniffi.kotlin_package
                    || target
                        .strip_prefix(uniffi.kotlin_package.as_str())
                        .is_some_and(|rest| rest.starts_with('.'))
            };
            if !matched {
                continue;
            }
            if importing
                .entry((file_path.clone(), language))
                .or_default()
                .insert(index)
            {
                bridges.push(NewBridge {
                    source: source.clone(),
                    target: krate.root.clone(),
                    file_path: file_path.clone(),
                    line,
                    extra: bridge_extra(
                        "loads_native_module",
                        "uniffi",
                        "manifest",
                        format!("import {target}"),
                        language,
                        krate.language,
                    ),
                });
            }
        }
    }
    for ((file_path, language), indexes) in importing {
        for (caller, target, line, _) in calls_in_file(tx, &file_path)? {
            if target.contains("::") {
                continue;
            }
            let hits: Vec<(&String, usize)> = indexes
                .iter()
                .filter_map(|index| unique(names[*index].get(&target)).map(|qn| (qn, *index)))
                .collect();
            let [(item, index)] = hits.as_slice() else {
                continue;
            };
            bridges.push(NewBridge {
                source: caller,
                target: (*item).clone(),
                file_path: file_path.clone(),
                line,
                extra: bridge_extra(
                    "calls_native_function",
                    "uniffi",
                    "syntax",
                    format!("uniffi {target}"),
                    language,
                    crates[*index].language,
                ),
            });
        }
    }
    Ok(())
}
