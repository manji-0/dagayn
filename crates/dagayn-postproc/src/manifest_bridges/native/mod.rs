//! C / C++ shared libraries declared by build systems.
//!
//! A Python `ctypes.CDLL("libfastsum.so")`, a Java `System.loadLibrary`,
//! or a C# `NativeLibrary.Load` names a build output, not a source file. This
//! module reads the build files that produce such outputs and reports which
//! repository sources each shared library is compiled from:
//!
//! * CMake: `add_library(NAME SHARED|MODULE ...)` (or a plain
//!   `add_library` when `BUILD_SHARED_LIBS` is on), `target_sources`,
//!   `set` / `list(APPEND)` / `file(GLOB)` variables, and the
//!   `OUTPUT_NAME` target property;
//! * Meson: `shared_library`, `shared_module`, `both_libraries`, and
//!   `library` unless `default_library` is `static`;
//! * node-gyp: `binding.gyp` targets, whose `sources` build the Node.js
//!   addon `build/Release/<target_name>.node`;
//! * Cargo build scripts: `cc::Build::new().file(...).compile("name")` and
//!   `cxx_build::bridge(...)` chains in `build.rs`, whose sources are
//!   linked into the crate beside them;
//! * Zig: `build.zig` shared libraries (`addSharedLibrary`, or
//!   `addLibrary` with `.linkage = .dynamic`) built from a Zig root source,
//!   and the C sources `addCSourceFile` / `addCSourceFiles` compile into the
//!   build;
//! * Emscripten: `emcc` / `em++` command lines, whose `-o` names the
//!   JavaScript glue and `.wasm` module and whose `-sEXPORTED_FUNCTIONS`
//!   lists the C functions JavaScript may call;
//! * compiler command lines with `-shared` / `-dynamiclib` / `-bundle`
//!   in Makefile recipes (`$@` / `$^` / `$<` and simple variables
//!   expanded, object files mapped back to their sources), justfiles, and
//!   package.json scripts.
//!
//! Only sources that exist in the repository count; a library none of whose
//! sources can be located is not reported.

mod cc;
mod cmake;
mod commands;
mod glob;
mod gyp;
mod meson;
mod pyliteral;
mod zig;

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::data::load_json;
use super::text::{is_file, join_rel, parent_dir, resolve_rel};
use super::{Bridge, Confidence, ManifestBridgeResult, strings};

const C_SOURCE_SUFFIXES: &[&str] = &[".c", ".cc", ".cpp", ".cxx", ".c++", ".m", ".mm"];
const CPP_SUFFIXES: &[&str] = &[".cc", ".cpp", ".cxx", ".c++", ".mm"];
static SHARED_OUTPUT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.(?:so(?:\.\d+)*|dylib|dll|bundle)$").unwrap());

/// A shared library a build file compiles from repository sources.
#[derive(Debug)]
struct NativeLibrary {
    config_rel: String,
    /// "cmake" | "meson" | "make" | "node-gyp" | "emscripten" | "cc" | "cxx"
    /// | "zig" (a Zig shared library) | "zig-c" (C sources in a Zig build)
    build_system: &'static str,
    lib_name: String,
    sources: Vec<String>,
    /// Build outputs at known paths (node-gyp's `build/Release/NAME.node`,
    /// Emscripten's glue and `.wasm`).
    outputs: Vec<String>,
    /// Emscripten `EXPORTED_FUNCTIONS` without the leading `_`; `None` when
    /// the command does not list them.
    wasm_exports: Option<Vec<String>>,
}

impl NativeLibrary {
    fn new(
        config_rel: &str,
        build_system: &'static str,
        lib_name: String,
        sources: Vec<String>,
    ) -> Self {
        Self {
            config_rel: config_rel.to_string(),
            build_system,
            lib_name,
            sources,
            outputs: Vec::new(),
            wasm_exports: None,
        }
    }

    fn language(&self) -> &'static str {
        let has = |suffixes: &[&str]| {
            self.sources
                .iter()
                .any(|source| suffixes.iter().any(|suffix| source.ends_with(suffix)))
        };
        if has(&[".zig"]) {
            "zig"
        } else if has(CPP_SUFFIXES) {
            "cpp"
        } else if has(&[".m"]) {
            "objc"
        } else {
            "c"
        }
    }
}

/// `NAME` from `build/libNAME.so.1`, the form loaders are matched by.
fn library_stem(output: &str) -> String {
    let normalized = output.replace('\\', "/");
    let base = normalized.rsplit('/').next().unwrap_or("");
    let base = SHARED_OUTPUT_RE.replace(base, "");
    let base = base.strip_prefix("lib").unwrap_or(&base);
    base.replace('-', "_")
}

fn is_source(path: &str) -> bool {
    C_SOURCE_SUFFIXES
        .iter()
        .any(|suffix| path.ends_with(suffix))
}

fn existing_sources(repo_root: &Path, base: &str, items: &[String]) -> Vec<String> {
    let mut sources: Vec<String> = Vec::new();
    for item in items {
        if !is_source(item) || item.contains('$') {
            continue;
        }
        if let Some(rel) = resolve_rel(base, item)
            && is_file(repo_root, &rel)
            && !sources.contains(&rel)
        {
            sources.push(rel);
        }
    }
    sources
}

