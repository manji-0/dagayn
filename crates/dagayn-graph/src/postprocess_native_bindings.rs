//! Python -> native code bridges through PyO3 extension modules and `ctypes`.
//!
//! Parsing each language on its own leaves both sides of an FFI boundary
//! disconnected: a Python call `fast_sum(xs)` names a symbol that only exists
//! once the extension is built. This pass joins the evidence the extractors
//! record separately:
//!
//! * the manifest bridge `Cargo.toml -> src/lib.rs` (`builds_from_source`),
//!   carrying the library name, crate directory, and -- for maturin crates --
//!   the Python module name;
//! * Rust nodes with `extra.ffi_export` (`#[pyfunction]`, `#[pyclass]`,
//!   `#[no_mangle]`), naming what the crate exports;
//! * Python `IMPORTS_FROM` edges with the raw module and bound names, and
//!   `CALLS` edges with the import alias used as receiver;
//! * `loads_shared_library` bridges naming `libNAME.so` / `.dylib` / `.dll`.
//!
//! Every edge it writes is a `CROSS_ARTIFACT` tagged
//! `extractor = "native_bindings"`, and each run replaces the previous set.

use crate::helpers::*;
use crate::postprocess_bridges::extra_json;
use crate::*;

pub(crate) const NATIVE_BINDINGS_EXTRACTOR: &str = "native_bindings";
/// Exact name matches inside a crate the file demonstrably loads.
const NATIVE_BINDING_CONFIDENCE: f64 = 0.8;

struct NativeCrate {
    root: String,
    crate_dir: String,
    lib_name: String,
    cdylib: bool,
    python_module: Option<String>,
}

#[derive(Default)]
struct CrateExports {
    /// Python-visible module attributes: `#[pyfunction]` and `#[pyclass]`.
    python: HashMap<String, Vec<String>>,
    /// C symbols: `#[no_mangle]` / `#[export_name]`.
    c: HashMap<String, Vec<String>>,
}

/// What a local name in a Python file refers to.
#[derive(Clone)]
enum Binding {
    /// `from pkg import _core` / `import pkg._core as core`.
    Module(usize),
    /// `from pkg._core import fast_sum as fs`: crate and exported name.
    Symbol(usize, String),
}

struct NewBridge {
    source: String,
    target: String,
    file_path: String,
    line: i64,
    extra: Value,
}

