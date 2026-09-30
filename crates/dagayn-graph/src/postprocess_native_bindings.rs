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
    /// napi-rs / neon crates: how JavaScript reaches the Node.js addon.
    node_addon: Option<NodeAddon>,
    /// C / C++ compiled to WebAssembly by Emscripten.
    emscripten: Option<Emscripten>,
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
            node_addon: NodeAddon::from_extra(&extra),
            emscripten: None,
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
                ("pyo3" | "python" | "wasm" | "napi", "method") => continue,
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
                        crates[index].language,
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
                    crates[index].language,
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

/// JavaScript / TypeScript imports of a wasm-bindgen or Node.js addon
/// crate, and the calls the extractor qualified as `specifier::name` into
/// it. Returns the files that import a Node.js addon or Emscripten glue,
/// whose other calls [`bind_namespace_calls`] matches by name.
fn bind_js_modules(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    bridges: &mut Vec<NewBridge>,
) -> Result<HashMap<String, HashSet<usize>>> {
    let mut module_files: HashMap<String, HashSet<usize>> = HashMap::new();
    if crates.iter().all(|krate| {
        krate.js_packages.is_empty() && krate.wasm_outputs.is_empty() && krate.node_addon.is_none()
    }) {
        return Ok(module_files);
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
            let Some(index) = js_crate_for(crates, &file_path, &target) else {
                continue;
            };
            if crates[index].node_addon.is_some() || crates[index].emscripten.is_some() {
                module_files
                    .entry(file_path.clone())
                    .or_default()
                    .insert(index);
            }
            if linked.insert((file_path.clone(), index)) {
                bridges.push(NewBridge {
                    source,
                    target: crates[index].root.clone(),
                    file_path,
                    line,
                    extra: bridge_extra(
                        "loads_native_module",
                        crates[index].js_bridge_kind(),
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
        let Some(index) = js_crate_for(crates, &file_path, spec) else {
            continue;
        };
        let name = path.split('.').next().unwrap_or(path);
        let Some(export) = unique(exports[index].js.get(name)) else {
            continue;
        };
        bridges.push(NewBridge {
            source,
            target: export.clone(),
            file_path,
            line,
            extra: bridge_extra(
                "calls_native_function",
                crates[index].js_bridge_kind(),
                "syntax",
                format!("export {name}"),
                language,
                crates[index].language,
            ),
        });
    }
    Ok(module_files)
}

/// `require("bindings")("addon")` (`loads_node_addon`) -> the node-gyp
/// target of that name.
fn bind_node_addon_loaders(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    module_files: &mut HashMap<String, HashSet<usize>>,
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    let mut stmt = tx.prepare(
        "SELECT source_qualified, target_qualified, file_path, line FROM edges \
         WHERE kind = 'CROSS_ARTIFACT' \
           AND json_extract(extra, '$.relationship_role') = 'loads_node_addon'",
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
        let (source, target, file_path, line) = row?;
        let name = target.strip_suffix(".node").unwrap_or(&target);
        let matches: Vec<usize> = crates
            .iter()
            .enumerate()
            .filter(|(_, krate)| krate.node_addon.is_some() && krate.lib_name == name)
            .map(|(index, _)| index)
            .collect();
        let [index] = matches.as_slice() else {
            continue;
        };
        module_files
            .entry(file_path.clone())
            .or_default()
            .insert(*index);
        bridges.push(NewBridge {
            source,
            target: crates[*index].root.clone(),
            file_path: file_path.clone(),
            line,
            extra: bridge_extra(
                "loads_native_module",
                "node_addon",
                "config",
                format!("{} target {name}", crates[*index].build_system),
                javascript_language(&file_path).unwrap_or("javascript"),
                crates[*index].language,
            ),
        });
    }
    Ok(())
}

/// In a file that loads a Node.js addon or Emscripten glue, a call the
/// extractor left as a bare name (`addon.hello()` on a CommonJS `require`,
/// `Module._add()`, whose receiver it cannot type) that exactly one of the
/// loaded modules exports.
fn bind_namespace_calls(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    module_files: &HashMap<String, HashSet<usize>>,
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    for (file_path, indexes) in module_files {
        let language = javascript_language(file_path).unwrap_or("javascript");
        for (caller, target, line, _) in calls_in_file(tx, file_path)? {
            if target.contains("::") {
                continue;
            }
            let hits: Vec<(&String, usize)> = indexes
                .iter()
                .filter_map(|index| unique(exports[*index].js.get(&target)).map(|qn| (qn, *index)))
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
                    crates[*index].js_bridge_kind(),
                    "syntax",
                    format!("export {target}"),
                    language,
                    crates[*index].language,
                ),
            });
        }
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
) -> Result<HashMap<String, HashSet<usize>>> {
    let mut loaded: HashMap<String, HashSet<usize>> = HashMap::new();
    if crates.iter().all(|krate| krate.wasm_outputs.is_empty()) {
        return Ok(loaded);
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
    for (file_path, indexes) in &loaded {
        let language = javascript_language(file_path).unwrap_or("unknown");
        for (caller, target, line, _) in calls_in_file(tx, file_path)? {
            let name = call_name(&target);
            let hits: Vec<(&String, usize)> = indexes
                .iter()
                .filter_map(|index| {
                    unique(exports[*index].js.get(name))
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
    Ok(loaded)
}

/// Emscripten's `ccall("add", ...)` / `cwrap("add", ...)`
/// (`calls_wasm_export`) in a file that loads the module -> the C function
/// behind the export `_add`.
fn bind_wasm_export_calls(
    tx: &Transaction<'_>,
    crates: &[NativeCrate],
    exports: &[CrateExports],
    loaded: [&HashMap<String, HashSet<usize>>; 2],
    bridges: &mut Vec<NewBridge>,
) -> Result<()> {
    let mut stmt = tx.prepare(
        "SELECT source_qualified, target_qualified, file_path, line FROM edges \
         WHERE kind = 'CROSS_ARTIFACT' \
           AND json_extract(extra, '$.relationship_role') = 'calls_wasm_export'",
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
        let (source, name, file_path, line) = row?;
        let indexes: HashSet<usize> = loaded
            .iter()
            .filter_map(|files| files.get(&file_path))
            .flatten()
            .copied()
            .filter(|index| crates[*index].emscripten.is_some())
            .collect();
        let key = format!("_{name}");
        let hits: Vec<(&String, usize)> = indexes
            .iter()
            .filter_map(|index| unique(exports[*index].js.get(&key)).map(|qn| (qn, *index)))
            .collect();
        let [(export, index)] = hits.as_slice() else {
            continue;
        };
        bridges.push(NewBridge {
            source,
            target: (*export).clone(),
            file_path: file_path.clone(),
            line,
            extra: bridge_extra(
                "calls_native_function",
                "wasm",
                "syntax",
                format!("export {key}"),
                javascript_language(&file_path).unwrap_or("javascript"),
                crates[*index].language,
            ),
        });
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

/// WebAssembly code calling into JavaScript: a foreign declaration -> the
/// JavaScript function that implements it (`wraps_foreign_api`).
///
/// * Rust `#[wasm_bindgen(module = "/js/util.js")] extern "C" { fn f(); }`:
///   the function `f` (or `js_name`) of that file, a path from the crate
///   root (the nearest directory with a `Cargo.toml`).
/// * Go `//go:wasmimport env f`: a JavaScript function `f` defined in an
///   object literal under the key `env` (the import object passed to
///   `WebAssembly.instantiate`), when exactly one exists.
fn bind_wasm_imports(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    let imports = {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, file_path, line_start, language, \
                    json_extract(extra, '$.ffi_import.abi'), \
                    json_extract(extra, '$.ffi_import.module'), \
                    json_extract(extra, '$.ffi_import.name') \
             FROM nodes WHERE json_extract(extra, '$.ffi_import.abi') IN ('wasm', 'wasmimport')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    if imports.is_empty() {
        return Ok(());
    }
    let cargo_dirs: HashSet<String> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT file_path FROM nodes \
             WHERE file_path = 'Cargo.toml' OR file_path LIKE '%/Cargo.toml'",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            row.map(|path| {
                path.strip_suffix("Cargo.toml")
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string()
            })
        })
        .collect::<std::result::Result<_, _>>()?
    };
    let mut in_file = tx.prepare_cached(
        "SELECT qualified_name FROM nodes \
         WHERE file_path = ? AND name = ? AND kind = 'Function' AND parent_name IS NULL",
    )?;
    let mut in_object = tx.prepare_cached(
        "SELECT qualified_name, file_path FROM nodes \
         WHERE name = ? AND kind = 'Function' \
           AND (parent_name = ? OR parent_name LIKE '%.' || ?)",
    )?;
    for (source, file_path, line, language, abi, module, name) in imports {
        let (Some(module), Some(name)) = (module, name) else {
            continue;
        };
        let targets: Vec<(String, String)> = if abi == "wasm" {
            let Some(rel) = module.strip_prefix('/') else {
                continue;
            };
            let Some(crate_dir) = nearest_dir(&file_path, &cargo_dirs) else {
                continue;
            };
            let js_file = if crate_dir.is_empty() {
                rel.to_string()
            } else {
                format!("{crate_dir}/{rel}")
            };
            let rows = in_file.query_map(params![js_file, name], |row| row.get::<_, String>(0))?;
            rows.map(|row| row.map(|qualified| (qualified, js_file.clone())))
                .collect::<std::result::Result<_, _>>()?
        } else {
            let rows = in_object.query_map(params![name, module, module], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|(_, path)| javascript_language(path).is_some())
                .collect()
        };
        let [(target, target_file)] = targets.as_slice() else {
            continue;
        };
        bridges.push(NewBridge {
            source,
            target: target.clone(),
            file_path,
            line,
            extra: bridge_extra(
                "wraps_foreign_api",
                "wasm",
                "syntax",
                format!("{module}::{name}"),
                language.as_deref().unwrap_or("unknown"),
                javascript_language(target_file).unwrap_or("javascript"),
            ),
        });
    }
    Ok(())
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

/// A declaration bound to a C symbol (`ffi_import.abi = "c"`: Rust
/// `extern "C" { fn f(); }`, Zig `extern fn f`, Dart
/// `@Native(symbol: "f")`), or a Zig call through `@cImport` -> the C-ABI
/// function `f` (`calls_native_function`). The symbol is looked for, in
/// order, among the C sources linked into the same crate or Zig build
/// (`build.rs` `cc` / `cxx_build`, `build.zig` `addCSourceFile(s)`), the
/// sources of the library `#[link(name = "...")]` / `extern "lib"` names,
/// and the whole repository; the first scope with a match must hold
/// exactly one.
fn bind_c_imports(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    let imports = {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, file_path, line_start, \
                    json_extract(extra, '$.ffi_import.name'), \
                    json_extract(extra, '$.ffi_import.library'), language \
             FROM nodes WHERE json_extract(extra, '$.ffi_import.abi') = 'c'",
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
        let mut imports = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        // Zig calls through `const c = @cImport(...)`: `c.fast_sum(...)`.
        let mut calls = tx.prepare(
            "SELECT source_qualified, file_path, line, COALESCE(target_name, target_qualified) \
             FROM edges WHERE kind = 'CALLS' AND json_extract(extra, '$.c_import') = 1",
        )?;
        let rows = calls.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
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
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
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
                let library = library.replace('-', "_");
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

/// cxx bridges, within the sources the crate's `build.rs` compiles with
/// `cxx_build` (C++ names are mangled, so nothing outside counts):
///
/// * `unsafe extern "C++" { fn f(); }` -> the free C++ function `f`, and
///   `fn m(self: Pin<&mut T>)` -> the method `T::m`;
/// * `extern "Rust" { fn g(); }` -> C++ calls to `g` reach the Rust
///   function `g` of the crate.
fn bind_cxx_bridges(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
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

/// cgo, which compiles the C files of a Go package directory with it:
///
/// * `C.f(...)` in a Go file -> the C-ABI function `f`, looked for among
///   the C files of the package directory, then the sources of the
///   libraries `#cgo LDFLAGS: -lNAME` links, then the whole repository;
///   the first scope with a match must hold exactly one;
/// * a C call in the package directory to a function a Go `//export`
///   declares -> that Go function.
fn bind_cgo(tx: &Transaction<'_>, bridges: &mut Vec<NewBridge>) -> Result<()> {
    let go_files: Vec<(String, Option<String>)> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT e.file_path, n.extra FROM edges e \
             LEFT JOIN nodes n ON n.kind = 'File' AND n.file_path = e.file_path \
             WHERE e.kind = 'IMPORTS_FROM' AND e.target_qualified = 'C' \
               AND e.file_path LIKE '%.go'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    if go_files.is_empty() {
        return Ok(());
    }
    let dir_of = |path: &str| path.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
    // symbol -> (qualified name, file, language) for C-ABI exports.
    let mut c_exports: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    let mut go_exports: HashMap<(String, String), Vec<String>> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, file_path, language, json_extract(extra, '$.ffi_export.name') \
             FROM nodes WHERE json_extract(extra, '$.ffi_export.abi') = 'c'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (qualified, file_path, language, symbol) = row?;
            let language = language.unwrap_or_else(|| "c".to_string());
            if language == "go" {
                go_exports
                    .entry((dir_of(&file_path), symbol))
                    .or_default()
                    .push(qualified);
            } else {
                c_exports
                    .entry(symbol)
                    .or_default()
                    .push((qualified, file_path, language));
            }
        }
    }
    let libraries = load_native_libraries(tx)?;
    let mut c_dirs: HashSet<String> = HashSet::new();
    for (go_file, extra) in &go_files {
        let package_dir = dir_of(go_file);
        c_dirs.insert(package_dir.clone());
        let linked: HashSet<&String> = parse_json_column(extra.clone())?
            .get("cgo_libraries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .flat_map(|name| {
                let name = name.replace('-', "_");
                libraries
                    .iter()
                    .filter(move |lib| lib.lib_name == name)
                    .flat_map(|lib| &lib.sources)
            })
            .collect();
        for (caller, name, line, receiver) in calls_in_file(tx, go_file)? {
            if receiver.as_deref() != Some("C") {
                continue;
            }
            let Some(candidates) = c_exports.get(&name) else {
                continue;
            };
            // Package directory, then `-l` libraries, then anywhere.
            let scope = |evidence: &'static str, in_scope: &dyn Fn(&String) -> bool| {
                let hits: Vec<&(String, String, String)> = candidates
                    .iter()
                    .filter(|(_, file, _)| in_scope(file))
                    .collect();
                (!hits.is_empty()).then_some((evidence, hits))
            };
            let Some((evidence, hits)) = scope("cgo package", &|file| dir_of(file) == package_dir)
                .or_else(|| scope("cgo LDFLAGS", &|file| linked.contains(file)))
                .or_else(|| scope("symbol", &|_| true))
            else {
                continue;
            };
            let [(target, _, target_language)] = hits.as_slice() else {
                continue;
            };
            bridges.push(NewBridge {
                source: caller,
                target: target.clone(),
                file_path: go_file.clone(),
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
                    "go",
                    target_language,
                ),
            });
        }
    }
    // C calling Go: C files in a cgo package directory.
    let mut c_files = tx.prepare_cached(
        "SELECT DISTINCT file_path FROM nodes WHERE kind = 'File' \
           AND language IN ('c', 'cpp', 'objc') AND file_path LIKE ? || '%'",
    )?;
    for dir in &c_dirs {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let files = c_files
            .query_map([&prefix], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for c_file in files.iter().filter(|file| dir_of(file) == *dir) {
            let language = if c_file.ends_with(".c") { "c" } else { "cpp" };
            for (caller, target, line, _) in calls_in_file(tx, c_file)? {
                let name = call_name(&target).to_string();
                let Some(go_function) = unique(go_exports.get(&(dir.clone(), name.clone()))) else {
                    continue;
                };
                bridges.push(NewBridge {
                    source: caller,
                    target: go_function.clone(),
                    file_path: c_file.clone(),
                    line,
                    extra: bridge_extra(
                        "calls_native_function",
                        "ffi",
                        "manifest",
                        format!("cgo //export {name}"),
                        language,
                        "go",
                    ),
                });
            }
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
        let mut bridges = Vec::new();
        bind_jni_methods(&tx, &mut bridges)?;
        bind_wasm_imports(&tx, &mut bridges)?;
        bind_c_imports(&tx, &mut bridges)?;
        bind_cxx_bridges(&tx, &mut bridges)?;
        bind_cgo(&tx, &mut bridges)?;
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
            node_addon: None,
            emscripten: None,
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
            js_crate_for(&crates, "web/src/a.ts", "../../as/build/release.js"),
            Some(1)
        );
    }
}