/// The build files native libraries are read from.
pub(super) struct NativeBuildFiles {
    pub cmake_lists: Vec<String>,
    pub meson_builds: Vec<String>,
    pub makefiles: Vec<String>,
    /// `(file, command lines)` for build files whose lines are plain shell
    /// commands (package.json scripts, justfiles).
    pub command_sources: Vec<(String, Vec<String>)>,
    pub binding_gyps: Vec<String>,
    pub cargo_build_scripts: Vec<String>,
    pub zig_builds: Vec<String>,
}

/// Shared libraries built from repository C / C++ / Objective-C sources,
/// merged per (build file, library name).
fn discover_native_libraries(repo_root: &Path, files: &NativeBuildFiles) -> Vec<NativeLibrary> {
    let mut libraries = Vec::new();
    for rel in &files.cmake_lists {
        libraries.extend(cmake::cmake_libraries(repo_root, rel));
    }
    for rel in &files.meson_builds {
        libraries.extend(meson::meson_libraries(repo_root, rel));
    }
    for rel in &files.binding_gyps {
        libraries.extend(gyp::gyp_libraries(repo_root, rel));
    }
    for rel in &files.cargo_build_scripts {
        libraries.extend(cc::cc_build_libraries(repo_root, rel));
    }
    for rel in &files.zig_builds {
        libraries.extend(zig::zig_libraries(repo_root, rel));
    }
    for rel in &files.makefiles {
        let Some(content) = super::text::read_text(&repo_root.join(rel)) else {
            continue;
        };
        let recipes = commands::make_recipe_commands(&content);
        libraries.extend(commands::command_libraries(repo_root, rel, &recipes));
    }
    for (rel, lines) in &files.command_sources {
        libraries.extend(commands::command_libraries(repo_root, rel, lines));
    }

    let mut merged: Vec<NativeLibrary> = Vec::new();
    for library in libraries {
        let existing = merged.iter_mut().find(|existing| {
            existing.config_rel == library.config_rel && existing.lib_name == library.lib_name
        });
        match existing {
            None => merged.push(library),
            Some(existing) => {
                for source in library.sources {
                    if !existing.sources.contains(&source) {
                        existing.sources.push(source);
                    }
                }
            }
        }
    }
    merged
}

/// Emit build file -> source edges for C / C++ shared libraries.
///
/// The edge carries the library name loaders are matched by (`libNAME.so`)
/// and every source file compiled into it, where native-binding resolution
/// looks for the exported C symbols.
pub(super) fn extract_native_libraries(
    repo_root: &Path,
    files: &NativeBuildFiles,
    result: &mut ManifestBridgeResult,
) {
    for library in discover_native_libraries(repo_root, files) {
        let source_language = match library.build_system {
            "cc" | "cxx" => "rust",   // build.rs
            "zig" | "zig-c" => "zig", // build.zig
            other => other,
        };
        result.ensure_file_node(&library.config_rel, source_language);
        // Command lines are build configuration; CMake / Meson / gyp declare
        // the library in a manifest.
        let from_command = matches!(
            library.build_system,
            "make" | "emscripten" | "cc" | "cxx" | "zig" | "zig-c"
        );
        let evidence_source = format!("{} library", library.build_system);
        let mut extra = Bridge {
            relationship_role: "builds_from_source",
            bridge_kind: "build_config",
            evidence_kind: if from_command { "config" } else { "manifest" },
            evidence_source: &evidence_source,
            source_language,
            target_language: library.language(),
            confidence: Confidence::High,
        }
        .extra();
        extra.insert("manifest_kind".to_string(), Value::from("native_library"));
        extra.insert(
            "build_system".to_string(),
            Value::from(library.build_system),
        );
        extra.insert(
            "lib_name".to_string(),
            Value::from(library.lib_name.clone()),
        );
        extra.insert("source_files".to_string(), strings(&library.sources));
        if library.build_system == "node-gyp" {
            extra.extend(gyp_addon_facts(repo_root, &library));
        } else if library.build_system == "emscripten" {
            extra.insert("wasm_outputs".to_string(), strings(&library.outputs));
            if let Some(exports) = &library.wasm_exports {
                extra.insert("wasm_exports".to_string(), strings(exports));
            }
        }
        result.push_edge(
            &library.config_rel,
            &library.sources[0],
            &library.config_rel,
            extra,
        );
    }
}

/// How JavaScript reaches a node-gyp addon: the package.json beside
/// `binding.gyp` names the package and its glue (`main`), and the
/// addon is loaded from `build/Release/NAME.node` or `bindings("NAME")`.
fn gyp_addon_facts(repo_root: &Path, library: &NativeLibrary) -> serde_json::Map<String, Value> {
    let package_dir = parent_dir(&library.config_rel);
    let data =
        load_json(&repo_root.join(join_rel(&package_dir, "package.json"))).unwrap_or_default();
    let name = data
        .get_str("name")
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let entry = data
        .get_str("main")
        .filter(|main| !main.trim().is_empty())
        .and_then(|main| resolve_rel(&package_dir, main));
    let mut facts = serde_json::Map::new();
    facts.insert("node_addon".to_string(), Value::from("node-gyp"));
    facts.insert(
        "js_packages".to_string(),
        strings(&name.map(str::to_string).into_iter().collect::<Vec<_>>()),
    );
    facts.insert(
        "js_entry_files".to_string(),
        strings(
            &entry
                .filter(|entry| !entry.ends_with(".node"))
                .into_iter()
                .collect::<Vec<_>>(),
        ),
    );
    facts.insert("node_outputs".to_string(), strings(&library.outputs));
    facts.insert("node_binary_names".to_string(), Value::Array(Vec::new()));
    facts
}