fn owning_crate(crates: &[NativeCrate], file_path: &str) -> Option<usize> {
    crates
        .iter()
        .enumerate()
        .filter(|(_, krate)| {
            krate.crate_dir.is_empty()
                || file_path
                    .strip_prefix(krate.crate_dir.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|(_, krate)| krate.crate_dir.len())
        .map(|(index, _)| index)
}

/// Dotted package of a Python file: `pkg/sub/mod.py` -> `pkg.sub`.
fn python_package(file_path: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = file_path.split('/').collect();
    parts.pop();
    parts
}

/// Absolute form of `module` as written in *file_path* (`.` / `.._core`).
fn absolute_module(module: &str, file_path: &str) -> String {
    let dots = module.bytes().take_while(|byte| *byte == b'.').count();
    if dots == 0 {
        return module.to_string();
    }
    let mut package = python_package(file_path);
    for _ in 1..dots {
        package.pop();
    }
    let rest = &module[dots..];
    if !rest.is_empty() {
        package.push(rest);
    }
    package.join(".")
}

/// True when `module` names `python_module`. A relative import resolves to
/// a repo-relative path, which carries the source directory maturin's
/// `python-source` puts in front (`python/pkg._core`), so a dotted suffix
/// match counts too.
fn module_matches(module: &str, python_module: &str) -> bool {
    module == python_module
        || module
            .strip_suffix(python_module)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// `NAME` from `./target/release/libNAME.so.1`, `NAME.dll`, `libNAME.dylib`.
fn shared_library_stem(target: &str) -> Option<String> {
    if target.starts_with('<') {
        return None;
    }
    let base = target.rsplit(['/', '\\']).next()?;
    let mut stem = base;
    for ext in [".so", ".dylib", ".dll", ".pyd", ".bundle"] {
        if let Some(index) = stem.find(ext) {
            let tail = &stem[index + ext.len()..];
            // `.so` may carry a version suffix: `.so.1.2`.
            if tail.is_empty() || tail.starts_with('.') {
                stem = &stem[..index];
                break;
            }
        }
    }
    let stem = stem.strip_prefix("lib").unwrap_or(stem);
    (!stem.is_empty()).then(|| stem.replace('-', "_"))
}

fn call_name(target_name: &str) -> &str {
    let tail = target_name.rsplit("::").next().unwrap_or(target_name);
    tail.rsplit('.').next().unwrap_or(tail)
}

fn unique(candidates: Option<&Vec<String>>) -> Option<&String> {
    match candidates.map(Vec::as_slice) {
        Some([only]) => Some(only),
        _ => None,
    }
}

fn bridge_extra(
    role: &str,
    bridge_kind: &str,
    evidence_kind: &str,
    evidence_source: String,
    source_language: &str,
) -> Value {
    serde_json::json!({
        "relationship_role": role,
        "bridge_kind": bridge_kind,
        "evidence_kind": evidence_kind,
        "evidence_source": evidence_source,
        "source_language": source_language,
        "target_language": "rust",
        "confidence": NATIVE_BINDING_CONFIDENCE,
        "confidence_tier": "HIGH",
        "extractor": NATIVE_BINDINGS_EXTRACTOR,
    })
}

fn load_crates(tx: &Transaction<'_>) -> Result<Vec<NativeCrate>> {
    let mut stmt = tx.prepare(
        "SELECT target_qualified, extra FROM edges WHERE kind = 'CROSS_ARTIFACT' \
         AND json_extract(extra, '$.relationship_role') = 'builds_from_source'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut crates = Vec::new();
    for row in rows {
        let (root, extra) = row?;
        let extra = parse_json_column(extra)?;
        let Some(lib_name) = extra.get("lib_name").and_then(Value::as_str) else {
            continue;
        };
        let cdylib = extra
            .get("crate_types")
            .and_then(Value::as_array)
            .is_some_and(|types| types.iter().any(|t| t.as_str() == Some("cdylib")));
        crates.push(NativeCrate {
            root,
            crate_dir: extra
                .get("crate_dir")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            lib_name: lib_name.to_string(),
            cdylib,
            python_module: extra
                .get("python_module")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    Ok(crates)
}

fn load_exports(tx: &Transaction<'_>, crates: &[NativeCrate]) -> Result<Vec<CrateExports>> {
    let mut exports: Vec<CrateExports> = crates.iter().map(|_| CrateExports::default()).collect();
    let mut stmt = tx.prepare(
        "SELECT qualified_name, file_path, extra FROM nodes \
         WHERE language = 'rust' AND extra LIKE '%ffi_export%'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    for row in rows {
        let (qualified, file_path, extra) = row?;
        let Some(index) = owning_crate(crates, &file_path) else {
            continue;
        };
        let extra = parse_json_column(extra)?;
        let Some(export) = extra.get("ffi_export") else {
            continue;
        };
        let (Some(abi), Some(name)) = (
            export.get("abi").and_then(Value::as_str),
            export.get("name").and_then(Value::as_str),
        ) else {
            continue;
        };
        let kind = export
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("function");
        let table = match (abi, kind) {
            // Methods are reached through an instance, which a bare name
            // cannot tell apart; only module attributes are bound here.
            ("pyo3", "method") => continue,
            ("pyo3", _) => &mut exports[index].python,
            ("c", _) => &mut exports[index].c,
            _ => continue,
        };
        table.entry(name.to_string()).or_default().push(qualified);
    }
    Ok(exports)
}

/// Python import bindings to extension modules, per file, plus one
/// file-level `loads_native_module` bridge per imported crate.
fn bind_extension_imports(
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
                    ),
                });
            }
        }
    }
    bindings.retain(|_, file_bindings| !file_bindings.is_empty());
    Ok(bindings)
}

/// Caller, target name, line, and import receiver of one `CALLS` row.
type CallRow = (String, String, i64, Option<String>);

/// `CALLS` rows from *file_path*.
fn calls_in_file(tx: &Transaction<'_>, file_path: &str) -> Result<Vec<CallRow>> {
    let mut stmt = tx.prepare_cached(
        "SELECT source_qualified, COALESCE(target_name, target_qualified), line, extra \
         FROM edges WHERE kind = 'CALLS' AND file_path = ?",
    )?;
    let rows = stmt.query_map([file_path], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut calls = Vec::new();
    for row in rows {
        let (source, target, line, extra) = row?;
        let receiver = parse_json_column(extra)?
            .get("receiver")
            .and_then(Value::as_str)
            .map(str::to_string);
        calls.push((source, target, line, receiver));
    }
    Ok(calls)
}

fn bind_extension_calls(
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
                ),
            });
        }
    }
    Ok(())
}

