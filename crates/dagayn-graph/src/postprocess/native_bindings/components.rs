use super::*;

/// WebAssembly components, paired through the WIT interface both sides'
/// generated bindings name (`example:calc/ops` is `example::calc::ops` in a
/// guest and `example_calc_ops()` on a wasmtime host):
///
/// * a host's `call_add` (`calls_component_export`) -> the guest's
///   `impl exports::example::calc::ops::Guest` function `add`, matching the
///   interface accessor it is called on when one is written;
/// * a guest's call `example::calc::logging::log(...)` -> the host's
///   `impl example::calc::logging::Host` function `log` (the called path
///   may be a suffix of the interface, as after `use example::calc`).
pub(super) fn bind_components(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    // (interface, name, qualified name) per side.
    let load = |abi: &str| -> Result<Vec<(String, String, String)>> {
        let mut stmt = tx.prepare(
            "SELECT json_extract(extra, '$.ffi_export.interface'), \
                    json_extract(extra, '$.ffi_export.name'), qualified_name \
             FROM nodes WHERE json_extract(extra, '$.ffi_export.abi') = ?",
        )?;
        let rows = stmt.query_map([abi], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    };
    let guest_exports = load("wit")?;
    let host_imports = load("wit_host")?;
    if guest_exports.is_empty() && host_imports.is_empty() {
        return Ok(());
    }
    let component_extra = |role: &str, evidence: String, source: &str| {
        bridge_extra(role, "component", "syntax", evidence, source, "rust")
    };
    if !guest_exports.is_empty() {
        let mut stmt = tx.prepare(
            "SELECT source_qualified, target_qualified, file_path, line, \
                    json_extract(extra, '$.interface_hint') FROM edges \
             WHERE kind = 'CROSS_ARTIFACT' \
               AND json_extract(extra, '$.relationship_role') = 'calls_component_export'",
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
            let (caller, name, file_path, line, hint) = row?;
            let hits: Vec<&(String, String, String)> = guest_exports
                .iter()
                .filter(|(interface, export, _)| {
                    *export == name
                        && hint.as_deref().is_none_or(|hint| {
                            interface.replace("::", "_").replace('-', "_") == hint
                        })
                })
                .collect();
            let [(interface, _, target)] = hits.as_slice() else {
                continue;
            };
            bridges.push(NewBridge {
                source: caller,
                target: target.clone(),
                file_path,
                line,
                extra: component_extra(
                    "calls_native_function",
                    format!("{interface}::{name}"),
                    "rust",
                ),
            });
        }
    }
    if !host_imports.is_empty() {
        let names: HashSet<&str> = host_imports
            .iter()
            .map(|(_, name, _)| name.as_str())
            .collect();
        let mut stmt = tx.prepare(
            "SELECT source_qualified, target_qualified, file_path, line \
             FROM edges WHERE kind = 'CALLS' AND file_path LIKE '%.rs' \
               AND target_qualified LIKE '%::%'",
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
            let (caller, target, file_path, line) = row?;
            let mut segments: Vec<&str> = target.split("::").collect();
            while matches!(segments.first(), Some(&("crate" | "self" | "bindings"))) {
                segments.remove(0);
            }
            let Some(name) = segments.pop() else {
                continue;
            };
            if segments.is_empty() || !names.contains(name) {
                continue;
            }
            let called = segments.join("::");
            let hits: Vec<&(String, String, String)> = host_imports
                .iter()
                .filter(|(interface, import, _)| {
                    import == name
                        && (*interface == called
                            || interface
                                .strip_suffix(called.as_str())
                                .is_some_and(|prefix| prefix.ends_with("::")))
                })
                .collect();
            let [(interface, _, target)] = hits.as_slice() else {
                continue;
            };
            bridges.push(NewBridge {
                source: caller,
                target: target.clone(),
                file_path,
                line,
                extra: component_extra(
                    "calls_native_function",
                    format!("{interface}::{name}"),
                    "rust",
                ),
            });
        }
    }
    Ok(())
}
