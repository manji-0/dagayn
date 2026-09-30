//! Bridges from Python and JavaScript / TypeScript into native code: PyO3
//! extension modules, shared libraries loaded with `ctypes`, and WebAssembly.
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
//! * C / C++ shared libraries a CMake, Meson, or Make build compiles from
//!   repository sources (`manifest_kind: native_library`), whose C symbols
//!   are the functions with `extra.ffi_export` (external C linkage);
//! * `loads_shared_library` bridges naming `libNAME.so` / `.dylib` / `.dll`;
//! * JavaScript / TypeScript imports of a wasm-bindgen package (by name, or a
//!   relative path into its wasm-pack output directory), and calls the
//!   extractor qualified as `specifier::name`;
//! * WebAssembly built from Go / TinyGo / AssemblyScript (`wasm_build`
//!   manifest bridges naming the `.wasm` outputs): JavaScript that imports
//!   the generated glue next to an output, or loads the output
//!   (`loads_wasm_module`, `fetch("app.wasm")`), and calls its exports; and
//!   calls to an ambient `declare function` that a Go `js.Global().Set`
//!   provides.
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
    lib_name: String,
    cdylib: bool,
    python_module: Option<String>,
    /// wasm-bindgen crates: package names JavaScript imports it by.
    js_packages: Vec<String>,
    /// wasm-bindgen crates: wasm-pack output directories (repo-relative).
    wasm_out_dirs: Vec<String>,
    /// `.wasm` files a Go / TinyGo / AssemblyScript build writes.
    wasm_outputs: Vec<String>,
    /// Where the exported items are declared.
    scope: ExportScope,
    /// Language of the exported items (`target_language` of the bridges).
    language: &'static str,
    /// What builds the library (`cargo`, `cmake`, `meson`, `make`), named
    /// in the evidence of the bridges that match its library name.
    build_system: String,
}

enum ExportScope {
    /// A Cargo crate: every file under the directory.
    Tree(String),
    /// A Go package: the files directly in the directory.
    Dir(String),
    /// AssemblyScript entry files, or the sources of a C / C++ library.
    Files(Vec<String>),
}

impl ExportScope {
    /// How specifically *file_path* belongs to the scope, if it does:
    /// explicit files and package directories beat a crate subtree.
    fn rank(&self, file_path: &str) -> Option<usize> {
        match self {
            Self::Tree(dir) => (dir.is_empty()
                || file_path
                    .strip_prefix(dir.as_str())
                    .is_some_and(|rest| rest.starts_with('/')))
            .then_some(dir.len()),
            Self::Dir(dir) => {
                let parent = file_path.rsplit_once('/').map_or("", |(parent, _)| parent);
                (parent == dir).then_some(usize::MAX - 1)
            }
            Self::Files(files) => files
                .iter()
                .any(|file| file == file_path)
                .then_some(usize::MAX),
        }
    }
}

