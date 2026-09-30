use super::*;

/// Python import bindings to extension modules, per file, plus one
/// file-level `loads_native_module` bridge per imported crate.
pub(super) fn bind_extension_imports(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    bridges: &mut Vec<NewBridge>,
) -> Result<HashMap<String, HashMap<String, Binding>>> {
    let mut bindings: HashMap<String, HashMap<String, Binding>> = HashMap::new();
    let modules: Vec<(usize, &str)> = crates
        .iter()
        .enumerate()
        .filter_map(|(index, krate)| krate.python_module.as_deref().map(|m| (index, m)))
        .collect();
    if modules.is_empty() {
        return Ok(bindings);
    }
    let mut stmt = tx.prepare(
        "SELECT file_path, line, extra FROM edges \
         WHERE kind = 'IMPORTS_FROM' AND extra LIKE '%\"module\"%' ORDER BY file_path, line",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut linked: HashSet<(String, usize)> = HashSet::new();
    for row in rows {
        let (file_path, line, extra) = row?;
        let extra = parse_json_column(extra)?;
        let Some(module) = extra.get("module").and_then(Value::as_str) else {
            continue;
        };
        let module = absolute_module(module, &file_path);
        let names: Vec<(String, String)> = extra
            .get("names")
            .and_then(Value::as_array)
            .map(|pairs| {
                pairs
                    .iter()
                    .filter_map(|pair| {
                        Some((
                            pair.get(0)?.as_str()?.to_string(),
                            pair.get(1)?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let alias = extra.get("alias").and_then(Value::as_str);
        for &(index, python_module) in &modules {
            let mut hit = false;
            let file_bindings = bindings.entry(file_path.clone()).or_default();
            if module_matches(&module, python_module) {
                hit = true;
                for (name, local) in &names {
                    file_bindings.insert(local.clone(), Binding::Symbol(index, name.clone()));
                }
                if let Some(alias) = alias {
                    file_bindings.insert(alias.to_string(), Binding::Module(index));
                }
            } else {
                for (name, local) in &names {
                    if module_matches(&format!("{module}.{name}"), python_module) {
                        hit = true;
                        file_bindings.insert(local.clone(), Binding::Module(index));
                    }
                }
            }
            if hit && linked.insert((file_path.clone(), index)) {
                bridges.push(NewBridge {
                    source: file_path.clone(),
                    target: crates[index].root.clone(),
                    file_path: file_path.clone(),
                    line,
                    extra: bridge_extra(
                        "loads_native_module",
                        "extension_module",
                        "manifest",
                        format!("import {python_module}"),
                        "python",
                        crates[index].language,
                    ),
                });
            }
        }
    }
    bindings.retain(|_, file_bindings| !file_bindings.is_empty());
    Ok(bindings)
}

pub(super) fn bind_extension_calls(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    bindings: &HashMap<String, HashMap<String, Binding>>,
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    for (file_path, file_bindings) in bindings {
        for (caller, target, line, receiver) in calls_in_file(tx, file_path)? {
            let name = call_name(&target);
            let resolved = match receiver.as_deref() {
                Some(receiver) => match file_bindings.get(receiver) {
                    Some(Binding::Module(index)) => Some((*index, name.to_string())),
                    _ => None,
                },
                None => match file_bindings.get(name) {
                    Some(Binding::Symbol(index, exported)) => Some((*index, exported.clone())),
                    _ => None,
                },
            };
            let Some((index, exported)) = resolved else {
                continue;
            };
            let Some(target) = unique(exports[index].python.get(&exported)) else {
                continue;
            };
            let module = crates[index].python_module.as_deref().unwrap_or_default();
            bridges.push(NewBridge {
                source: caller,
                target: target.clone(),
                file_path: file_path.clone(),
                line,
                extra: bridge_extra(
                    "calls_native_function",
                    "extension_module",
                    "syntax",
                    format!("{module}.{exported}"),
                    "python",
                    crates[index].language,
                ),
            });
        }
    }
    Ok(())
}
