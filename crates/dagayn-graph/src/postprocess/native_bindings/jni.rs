use super::*;

/// Java `native` / Kotlin `external` methods -> the C-ABI function their JNI
/// name spells (`Java_com_example_Sum_fastSum`, or an overload's
/// `Java_..._fastSum__<signature>`), in C, C++ `extern "C"`, or Rust
/// `#[no_mangle]`. The name encodes package, class, and method, so it is
/// matched across the whole repository rather than inside a library the
/// class is known to load.
pub(super) fn bind_jni_methods(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    let mut exports: HashMap<String, Vec<(String, String)>> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, language, json_extract(extra, '$.ffi_export.name') \
             FROM nodes \
             WHERE json_extract(extra, '$.ffi_export.abi') = 'c' \
               AND json_extract(extra, '$.ffi_export.name') LIKE 'Java\\_%' ESCAPE '\\'",
        )?;
        let rows = stmt.query_map([], |row| <(String, Option<String>, String)>::try_from(row))?;
        for row in rows {
            let (qualified, language, symbol) = row?;
            exports
                .entry(symbol)
                .or_default()
                .push((qualified, language.unwrap_or_else(|| "c".to_string())));
        }
    }
    if exports.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare(
        "SELECT qualified_name, file_path, line_start, language, \
                json_extract(extra, '$.ffi_import.symbol') \
         FROM nodes WHERE json_extract(extra, '$.ffi_import.abi') = 'jni'",
    )?;
    let rows = stmt.query_map([], |row| {
        <(String, String, i64, Option<String>, Option<String>)>::try_from(row)
    })?;
    for row in rows {
        let (qualified, file_path, line, language, symbol) = row?;
        let Some(symbol) = symbol else {
            continue;
        };
        let overload_prefix = format!("{symbol}__");
        let candidates: Vec<&(String, String)> = match exports.get(&symbol) {
            Some(exact) => exact.iter().collect(),
            None => exports
                .iter()
                .filter(|(name, _)| name.starts_with(&overload_prefix))
                .flat_map(|(_, targets)| targets)
                .collect(),
        };
        let [(target, target_language)] = candidates.as_slice() else {
            continue;
        };
        bridges.push(NewBridge {
            source: qualified,
            target: target.clone(),
            file_path,
            line,
            extra: bridge_extra(
                "calls_native_function",
                "jni",
                "syntax",
                symbol,
                language.as_deref().unwrap_or("java"),
                target_language,
            ),
        });
    }
    Ok(())
}