#[derive(Default)]
struct CrateExports {
    /// Python-visible module attributes: `#[pyfunction]` and `#[pyclass]`.
    python: HashMap<String, Vec<String>>,
    /// C symbols: `#[no_mangle]` / `#[export_name]`.
    c: HashMap<String, Vec<String>>,
    /// JavaScript-visible exports: `#[wasm_bindgen]` functions and classes,
    /// Go `//go:wasmexport` / TinyGo `//export`, AssemblyScript `export`.
    wasm: HashMap<String, Vec<String>>,
    /// Globals a Go `js.Global().Set("name", js.FuncOf(f))` defines.
    js_global: HashMap<String, Vec<String>>,
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

/// The crates a file's exports belong to: every one at the most specific
/// rank, since a C source can be compiled into several shared libraries.
fn owning_crates(crates: &[NativeCrate], file_path: &str) -> Vec<usize> {
    let ranked: Vec<(usize, usize)> = crates
        .iter()
        .enumerate()
        .filter_map(|(index, krate)| krate.scope.rank(file_path).map(|rank| (index, rank)))
        .collect();
    let Some(best) = ranked.iter().map(|(_, rank)| *rank).max() else {
        return Vec::new();
    };
    ranked
        .into_iter()
        .filter(|(_, rank)| *rank == best)
        .map(|(index, _)| index)
        .collect()
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

fn string_list(extra: &Value, key: &str) -> Vec<String> {
    extra
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

const JAVASCRIPT_EXTENSIONS: &[&str] = &[
    ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".vue", ".svelte",
];

fn javascript_language(file_path: &str) -> Option<&'static str> {
    let ext = JAVASCRIPT_EXTENSIONS
        .iter()
        .find(|ext| file_path.ends_with(**ext))?;
    Some(if matches!(*ext, ".ts" | ".tsx" | ".mts" | ".cts") {
        "typescript"
    } else {
        "javascript"
    })
}

/// `dir/spec` with `.` / `..` folded; `None` when it climbs above the root.
fn join_relative(file_path: &str, spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = file_path.split('/').collect();
    parts.pop();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// A module path without its JavaScript / WebAssembly extension, so the
/// glue `build/release.js` meets the output `build/release.wasm`.
fn module_stem(path: &str) -> &str {
    for ext in [".d.ts", ".wasm", ".js", ".mjs", ".cjs", ".ts"] {
        if let Some(stem) = path.strip_suffix(ext) {
            return stem;
        }
    }
    path
}

/// A `.wasm` path as written (`./app.wasm?v=1`, `/static/app.wasm`) against
/// the crates' outputs: resolved from the importing file, or as a suffix of
/// an output (a URL is relative to the served page, not the file).
fn wasm_output_crate(crates: &[NativeCrate], file_path: &str, literal: &str) -> Option<usize> {
    let literal = literal.split(['?', '#']).next().unwrap_or(literal);
    let relative = (literal.starts_with("./") || literal.starts_with("../"))
        .then(|| join_relative(file_path, literal))
        .flatten();
    // A parent-relative path only matches when resolved from the file.
    let tail = Some(literal.trim_start_matches("./").trim_start_matches('/'))
        .filter(|tail| !tail.is_empty() && !tail.starts_with("../"));
    if relative.is_none() && tail.is_none() {
        return None;
    }
    let matches: Vec<usize> = crates
        .iter()
        .enumerate()
        .filter(|(_, krate)| {
            krate.wasm_outputs.iter().any(|output| {
                relative.as_deref() == Some(output.as_str())
                    || tail.is_some_and(|tail| {
                        output == tail
                            || output
                                .strip_suffix(tail)
                                .is_some_and(|prefix| prefix.ends_with('/'))
                    })
            })
        })
        .map(|(index, _)| index)
        .collect();
    match matches.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// The npm package a bare specifier names (`@scope/name/sub` -> `@scope/name`).
fn package_of(spec: &str) -> &str {
    let mut segments = spec.splitn(3, '/');
    let first = segments.next().unwrap_or_default();
    if first.starts_with('@') {
        let second = segments.next().unwrap_or_default();
        &spec[..(first.len() + 1 + second.len()).min(spec.len())]
    } else {
        first
    }
}

/// The wasm-bindgen crate a JavaScript module specifier imports: its package
/// name, or a relative path into its wasm-pack output directory.
fn wasm_crate_for(crates: &[NativeCrate], file_path: &str, spec: &str) -> Option<usize> {
    let matches: Vec<usize> = if spec.starts_with("./") || spec.starts_with("../") {
        let resolved = join_relative(file_path, spec)?;
        let stem = module_stem(&resolved);
        crates
            .iter()
            .enumerate()
            .filter(|(_, krate)| {
                krate.wasm_out_dirs.iter().any(|dir| {
                    resolved == *dir
                        || resolved
                            .strip_prefix(dir.as_str())
                            .is_some_and(|rest| rest.starts_with('/'))
                }) || krate
                    .wasm_outputs
                    .iter()
                    .any(|output| module_stem(output) == stem)
            })
            .map(|(index, _)| index)
            .collect()
    } else {
        let package = package_of(spec);
        crates
            .iter()
            .enumerate()
            .filter(|(_, krate)| krate.js_packages.iter().any(|name| name == package))
            .map(|(index, _)| index)
            .collect()
    };
    match matches.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

fn bridge_extra(
    role: &str,
    bridge_kind: &str,
    evidence_kind: &str,
    evidence_source: String,
    source_language: &str,
    target_language: &str,
) -> Value {
    serde_json::json!({
        "relationship_role": role,
        "bridge_kind": bridge_kind,
        "evidence_kind": evidence_kind,
        "evidence_source": evidence_source,
        "source_language": source_language,
        "target_language": target_language,
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
        if extra.get("manifest_kind").and_then(Value::as_str) == Some("native_library") {
            let Some(lib_name) = extra.get("lib_name").and_then(Value::as_str) else {
                continue;
            };
            let language = match extra.get("target_language").and_then(Value::as_str) {
                Some("cpp") => "cpp",
                Some("objc") => "objc",
                _ => "c",
            };
            crates.push(NativeCrate {
                root,
                lib_name: lib_name.to_string(),
                cdylib: true,
                python_module: None,
                js_packages: Vec::new(),
                wasm_out_dirs: Vec::new(),
                wasm_outputs: Vec::new(),
                scope: ExportScope::Files(string_list(&extra, "source_files")),
                language,
                build_system: extra
                    .get("build_system")
                    .and_then(Value::as_str)
                    .unwrap_or("make")
                    .to_string(),
            });
            continue;
        }
        if let Some(producer) = extra.get("wasm_producer").and_then(Value::as_str) {
            let (scope, language) = match producer {
                "go" | "tinygo" => (
                    ExportScope::Dir(
                        extra
                            .get("export_dir")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    "go",
                ),
                "assemblyscript" => (
                    ExportScope::Files(string_list(&extra, "entry_files")),
                    "typescript",
                ),
                _ => continue,
            };
            crates.push(NativeCrate {
                root,
                lib_name: String::new(),
                cdylib: false,
                python_module: None,
                js_packages: Vec::new(),
                wasm_out_dirs: Vec::new(),
                wasm_outputs: string_list(&extra, "wasm_outputs"),
                scope,
                language,
                build_system: producer.to_string(),
            });
            continue;
        }
        let Some(lib_name) = extra.get("lib_name").and_then(Value::as_str) else {
            continue;
        };
        let cdylib = extra
            .get("crate_types")
            .and_then(Value::as_array)
            .is_some_and(|types| types.iter().any(|t| t.as_str() == Some("cdylib")));
        let crate_dir = extra
            .get("crate_dir")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        crates.push(NativeCrate {
            root,
            scope: ExportScope::Tree(crate_dir),
            language: "rust",
            build_system: "cargo".to_string(),
            wasm_outputs: Vec::new(),
            lib_name: lib_name.to_string(),
            cdylib,
            python_module: extra
                .get("python_module")
                .and_then(Value::as_str)
                .map(str::to_string),
            js_packages: string_list(&extra, "js_packages"),
            wasm_out_dirs: string_list(&extra, "wasm_out_dirs"),
        });
    }
    Ok(crates)
}

fn load_exports(tx: &Transaction<'_>, crates: &[NativeCrate]) -> Result<Vec<CrateExports>> {
    let mut exports: Vec<CrateExports> = crates.iter().map(|_| CrateExports::default()).collect();
    let mut stmt = tx.prepare(
        "SELECT qualified_name, file_path, extra FROM nodes WHERE extra LIKE '%ffi_export%'",
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
        let owners = owning_crates(crates, &file_path);
        if owners.is_empty() {
            continue;
        }
        let extra = parse_json_column(extra)?;
        let listed: Vec<&Value> = extra
            .get("ffi_export")
            .into_iter()
            .chain(
                extra
                    .get("ffi_exports")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            )
            .collect();
        for (index, export) in owners
            .iter()
            .flat_map(|index| listed.iter().map(move |export| (*index, *export)))
        {
            let go_wasm = crates[index].language == "go";
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
                ("pyo3" | "wasm", "method") => continue,
                ("pyo3", _) => &mut exports[index].python,
                // TinyGo's `//export` is a WebAssembly export.
                ("c", _) if go_wasm => &mut exports[index].wasm,
                ("c", _) => &mut exports[index].c,
                ("wasm", _) => &mut exports[index].wasm,
                ("js_global", _) => &mut exports[index].js_global,
                _ => continue,
            };
            table
                .entry(name.to_string())
                .or_default()
                .push(qualified.clone());
        }
    }
    // AssemblyScript: the entry files' exported functions and classes.
    let mut entry_stmt = tx.prepare(
        "SELECT qualified_name, name FROM nodes WHERE file_path = ? \
         AND kind IN ('Function', 'Class') AND extra LIKE '%\"exported\":true%'",
    )?;
    for (index, krate) in crates.iter().enumerate() {
        let ExportScope::Files(files) = &krate.scope else {
            continue;
        };
        if krate.language != "typescript" {
            continue;
        }
        for file in files {
            let rows = entry_stmt.query_map([file], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (qualified, name) = row?;
                exports[index].wasm.entry(name).or_default().push(qualified);
            }
        }
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
                        "rust",
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
                    "rust",
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
    // Python file -> crates it loads. Only Python looks symbols up by the
    // name it calls (`lib.fast_sum(...)` on a `ctypes.CDLL`); a Java file
    // that loads a library calls `native` methods whose C symbols are
    // `Java_<class>_<method>`, so a bare-name match there is a coincidence.
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
        if source_language == "python" {
            loaded.entry(file_path).or_default().insert(*index);
        }
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
            let extra = bridge_extra(
                "calls_native_function",
                "ffi",
                "syntax",
                format!("{}::{name}", crates[*index].lib_name),
                "python",
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

/// JavaScript / TypeScript imports of a wasm-bindgen crate, and the calls
/// the extractor qualified as `specifier::name` into it.
fn bind_wasm_modules(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    if crates
        .iter()
        .all(|krate| krate.js_packages.is_empty() && krate.wasm_outputs.is_empty())
    {
        return Ok(());
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
            let Some(index) = wasm_crate_for(crates, &file_path, &target) else {
                continue;
            };
            if linked.insert((file_path.clone(), index)) {
                bridges.push(NewBridge {
                    source,
                    target: crates[index].root.clone(),
                    file_path,
                    line,
                    extra: bridge_extra(
                        "loads_native_module",
                        "wasm",
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
        let Some(index) = wasm_crate_for(crates, &file_path, spec) else {
            continue;
        };
        let name = path.split('.').next().unwrap_or(path);
        let Some(export) = unique(exports[index].wasm.get(name)) else {
            continue;
        };
        bridges.push(NewBridge {
            source,
            target: export.clone(),
            file_path,
            line,
            extra: bridge_extra(
                "calls_native_function",
                "wasm",
                "syntax",
                format!("wasm export {name}"),
                language,
                crates[index].language,
            ),
        });
    }
    Ok(())
}

/// `fetch("app.wasm")` and friends (`loads_wasm_module`) -> the crate whose
/// build writes that file; then calls in the loading file to its exports
/// (`instance.exports.add(...)`) or to the globals a Go module defines.
fn bind_wasm_loaders(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    if crates.iter().all(|krate| krate.wasm_outputs.is_empty()) {
        return Ok(());
    }
    let loaders = {
        let mut stmt = tx.prepare(
            "SELECT source_qualified, target_qualified, file_path, line FROM edges \
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
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut loaded: HashMap<String, HashSet<usize>> = HashMap::new();
    for (source, target, file_path, line) in loaders {
        let Some(index) = wasm_output_crate(crates, &file_path, &target) else {
            continue;
        };
        let language = javascript_language(&file_path).unwrap_or("unknown");
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
    for (file_path, indexes) in loaded {
        let language = javascript_language(&file_path).unwrap_or("unknown");
        for (caller, target, line, _) in calls_in_file(tx, &file_path)? {
            let name = call_name(&target);
            let hits: Vec<(&String, usize)> = indexes
                .iter()
                .filter_map(|index| {
                    unique(exports[*index].wasm.get(name))
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
    Ok(())
}

/// A call to an ambient `declare function name` in JavaScript / TypeScript,
/// when exactly one Go module defines the global `name`.
fn bind_js_globals(
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

/// Java `native` / Kotlin `external` methods -> the C-ABI function their JNI
/// name spells (`Java_com_example_Sum_fastSum`, or an overload's
/// `Java_..._fastSum__<signature>`), in C, C++ `extern "C"`, or Rust
/// `#[no_mangle]`. The name encodes package, class, and method, so it is
/// matched across the whole repository rather than inside a library the
/// class is known to load.
fn bind_jni_methods(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    let mut exports: HashMap<String, Vec<(String, String)>> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, language, json_extract(extra, '$.ffi_export.name') \
             FROM nodes \
             WHERE json_extract(extra, '$.ffi_export.abi') = 'c' \
               AND json_extract(extra, '$.ffi_export.name') LIKE 'Java\\_%' ESCAPE '\\'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
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
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
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
        let mut bridges = Vec::new();
        bind_jni_methods(&tx, &mut bridges)?;
        let crates = load_crates(&tx)?;
        if !crates.is_empty() {
            let exports = load_exports(&tx, &crates)?;
            let bindings = bind_extension_imports(&tx, &crates, &mut bridges)?;
            bind_extension_calls(&tx, &crates, &exports, &bindings, &mut bridges)?;
            bind_shared_libraries(&tx, &crates, &exports, &mut bridges)?;
            bind_wasm_modules(&tx, &crates, &exports, &mut bridges)?;
            bind_wasm_loaders(&tx, &crates, &exports, &mut bridges)?;
            bind_js_globals(&tx, &crates, &exports, &mut bridges)?;
        }

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

    #[test]
    fn binds_javascript_imports_and_calls_to_a_wasm_bindgen_crate() {
        let path = temp_db("wasm");
        let mut store = GraphStore::open(&path).expect("open");
        let lib = "wasm/src/lib.rs";
        store
            .store_file_nodes_edges(
                lib,
                &[
                    node("File", lib, lib, "rust", json!({})),
                    node(
                        "Function",
                        "mean_of",
                        lib,
                        "rust",
                        json!({"ffi_export": {"abi": "wasm", "kind": "function", "name": "meanOf"}}),
                    ),
                    node(
                        "Class",
                        "Accumulator",
                        lib,
                        "rust",
                        json!({"ffi_export": {"abi": "wasm", "kind": "class", "name": "Accumulator"}}),
                    ),
                ],
                &[],
                "",
                0,
            )
            .expect("store crate");
        store
            .replace_manifest_bridges(
                "manifest_bridges",
                &[],
                &[edge(
                    "CROSS_ARTIFACT",
                    "wasm/Cargo.toml",
                    lib,
                    "wasm/Cargo.toml",
                    json!({
                        "relationship_role": "builds_from_source",
                        "extractor": "manifest_bridges",
                        "confidence_tier": "HIGH",
                        "confidence": 0.8,
                        "lib_name": "fast_sum",
                        "crate_types": ["cdylib"],
                        "crate_dir": "wasm",
                        "wasm_bindgen": true,
                        "js_packages": ["fast-sum"],
                        "wasm_out_dirs": ["wasm/pkg"],
                    }),
                )],
            )
            .expect("manifest bridge");
        store
            .store_file_nodes_edges(
                "web/src/stats.ts",
                &[
                    node(
                        "File",
                        "web/src/stats.ts",
                        "web/src/stats.ts",
                        "typescript",
                        json!({}),
                    ),
                    node(
                        "Function",
                        "mean",
                        "web/src/stats.ts",
                        "typescript",
                        json!({}),
                    ),
                    node(
                        "Function",
                        "run",
                        "web/src/stats.ts",
                        "typescript",
                        json!({}),
                    ),
                ],
                &[
                    edge(
                        "IMPORTS_FROM",
                        "web/src/stats.ts",
                        "fast-sum",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    edge(
                        "IMPORTS_FROM",
                        "web/src/stats.ts",
                        "../../wasm/pkg/fast_sum.js",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/stats.ts::mean",
                        "fast-sum::meanOf",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/stats.ts::run",
                        "../../wasm/pkg/fast_sum.js::Accumulator",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    // wasm-bindgen's `init` is JS glue, not a Rust export.
                    edge(
                        "CALLS",
                        "web/src/stats.ts::run",
                        "fast-sum::default",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                    // Another package with the same export name is not the crate.
                    edge(
                        "CALLS",
                        "web/src/stats.ts::run",
                        "other-pkg::meanOf",
                        "web/src/stats.ts",
                        json!({}),
                    ),
                ],
                "",
                0,
            )
            .expect("store typescript");
        assert_eq!(store.resolve_native_bindings().unwrap(), 3);
        assert_eq!(
            bridges(&store),
            vec![
                triple("web/src/stats.ts", lib, "loads_native_module"),
                triple(
                    "web/src/stats.ts::mean",
                    "wasm/src/lib.rs::mean_of",
                    "calls_native_function"
                ),
                triple(
                    "web/src/stats.ts::run",
                    "wasm/src/lib.rs::Accumulator",
                    "calls_native_function"
                ),
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn relative_specifiers_resolve_against_the_importing_file() {
        assert_eq!(
            join_relative("web/src/stats.ts", "../../wasm/pkg/fast_sum.js").as_deref(),
            Some("wasm/pkg/fast_sum.js")
        );
        assert_eq!(join_relative("a.ts", "../x"), None);
        assert_eq!(package_of("@scope/name/sub"), "@scope/name");
        assert_eq!(package_of("fast-sum/snippets"), "fast-sum");
    }

    fn wasm_build_edge(config: &str, root: &str, producer: &str, extra: Value) -> EdgeInput {
        let mut payload = json!({
            "relationship_role": "builds_from_source",
            "extractor": "manifest_bridges",
            "confidence_tier": "HIGH",
            "confidence": 0.8,
            "manifest_kind": "wasm_build",
            "wasm_producer": producer,
        });
        for (key, value) in extra.as_object().unwrap() {
            payload[key] = value.clone();
        }
        edge("CROSS_ARTIFACT", config, root, config, payload)
    }

    #[test]
    fn binds_javascript_to_go_and_assemblyscript_webassembly() {
        let path = temp_db("wasm-producers");
        let mut store = GraphStore::open(&path).expect("open");
        let go = "gowasm/main.go";
        let tiny = "tinygo/main.go";
        let asc = "as/assembly/index.ts";
        store
            .store_file_nodes_edges(
                go,
                &[
                    node("File", go, go, "go", json!({})),
                    node(
                        "Function",
                        "fastSum",
                        go,
                        "go",
                        json!({"ffi_exports": [{"abi": "js_global", "kind": "function", "name": "goFastSum"}]}),
                    ),
                ],
                &[],
                "",
                0,
            )
            .expect("store go");
        store
            .store_file_nodes_edges(
                tiny,
                &[
                    node("File", tiny, tiny, "go", json!({})),
                    node(
                        "Function",
                        "mul",
                        tiny,
                        "go",
                        json!({"ffi_export": {"abi": "c", "kind": "function", "name": "mul"}}),
                    ),
                ],
                &[],
                "",
                0,
            )
            .expect("store tinygo");
        store
            .store_file_nodes_edges(
                asc,
                &[
                    node("File", asc, asc, "typescript", json!({})),
                    node(
                        "Function",
                        "asSum",
                        asc,
                        "typescript",
                        json!({"exported": true}),
                    ),
                    node("Function", "helper", asc, "typescript", json!({})),
                ],
                &[],
                "",
                0,
            )
            .expect("store assemblyscript");
        store
            .replace_manifest_bridges(
                "manifest_bridges",
                &[],
                &[
                    wasm_build_edge(
                        "web/package.json",
                        go,
                        "go",
                        json!({"wasm_outputs": ["web/public/go.wasm"], "export_dir": "gowasm"}),
                    ),
                    wasm_build_edge(
                        "web/package.json",
                        tiny,
                        "tinygo",
                        json!({"wasm_outputs": ["web/public/tiny.wasm"], "export_dir": "tinygo"}),
                    ),
                    wasm_build_edge(
                        "as/asconfig.json",
                        asc,
                        "assemblyscript",
                        json!({"wasm_outputs": ["as/build/release.wasm"], "entry_files": [asc]}),
                    ),
                ],
            )
            .expect("manifest bridges");
        let web = "web/src/app.ts";
        let loader = json!({
            "relationship_role": "loads_wasm_module",
            "bridge_kind": "wasm",
            "confidence_tier": "HIGH",
            "confidence": 0.8,
        });
        store
            .store_file_nodes_edges(
                web,
                &[
                    node("File", web, web, "typescript", json!({})),
                    node("Function", "load", web, "typescript", json!({})),
                    node("Function", "useAll", web, "typescript", json!({})),
                    node(
                        "Function",
                        "goFastSum",
                        web,
                        "typescript",
                        json!({"ambient": true, "declaration_only": true}),
                    ),
                ],
                &[
                    edge(
                        "CROSS_ARTIFACT",
                        "web/src/app.ts::load",
                        "tiny.wasm?v=1",
                        web,
                        loader,
                    ),
                    edge(
                        "IMPORTS_FROM",
                        web,
                        "../../as/build/release.js",
                        web,
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/app.ts::useAll",
                        "mul",
                        web,
                        json!({"receiver_unknown": true}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/app.ts::useAll",
                        "../../as/build/release.js::asSum",
                        web,
                        json!({"unresolved_module": "../../as/build/release.js"}),
                    ),
                    // Not exported from the entry file.
                    edge(
                        "CALLS",
                        "web/src/app.ts::useAll",
                        "../../as/build/release.js::helper",
                        web,
                        json!({}),
                    ),
                    edge(
                        "CALLS",
                        "web/src/app.ts::useAll",
                        "web/src/app.ts::goFastSum",
                        web,
                        json!({}),
                    ),
                ],
                "",
                0,
            )
            .expect("store typescript");
        store.resolve_native_bindings().unwrap();
        assert_eq!(
            bridges(&store),
            vec![
                triple(web, asc, "loads_native_module"),
                triple("web/src/app.ts::load", tiny, "loads_native_module"),
                triple(
                    "web/src/app.ts::useAll",
                    "as/assembly/index.ts::asSum",
                    "calls_native_function"
                ),
                triple(
                    "web/src/app.ts::useAll",
                    "gowasm/main.go::fastSum",
                    "calls_native_function"
                ),
                triple(
                    "web/src/app.ts::useAll",
                    "tinygo/main.go::mul",
                    "calls_native_function"
                ),
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn wasm_output_paths_match_as_written_or_as_a_suffix() {
        let producer = |outputs: &[&str]| NativeCrate {
            root: String::new(),
            lib_name: String::new(),
            cdylib: false,
            python_module: None,
            js_packages: Vec::new(),
            wasm_out_dirs: Vec::new(),
            wasm_outputs: outputs.iter().map(|o| o.to_string()).collect(),
            scope: ExportScope::Files(Vec::new()),
            language: "go",
            build_system: "go".to_string(),
        };
        let crates = [
            producer(&["web/public/go.wasm"]),
            producer(&["as/build/release.wasm"]),
        ];
        assert_eq!(
            wasm_output_crate(&crates, "web/src/a.ts", "/go.wasm"),
            Some(0)
        );
        assert_eq!(
            wasm_output_crate(&crates, "web/src/a.ts", "go.wasm?v=3"),
            Some(0)
        );
        assert_eq!(
            wasm_output_crate(&crates, "web/src/a.ts", "../../as/build/release.wasm"),
            Some(1)
        );
        assert_eq!(
            wasm_output_crate(&crates, "web/src/a.ts", "other.wasm"),
            None
        );
        assert_eq!(
            wasm_crate_for(&crates, "web/src/a.ts", "../../as/build/release.js"),
            Some(1)
        );
    }
}
