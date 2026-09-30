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
//! * C / C++ files defining a Python extension module (pybind11,
//!   nanobind, the CPython C-API), with the functions and classes they
//!   register (`abi: "python"`);
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
use crate::*;

mod c_imports;
mod cgo;
mod components;
mod cxx;
mod javascript;
mod jni;
mod python;
mod shared_libraries;
#[cfg(test)]
mod tests;
mod uniffi;
mod wasm;

use c_imports::*;
use cgo::*;
use components::*;
use cxx::*;
use javascript::*;
use jni::*;
use python::*;
use shared_libraries::*;
use uniffi::*;
use wasm::*;

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
    /// napi-rs / neon crates: how JavaScript reaches the Node.js addon.
    node_addon: Option<NodeAddon>,
    /// C / C++ compiled to WebAssembly by Emscripten.
    emscripten: Option<Emscripten>,
    /// Rust exposed to Kotlin / Swift / Python by UniFFI.
    uniffi: Option<Uniffi>,
}

/// Where UniFFI's generated bindings live in each foreign language.
#[derive(Clone)]
struct Uniffi {
    /// Kotlin package (`uniffi.<namespace>` by default).
    kotlin_package: String,
    /// Swift module (the namespace by default).
    swift_module: String,
}

/// An Emscripten build: its C functions are JavaScript exports `_name`.
struct Emscripten {
    /// `-sEXPORTED_FUNCTIONS` without the `_`; `None` when not listed, in
    /// which case every function with external C linkage counts.
    exported: Option<HashSet<String>>,
}

impl NativeCrate {
    /// `bridge_kind` of the bridges from JavaScript into this crate.
    fn js_bridge_kind(&self) -> &'static str {
        if self.node_addon.is_some() {
            "node_addon"
        } else {
            "wasm"
        }
    }
}

/// A napi-rs / neon Node.js addon, besides its package names.
struct NodeAddon {
    /// Generated glue (`index.js`, `index.d.ts`), repo-relative.
    entry_files: Vec<String>,
    /// Built `.node` files at known paths (neon's `index.node`).
    outputs: Vec<String>,
    /// napi-rs `binaryName`: `<name>.node` / `<name>.<platform>.node`.
    binary_names: Vec<String>,
}