/// `ctypes.CDLL("…/libNAME.so")` and friends -> the cdylib crate named NAME,
/// then calls in the same file to the crate's C symbols.
fn bind_shared_libraries(
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
    // File -> crates it loads.
    let mut loaded: HashMap<String, HashSet<usize>> = HashMap::new();
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
        let source_language = parse_json_column(extra)?
            .get("source_language")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let mut extra = bridge_extra(
            "loads_shared_library",
            "ffi",
            "manifest",
            format!("cargo lib name {}", crates[*index].lib_name),
            &source_language,
        );
        extra["library"] = Value::String(target);
        bridges.push(NewBridge {
            source,
            target: crates[*index].root.clone(),
            file_path: file_path.clone(),
            line,
            extra,
        });
        loaded.entry(file_path).or_default().insert(*index);
    }
    for (file_path, indexes) in loaded {
        for (caller, target, line, _) in calls_in_file(tx, &file_path)? {
            let name = call_name(&target);
            let hits: Vec<(&String, usize)> = indexes
                .iter()
                .filter_map(|index| unique(exports[*index].c.get(name)).map(|qn| (qn, *index)))
                .collect();
            let [(target, index)] = hits.as_slice() else {
                continue;
            };
            let mut extra = bridge_extra(
                "calls_native_function",
                "ffi",
                "syntax",
                format!("{}::{name}", crates[*index].lib_name),
                "unknown",
            );
            extra["source_language"] = Value::String(
                if file_path.ends_with(".py") || file_path.ends_with(".pyi") {
                    "python".to_string()
                } else {
                    "unknown".to_string()
                },
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

impl GraphStore {
    /// Replace the `native_bindings` bridges; returns how many were written.
    pub fn resolve_native_bindings(&mut self) -> Result<i64> {
        let now = now_seconds()?;
        let tx = write_tx(&mut self.conn)?;
        tx.execute(
            "DELETE FROM edges WHERE kind = 'CROSS_ARTIFACT' \
             AND json_extract(extra, '$.extractor') = ?",
            [NATIVE_BINDINGS_EXTRACTOR],
        )?;
        let crates = load_crates(&tx)?;
        if crates.is_empty() {
            tx.commit()?;
            return Ok(0);
        }
        let exports = load_exports(&tx, &crates)?;
        let mut bridges = Vec::new();
        let bindings = bind_extension_imports(&tx, &crates, &mut bridges)?;
        bind_extension_calls(&tx, &crates, &exports, &bindings, &mut bridges)?;
        bind_shared_libraries(&tx, &crates, &exports, &mut bridges)?;

        let mut seen: HashSet<(String, String, i64)> = HashSet::new();
        let mut written = 0_i64;
        for bridge in bridges {
            if !seen.insert((bridge.source.clone(), bridge.target.clone(), bridge.line)) {
                continue;
            }
            tx.execute(
                "INSERT INTO edges
                    (kind, source_qualified, target_qualified, target_name, file_path, line,
                     extra, confidence, confidence_tier, updated_at)
                 VALUES ('CROSS_ARTIFACT', ?, ?, ?, ?, ?, ?, ?, 'HIGH', ?)",
                params![
                    bridge.source,
                    bridge.target,
                    edge_target_name(&bridge.target),
                    bridge.file_path,
                    bridge.line,
                    extra_json(&bridge.extra)?,
                    NATIVE_BINDING_CONFIDENCE,
                    now
                ],
            )?;
            written += 1;
        }
        tx.commit()?;
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_library_stem_strips_prefix_and_extension() {
        assert_eq!(
            shared_library_stem("native/target/release/libfastsum.dylib").as_deref(),
            Some("fastsum")
        );
        assert_eq!(
            shared_library_stem("./libfoo-bar.so.1.2").as_deref(),
            Some("foo_bar")
        );
        assert_eq!(
            shared_library_stem("C:\\x\\foo.dll").as_deref(),
            Some("foo")
        );
        assert_eq!(shared_library_stem("<dynamic:ctypes.CDLL@a.py:1>"), None);
    }

    #[test]
    fn relative_modules_resolve_against_the_file_package() {
        assert_eq!(absolute_module(".", "pkg/__init__.py"), "pkg");
        assert_eq!(absolute_module("._core", "pkg/sub/a.py"), "pkg.sub._core");
        assert_eq!(absolute_module(".._core", "pkg/sub/a.py"), "pkg._core");
        assert!(module_matches("python.pkg._core", "pkg._core"));
        assert!(!module_matches("mypkg._core", "pkg._core"));
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;
    use serde_json::json;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dagayn-native-bindings-{name}-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn node(kind: &str, name: &str, file_path: &str, language: &str, extra: Value) -> NodeInput {
        NodeInput {
            kind: kind.to_string(),
            name: name.to_string(),
            file_path: file_path.to_string(),
            line_start: 1,
            line_end: 2,
            language: language.to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra,
        }
    }

    fn edge(kind: &str, source: &str, target: &str, file_path: &str, extra: Value) -> EdgeInput {
        EdgeInput {
            kind: kind.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            file_path: file_path.to_string(),
            line: 1,
            extra,
        }
    }

    fn store_rust_crate(store: &mut GraphStore, lib_name: &str, python_module: Option<&str>) {
        let file = "rust/src/lib.rs";
        store
            .store_file_nodes_edges(
                file,
                &[
                    node("File", file, file, "rust", json!({})),
                    node(
                        "Function",
                        "fast_sum",
                        file,
                        "rust",
                        json!({"ffi_export": {"abi": "pyo3", "kind": "function", "name": "fast_sum"}}),
                    ),
                    node(
                        "Function",
                        "c_sum",
                        file,
                        "rust",
                        json!({"ffi_export": {"abi": "c", "kind": "function", "name": "c_sum"}}),
                    ),
                    node(
                        "Function",
                        "commit",
                        file,
                        "rust",
                        json!({"ffi_export": {"abi": "pyo3", "kind": "method", "name": "commit"}}),
                    ),
                ],
                &[],
                "",
                0,
            )
            .expect("store crate");
        let mut extra = json!({
            "relationship_role": "builds_from_source",
            "extractor": "manifest_bridges",
            "confidence_tier": "HIGH",
            "confidence": 0.8,
            "lib_name": lib_name,
            "crate_types": ["cdylib"],
            "crate_dir": "rust",
        });
        if let Some(module) = python_module {
            extra["python_module"] = json!(module);
        }
        store
            .replace_manifest_bridges(
                "manifest_bridges",
                &[],
                &[edge(
                    "CROSS_ARTIFACT",
                    "rust/Cargo.toml",
                    file,
                    "rust/Cargo.toml",
                    extra,
                )],
            )
            .expect("manifest bridge");
    }

    fn bridges(store: &GraphStore) -> Vec<(String, String, String)> {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT source_qualified, target_qualified, \
                 json_extract(extra, '$.relationship_role') FROM edges \
                 WHERE json_extract(extra, '$.extractor') = 'native_bindings' \
                 ORDER BY 1, 2",
            )
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    }

    fn triple(a: &str, b: &str, c: &str) -> (String, String, String) {
        (a.to_string(), b.to_string(), c.to_string())
    }

    #[test]
    fn binds_extension_module_imports_and_calls() {
        let path = temp_db("pyo3");
        let mut store = GraphStore::open(&path).expect("open");
        store_rust_crate(&mut store, "_core", Some("pkg._core"));
        store
            .store_file_nodes_edges(
                "pkg/__init__.py",
                &[
                    node(
                        "File",
                        "pkg/__init__.py",
                        "pkg/__init__.py",
                        "python",
                        json!({}),
                    ),
                    node("Function", "total", "pkg/__init__.py", "python", json!({})),
                    node(
                        "Function",
                        "via_module",
                        "pkg/__init__.py",
                        "python",
                        json!({}),
                    ),
                ],
                &[
                    edge(
                        "IMPORTS_FROM",
                        "pkg/__init__.py",
                        "pkg._core",
                        "pkg/__init__.py",
                        json!({"module": "pkg._core", "names": [["fast_sum", "fs"]]}),
                    ),
                    edge(
                        "IMPORTS_FROM",
                        "pkg/__init__.py",
                        "pkg/__init__.py",
                        "pkg/__init__.py",
                        json!({"module": ".", "names": [["_core", "_core"]]}),
                    ),
                    edge(
                        "CALLS",
                        "pkg/__init__.py::total",
                        "fs",
                        "pkg/__init__.py",
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "pkg/__init__.py::via_module",
                        "fast_sum",
                        "pkg/__init__.py",
                        json!({"receiver": "_core"}),
                    ),
                    // A method name is not a module attribute.
                    edge(
                        "CALLS",
                        "pkg/__init__.py::via_module",
                        "commit",
                        "pkg/__init__.py",
                        json!({"receiver": "_core"}),
                    ),
                ],
                "",
                0,
            )
            .expect("store python");
        assert_eq!(store.resolve_native_bindings().unwrap(), 3);
        assert_eq!(
            bridges(&store),
            vec![
                triple("pkg/__init__.py", "rust/src/lib.rs", "loads_native_module"),
                triple(
                    "pkg/__init__.py::total",
                    "rust/src/lib.rs::fast_sum",
                    "calls_native_function"
                ),
                triple(
                    "pkg/__init__.py::via_module",
                    "rust/src/lib.rs::fast_sum",
                    "calls_native_function"
                ),
            ]
        );
        // Re-running replaces instead of duplicating.
        assert_eq!(store.resolve_native_bindings().unwrap(), 3);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn binds_ctypes_library_and_its_c_symbols() {
        let path = temp_db("ctypes");
        let mut store = GraphStore::open(&path).expect("open");
        store_rust_crate(&mut store, "fastsum", None);
        let loader_extra = json!({
            "relationship_role": "loads_shared_library",
            "bridge_kind": "ffi",
            "source_language": "python",
            "confidence_tier": "HIGH",
            "confidence": 0.8,
        });
        store
            .store_file_nodes_edges(
                "app/native.py",
                &[
                    node(
                        "File",
                        "app/native.py",
                        "app/native.py",
                        "python",
                        json!({}),
                    ),
                    node("Function", "load", "app/native.py", "python", json!({})),
                    node("Function", "total", "app/native.py", "python", json!({})),
                ],
                &[
                    edge(
                        "CROSS_ARTIFACT",
                        "app/native.py::load",
                        "rust/target/release/libfastsum.dylib",
                        "app/native.py",
                        loader_extra.clone(),
                    ),
                    edge(
                        "CROSS_ARTIFACT",
                        "app/native.py::load",
                        "libother.so",
                        "app/native.py",
                        loader_extra,
                    ),
                    edge(
                        "CALLS",
                        "app/native.py::total",
                        "c_sum",
                        "app/native.py",
                        json!({}),
                    ),
                    // pyo3 exports are not C symbols.
                    edge(
                        "CALLS",
                        "app/native.py::total",
                        "fast_sum",
                        "app/native.py",
                        json!({}),
                    ),
                ],
                "",
                0,
            )
            .expect("store python");
        assert_eq!(store.resolve_native_bindings().unwrap(), 2);
        assert_eq!(
            bridges(&store),
            vec![
                triple(
                    "app/native.py::load",
                    "rust/src/lib.rs",
                    "loads_shared_library"
                ),
                triple(
                    "app/native.py::total",
                    "rust/src/lib.rs::c_sum",
                    "calls_native_function"
                ),
            ]
        );
        let _ = std::fs::remove_file(path);
    }
}
