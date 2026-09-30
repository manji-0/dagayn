//! Output versions of the extractors.
//!
//! A graph records the versions it was parsed with (graph metadata key
//! `extractor_versions`). When an extractor's version moves past the stored
//! one, the next incremental update re-parses every file that extractor
//! owns, even though the files themselves did not change: without that,
//! unchanged files would keep nodes and edges under qualified names the new
//! extractor no longer produces.

/// One extractor's output version and the file languages it parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractorVersion {
    /// Stable extractor name used in graph metadata (`javascript=1`).
    pub extractor: &'static str,
    /// Bump when a change renames qualified names or otherwise changes the
    /// nodes / edges produced for unchanged source.
    pub version: u32,
    /// `detect_language` values of the files this extractor parses.
    pub languages: &'static [&'static str],
}

/// Extractors with a tracked output version. Extractors not listed here are
/// treated as never changing their output.
pub const EXTRACTOR_VERSIONS: &[ExtractorVersion] = &[
    ExtractorVersion {
        // TypeScript / JavaScript rework (docs/TYPESCRIPT-EXTRACTION.md). Vue,
        // Svelte, and Astro script blocks run through the same extractor.
        // 2: symbols of a relative module missing from the repository are
        // `spec::name` with `unresolved_module`, not bare names; calls whose
        // first argument names a `.wasm` file emit `loads_wasm_module`.
        // 3: `require("bindings")("addon")` emits `loads_node_addon`.
        // 4: Emscripten `ccall("name")` / `cwrap("name")` emit
        // `calls_wasm_export`.
        extractor: "javascript",
        version: 4,
        languages: &["javascript", "typescript", "tsx", "vue", "svelte"],
    },
    ExtractorVersion {
        // 1: IMPORTS_FROM records the module as written and the names it
        // binds; CALLS on an import alias record the receiver.
        extractor: "python",
        version: 1,
        languages: &["python", "notebook"],
    },
    ExtractorVersion {
        // 1: functions and types record `ffi_export` (`#[pyfunction]`,
        // `#[pyclass]`, `#[pymethods]`, `#[no_mangle]`, `#[export_name]`).
        // 2: also `#[wasm_bindgen]` items and `pub` methods of a
        // `#[wasm_bindgen] impl`.
        // 3: also napi-rs (`#[napi]`) and neon (`#[neon::export]`,
        // `cx.export_function`) Node.js addon exports.
        // 4: declarations in a `#[wasm_bindgen] extern "C"` block record
        // `ffi_import` instead of `ffi_export`.
        // 5: declarations in a C-ABI `extern` block record `ffi_import`
        // (`abi: "c"`, `#[link_name]`, `#[link(name)]`).
        extractor: "rust",
        version: 5,
        languages: &["rust"],
    },
    ExtractorVersion {
        // 1: functions record WebAssembly exports (`//go:wasmexport`,
        // `//export`, `js.Global().Set("name", js.FuncOf(f))`).
        // 2: `//go:wasmimport module name` records `ffi_import`.
        extractor: "go",
        version: 2,
        languages: &["go"],
    },
    ExtractorVersion {
        // 1: functions with external C linkage record `ffi_export` (C and
        // Objective-C free functions that are not `static` or hidden, C++
        // functions inside `extern "C"`).
        // 2: functions registered as Node.js addon exports (N-API,
        // node-addon-api, NAN, `NODE_SET_METHOD`) record `ffi_exports` with
        // `abi: "napi"`.
        extractor: "c_like",
        version: 2,
        languages: &["c", "cpp", "objc"],
    },
    ExtractorVersion {
        // 1: `native` methods record `ffi_import` with their JNI symbol.
        extractor: "java",
        version: 1,
        languages: &["java"],
    },
    ExtractorVersion {
        // 1: `external fun` records `ffi_import` with its JNI symbol.
        extractor: "kotlin",
        version: 1,
        languages: &["kotlin"],
    },
    ExtractorVersion {
        // 1: `[DllImport]` / `[LibraryImport]` methods emit a
        // `loads_shared_library` bridge carrying the C symbol they bind.
        extractor: "csharp",
        version: 1,
        languages: &["csharp"],
    },
];

/// The tracked extractor versions.
pub fn extractor_versions() -> &'static [ExtractorVersion] {
    EXTRACTOR_VERSIONS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extractor_versions_are_unique_and_positive() {
        let mut names = std::collections::HashSet::new();
        for entry in extractor_versions() {
            assert!(names.insert(entry.extractor), "{}", entry.extractor);
            assert!(entry.version > 0, "{}", entry.extractor);
            assert!(!entry.languages.is_empty(), "{}", entry.extractor);
        }
    }
}