/// `(namespace, bindings)` for a UniFFI crate: `setup_scaffolding!("ns")` in
/// the library root overrides the manifest's namespace.
fn uniffi_names(tx: &Transaction<'_>, root: &str, facts: &Value) -> Result<(String, Uniffi)> {
    let declared: Option<String> = tx
        .query_row(
            "SELECT json_extract(extra, '$.uniffi_namespace') FROM nodes \
             WHERE kind = 'File' AND file_path = ?",
            [root],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let namespace = declared
        .or_else(|| {
            facts
                .get("namespace")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let field = |key: &str| facts.get(key).and_then(Value::as_str).map(str::to_string);
    let bindings = Uniffi {
        kotlin_package: field("kotlin_package").unwrap_or_else(|| format!("uniffi.{namespace}")),
        swift_module: field("swift_module").unwrap_or_else(|| namespace.clone()),
    };
    Ok((namespace, bindings))
}

impl NodeAddon {
    /// From a `builds_from_source` edge marked `node_addon`.
    fn from_extra(extra: &Value) -> Option<Self> {
        extra.get("node_addon").and_then(Value::as_str)?;
        Some(Self {
            entry_files: string_list(extra, "js_entry_files"),
            outputs: string_list(extra, "node_outputs"),
            binary_names: string_list(extra, "node_binary_names"),
        })
    }

    /// True when the repo-relative module *path* is this addon's glue or
    /// binary (`native/index` for `native/index.js`, `native` for its
    /// `index`, or a `.node` file).
    fn matches(&self, path: &str) -> bool {
        let stem = module_stem(path);
        let glue = self.entry_files.iter().any(|entry| {
            let entry_stem = module_stem(entry);
            entry_stem == stem
                || entry_stem
                    .strip_suffix("/index")
                    .or_else(|| (entry_stem == "index").then_some(""))
                    .is_some_and(|dir| dir == stem)
        });
        let binary = path.ends_with(".node")
            && (self.outputs.iter().any(|output| output == path)
                || self.binary_names.iter().any(|name| {
                    let file = path.rsplit('/').next().unwrap_or(path);
                    file.strip_prefix(name.as_str())
                        .is_some_and(|rest| rest.starts_with('.'))
                }));
        glue || binary
    }
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
    /// Go `//go:wasmexport` / TinyGo `//export`, AssemblyScript `export`,
    /// and Node.js addon exports (`#[napi]`, neon).
    js: HashMap<String, Vec<String>>,
    /// Globals a Go `js.Global().Set("name", js.FuncOf(f))` defines.
    js_global: HashMap<String, Vec<String>>,
    /// UniFFI functions (by Rust name) and types, for Kotlin and Swift.
    uniffi_functions: HashMap<String, Vec<String>>,
    uniffi_types: HashMap<String, Vec<String>>,
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
    for ext in [".d.ts", ".wasm", ".node", ".js", ".mjs", ".cjs", ".ts"] {
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

/// The crate a JavaScript module specifier imports: a wasm-bindgen or
/// Node.js addon package by name, a relative path into a wasm-pack output
/// directory or to a WebAssembly glue / addon glue or `.node` file, or the
/// repo-relative path the extractor resolved such an import to.
fn js_crate_for(crates: &[NativeCrate], file_path: &str, spec: &str) -> Option<usize> {
    let resolved = if spec.starts_with("./") || spec.starts_with("../") {
        Some(join_relative(file_path, spec)?)
    } else {
        None
    };
    let matches_path = |krate: &NativeCrate, path: &str| {
        let stem = module_stem(path);
        krate.wasm_out_dirs.iter().any(|dir| {
            path == dir
                || path
                    .strip_prefix(dir.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        }) || krate
            .wasm_outputs
            .iter()
            .any(|output| module_stem(output) == stem)
            || krate
                .node_addon
                .as_ref()
                .is_some_and(|addon| addon.matches(path))
    };
    let matches: Vec<usize> = match &resolved {
        Some(path) => crates
            .iter()
            .enumerate()
            .filter(|(_, krate)| matches_path(krate, path))
            .map(|(index, _)| index)
            .collect(),
        None => {
            let package = package_of(spec);
            let by_name: Vec<usize> = crates
                .iter()
                .enumerate()
                .filter(|(_, krate)| krate.js_packages.iter().any(|name| name == package))
                .map(|(index, _)| index)
                .collect();
            if by_name.is_empty() {
                // An import the extractor resolved to a file in the
                // repository (committed napi-rs glue) arrives as its path.
                crates
                    .iter()
                    .enumerate()
                    .filter(|(_, krate)| krate.node_addon.is_some() && matches_path(krate, spec))
                    .map(|(index, _)| index)
                    .collect()
            } else {
                by_name
            }
        }
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
                Some("zig") => "zig",
                _ => "c",
            };
            let node_addon = NodeAddon::from_extra(&extra);
            let emscripten = (extra.get("build_system").and_then(Value::as_str)
                == Some("emscripten"))
            .then(|| Emscripten {
                exported: extra
                    .get("wasm_exports")
                    .is_some()
                    .then(|| string_list(&extra, "wasm_exports").into_iter().collect()),
            });
            crates.push(NativeCrate {
                root,
                lib_name: lib_name.to_string(),
                // node-gyp and Emscripten outputs are not libraries `ctypes`
                // loads, and `build.rs` / `build.zig` link their C sources
                // statically.
                cdylib: node_addon.is_none()
                    && emscripten.is_none()
                    && !matches!(
                        extra.get("build_system").and_then(Value::as_str),
                        Some("cc" | "cxx" | "zig-c")
                    ),
                python_module: None,
                js_packages: string_list(&extra, "js_packages"),
                wasm_out_dirs: Vec::new(),
                wasm_outputs: string_list(&extra, "wasm_outputs"),
                scope: ExportScope::Files(string_list(&extra, "source_files")),
                language,
                node_addon,
                emscripten,
                uniffi: None,
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
                node_addon: None,
                emscripten: None,
                uniffi: None,
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
        let uniffi = extra
            .get("uniffi")
            .map(|facts| uniffi_names(tx, &root, facts))
            .transpose()?;
        crates.push(NativeCrate {
            root,
            scope: ExportScope::Tree(crate_dir),
            language: "rust",
            build_system: "cargo".to_string(),
            node_addon: NodeAddon::from_extra(&extra),
            emscripten: None,
            uniffi: uniffi.as_ref().map(|(_, facts)| facts.clone()),
            // `cargo build --target wasm32-*` outputs of a guest crate.
            wasm_outputs: string_list(&extra, "wasm_outputs"),
            lib_name: lib_name.to_string(),
            cdylib,
            // UniFFI's Python bindings are a module named after the namespace.
            python_module: extra
                .get("python_module")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| uniffi.as_ref().map(|(namespace, _)| namespace.clone())),
            js_packages: string_list(&extra, "js_packages"),
            wasm_out_dirs: string_list(&extra, "wasm_out_dirs"),
        });
    }
    // C / C++ files that define a Python extension module themselves
    // (`PYBIND11_MODULE`, `NB_MODULE`, `PyInit_name`).
    let mut stmt = tx.prepare(
        "SELECT file_path, language, json_extract(extra, '$.python_module') FROM nodes \
         WHERE kind = 'File' AND json_extract(extra, '$.python_module') IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (file_path, language, module) = row?;
        crates.push(NativeCrate {
            root: file_path.clone(),
            lib_name: module.clone(),
            cdylib: false,
            python_module: Some(module),
            js_packages: Vec::new(),
            wasm_out_dirs: Vec::new(),
            wasm_outputs: Vec::new(),
            scope: ExportScope::Files(vec![file_path]),
            language: if language.as_deref() == Some("c") {
                "c"
            } else {
                "cpp"
            },
            build_system: "python-extension".to_string(),
            node_addon: None,
            emscripten: None,
            uniffi: None,
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
            if abi == "c"
                && let Some(emscripten) = &crates[index].emscripten
            {
                if emscripten
                    .exported
                    .as_ref()
                    .is_none_or(|exported| exported.contains(name))
                {
                    exports[index]
                        .js
                        .entry(format!("_{name}"))
                        .or_default()
                        .push(qualified.clone());
                }
                continue;
            }
            let table = match (abi, kind) {
                // Methods are reached through an instance, which a bare name
                // cannot tell apart; only module attributes are bound here.
                ("pyo3" | "python" | "wasm" | "napi" | "uniffi", "method") => continue,
                ("uniffi", kind) => {
                    // Python keeps the Rust name; Kotlin and Swift rename.
                    exports[index]
                        .python
                        .entry(name.to_string())
                        .or_default()
                        .push(qualified.clone());
                    if kind == "class" {
                        &mut exports[index].uniffi_types
                    } else {
                        &mut exports[index].uniffi_functions
                    }
                }
                ("pyo3" | "python", _) => &mut exports[index].python,
                // TinyGo's `//export` is a WebAssembly export.
                ("c", _) if go_wasm => &mut exports[index].js,
                ("c", _) => &mut exports[index].c,
                ("wasm" | "napi", _) => &mut exports[index].js,
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
                exports[index].js.entry(name).or_default().push(qualified);
            }
        }
    }
    Ok(exports)
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

/// The deepest directory in *dirs* containing *file_path* (`""` is the root).
fn nearest_dir(file_path: &str, dirs: &HashSet<String>) -> Option<String> {
    let mut current = file_path;
    while let Some((parent, _)) = current.rsplit_once('/') {
        if dirs.contains(parent) {
            return Some(parent.to_string());
        }
        current = parent;
    }
    dirs.contains("").then(String::new)
}

/// A native library a build file compiles from repository sources.
struct NativeLibraryRow {
    build_system: String,
    lib_name: String,
    /// Directory of the build file (`""` at the root).
    dir: String,
    sources: HashSet<String>,
}

fn load_native_libraries(tx: &Transaction<'_>) -> Result<Vec<NativeLibraryRow>> {
    let mut stmt = tx.prepare(
        "SELECT source_qualified, extra FROM edges WHERE kind = 'CROSS_ARTIFACT' \
           AND json_extract(extra, '$.manifest_kind') = 'native_library'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut libraries = Vec::new();
    for row in rows {
        let (config, extra) = row?;
        let extra = parse_json_column(extra)?;
        libraries.push(NativeLibraryRow {
            build_system: extra
                .get("build_system")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            lib_name: extra
                .get("lib_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .replace('-', "_"),
            dir: config
                .rsplit_once('/')
                .map_or("", |(dir, _)| dir)
                .to_string(),
            sources: string_list(&extra, "source_files").into_iter().collect(),
        });
    }
    Ok(libraries)
}

fn path_under(dir: &str, file: &str) -> bool {
    dir.is_empty()
        || file
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Sources a `build.rs` of one of *systems* links into the crate holding
/// *file_path*: the deepest such crate.
fn crate_linked_sources<'a>(
    libraries: &'a [NativeLibraryRow],
    file_path: &str,
    systems: &[&str],
) -> HashSet<&'a String> {
    let crate_dir = libraries
        .iter()
        .filter(|lib| {
            systems.contains(&lib.build_system.as_str()) && path_under(&lib.dir, file_path)
        })
        .map(|lib| lib.dir.as_str())
        .max_by_key(|dir| dir.len());
    libraries
        .iter()
        .filter(|lib| {
            systems.contains(&lib.build_system.as_str()) && Some(lib.dir.as_str()) == crate_dir
        })
        .flat_map(|lib| &lib.sources)
        .collect()
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
        bind_wasm_imports(&tx, &mut bridges)?;
        bind_c_imports(&tx, &mut bridges)?;
        bind_cxx_bridges(&tx, &mut bridges)?;
        bind_cgo(&tx, &mut bridges)?;
        bind_components(&tx, &mut bridges)?;
        let crates = load_crates(&tx)?;
        if !crates.is_empty() {
            let exports = load_exports(&tx, &crates)?;
            let bindings = bind_extension_imports(&tx, &crates, &mut bridges)?;
            bind_extension_calls(&tx, &crates, &exports, &bindings, &mut bridges)?;
            bind_shared_libraries(&tx, &crates, &exports, &mut bridges)?;
            let mut module_files = bind_js_modules(&tx, &crates, &exports, &mut bridges)?;
            bind_node_addon_loaders(&tx, &crates, &mut module_files, &mut bridges)?;
            bind_namespace_calls(&tx, &crates, &exports, &module_files, &mut bridges)?;
            let wasm_files = bind_wasm_loaders(&tx, &crates, &exports, &mut bridges)?;
            bind_wasm_export_calls(
                &tx,
                &crates,
                &exports,
                [&module_files, &wasm_files],
                &mut bridges,
            )?;
            bind_js_globals(&tx, &crates, &exports, &mut bridges)?;
            bind_uniffi(&tx, &crates, &exports, &mut bridges)?;
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
