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
        // Passes every file's output goes through after its extractor
        // (`parse_file_in_repo`), so every language re-parses when they change.
        // 1: nodes sharing a qualified name merge into one
        // (`merged_declarations`), and CONTAINS edges start at a node of the
        // file (the File node for members of a type declared elsewhere).
        // Symbols with an empty or multi-line name or target (error recovery
        // in a file mid-edit) are dropped; an unparseable notebook keeps its
        // File node.
        extractor: "shared",
        version: 1,
        languages: &[
            "bash",
            "c",
            "cpp",
            "csharp",
            "dart",
            "elixir",
            "gdscript",
            "go",
            "java",
            "javascript",
            "julia",
            "kotlin",
            "lua",
            "markdown",
            "notebook",
            "objc",
            "perl",
            "php",
            "powershell",
            "python",
            "r",
            "ruby",
            "rust",
            "scala",
            "svelte",
            "swift",
            "terraform",
            "tsx",
            "typescript",
            "vue",
            "zig",
        ],
    },
    ExtractorVersion {
        // TypeScript / JavaScript rework (docs/TYPESCRIPT-EXTRACTION.md). Vue,
        // Svelte, and Astro script blocks run through the same extractor.
        // 2: symbols of a relative module missing from the repository are
        // `spec::name` with `unresolved_module`, not bare names; calls whose
        // first argument names a `.wasm` file emit `loads_wasm_module`.
        // 3: `require("bindings")("addon")` emits `loads_node_addon`.
        // 4: Emscripten `ccall("name")` / `cwrap("name")` emit
        // `calls_wasm_export`.
        // 5: `Deno.dlopen(...)` and Bun's `dlopen(...)` emit
        // `loads_shared_library`.
        extractor: "javascript",
        version: 5,
        languages: &["javascript", "typescript", "tsx", "vue", "svelte"],
    },
    ExtractorVersion {
        // 1: IMPORTS_FROM records the module as written and the names it
        // binds; CALLS on an import alias record the receiver.
        // 2: any `<receiver>.dlopen("lib")` (a cffi `FFI()` instance) emits
        // `loads_shared_library`.
        // 3: WebAssembly hosts (wasmtime / wasmer) emit `loads_wasm_module`
        // and `calls_wasm_export`.
        // 4: a call binds to a function nested in the caller before any other
        // of that name (`b`'s `helper()` bound to `a.helper`).
        extractor: "python",
        version: 4,
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
        // 6: `#[cxx::bridge]` `extern "C++"` declarations record `ffi_import`
        // and `extern "Rust"` declarations `ffi_export` (`abi: "cxx"`).
        // 7: UniFFI `#[uniffi::export]` items and `#[derive(uniffi::Object)]`
        // types record `ffi_export` (`abi: "uniffi"`), and the File node the
        // `setup_scaffolding!("ns")` namespace.
        // 8: WebAssembly hosts: a `.wasm` string argument emits
        // `loads_wasm_module`, export lookups by name `calls_wasm_export`.
        // 9: WebAssembly components: `impl exports::..::Guest` functions
        // record `ffi_export` (`abi: "wit"`), `impl ..::Host` functions
        // `abi: "wit_host"`, and wasmtime `call_<name>` calls emit
        // `calls_component_export`.
        // 10: a member of a type declared in another file is CONTAINED by the
        // File node, not by a `file::Type` node that does not exist.
        // Items in a function body are `fn.item`. `use` paths resolve to
        // module files (IMPORTS_FROM, with `names`, `glob`, `re_export`);
        // calls carry `receiver_type` / `module_file` / `receiver_unknown`,
        // macros are `name!`, and calls inside macro arguments are extracted.
        extractor: "rust",
        version: 10,
        languages: &["rust"],
    },
    ExtractorVersion {
        // 1: functions record WebAssembly exports (`//go:wasmexport`,
        // `//export`, `js.Global().Set("name", js.FuncOf(f))`).
        // 2: `//go:wasmimport module name` records `ffi_import`.
        // 3: `C.f(...)` calls record `receiver: "C"`, and the File node
        // records the cgo preamble's `-lNAME` libraries (`cgo_libraries`).
        // 4: WebAssembly hosts (wazero, wasmtime-go, wasmer-go) emit
        // `loads_wasm_module` (instead of `reads_file` for a `.wasm` path)
        // and `calls_wasm_export`.
        // 5: a member of a type declared in another file is CONTAINED by the
        // File node, not by a `file::Type` node that does not exist.
        // Types in a function body are `fn.Type`.
        extractor: "go",
        version: 5,
        languages: &["go"],
    },
    ExtractorVersion {
        // 1: functions with external C linkage record `ffi_export` (C and
        // Objective-C free functions that are not `static` or hidden, C++
        // functions inside `extern "C"`).
        // 2: functions registered as Node.js addon exports (N-API,
        // node-addon-api, NAN, `NODE_SET_METHOD`) record `ffi_exports` with
        // `abi: "napi"`.
        // 3: Python extension modules: the File node records
        // `python_module` (`PYBIND11_MODULE`, `NB_MODULE`, `PyInit_name`),
        // and pybind11 / nanobind `m.def` / `class_` and `PyMethodDef`
        // registrations record `ffi_exports` with `abi: "python"`.
        // 4: C++ test macros are named by their case: googletest `TEST(Suite,
        // Name)` is `Suite.Name`, Boost.Test by its first argument, and a
        // Catch2 / doctest `TEST_CASE("name") { }` is a Test node owning its
        // block's calls. `export namespace` misparsed under `#if` is no
        // longer a function `namespace`, and a base whose template arguments
        // contain `::` is named by the base, not the last argument segment.
        // Objective-C++ `.mm` files are parsed (as Objective-C). An
        // out-of-line `Widget::draw` whose class is declared in another file
        // is CONTAINED by the File node.
        extractor: "c_like",
        version: 4,
        languages: &["c", "cpp", "objc"],
    },
    ExtractorVersion {
        // 1: `native` methods record `ffi_import` with their JNI symbol.
        // 2: base classes and interfaces are named without type arguments
        // (`JpaRepository`, not `JpaRepository<User, Integer>`); a local
        // class is `Outer.method.Local`.
        extractor: "java",
        version: 2,
        languages: &["java"],
    },
    ExtractorVersion {
        // 1: `external fun` records `ffi_import` with its JNI symbol.
        // 2: bases are the constructed / named type (`B` of `B<String>()`),
        // not the last type argument.
        extractor: "kotlin",
        version: 2,
        languages: &["kotlin"],
    },
    ExtractorVersion {
        // 1: `[DllImport]` / `[LibraryImport]` methods emit a
        // `loads_shared_library` bridge carrying the C symbol they bind.
        // 2: calls on a PascalCase receiver (`Native.Total`) record
        // `receiver_type`.
        // 3: so do calls on a variable of a type declared in another file
        // (`var n = new Native(); n.Total()`) and `new Native()` itself.
        extractor: "csharp",
        version: 3,
        languages: &["csharp"],
    },
    ExtractorVersion {
        // 1: `ccall((:sym, "lib"), ...)` and `@ccall lib.sym(...)` name the
        // library and record the C `symbol`.
        // 2: a member of a type declared in another file is CONTAINED by the
        // File node, not by a `file::Type` node that does not exist.
        // Calls in a local `f(x) = ...` come from its `outer.f` node, and a
        // call binds to a function nested in the caller first.
        extractor: "julia",
        version: 2,
        languages: &["julia"],
    },
    ExtractorVersion {
        // 1: LuaJIT `ffi.load("lib")` emits `loads_shared_library`.
        // 2: a member of a type declared in another file is CONTAINED by the
        // File node, not by a `file::Type` node that does not exist.
        // `local function f` in a function body is `outer.f`.
        extractor: "lua",
        version: 2,
        languages: &["lua"],
    },
    ExtractorVersion {
        // 1: `DynamicLibrary.open("lib")` emits `loads_shared_library`, and
        // `@Native` externals record `ffi_import`.
        // 2: type arguments are no longer bases, and `implements` emits
        // IMPLEMENTS (role `implements`), `with` INHERITS (role `mixin`);
        // local functions are `outer.f`.
        extractor: "dart",
        version: 2,
        languages: &["dart"],
    },
    ExtractorVersion {
        // 1: type arguments of a base (`Q<Int>`) are no longer bases, and
        // nested functions are `outer.f`.
        extractor: "swift",
        version: 1,
        languages: &["swift"],
    },
    ExtractorVersion {
        // 1: a callee is a sub name; error recovery no longer turns an
        // expression (`input_avail && do { ... }`) into a call target.
        extractor: "perl",
        version: 1,
        languages: &["perl"],
    },
    ExtractorVersion {
        // 1: `fun.(x)` (an anonymous function in a variable) is not a call
        // edge with an empty target.
        extractor: "elixir",
        version: 1,
        languages: &["elixir"],
    },
    ExtractorVersion {
        // 1: a `def` nested in a function body is `outer.f`, local to it.
        extractor: "scala",
        version: 1,
        languages: &["scala"],
    },
    ExtractorVersion {
        // 1: `f <- function` in a function body is `outer.f`, local to it,
        // and a call binds to a function nested in the caller first.
        extractor: "r",
        version: 1,
        languages: &["r"],
    },
    ExtractorVersion {
        // 1: `import` / `moved` / `removed` blocks emit resolved REFERENCES
        // tagged with `terraform_kind`, not an IMPORTS_FROM to the provider's
        // import id or an edge between raw addresses.
        extractor: "terraform",
        version: 1,
        languages: &["terraform"],
    },
    ExtractorVersion {
        // 1: `export fn` records `ffi_export`, `extern fn` records
        // `ffi_import`, and calls through an `@cImport` constant record
        // `c_import`.
        // 2: a type declared in a function body is `fn.Type`, local to it.
        extractor: "zig",
        version: 2,
        languages: &["zig"],
    },
    ExtractorVersion {
        // 1: a call's name is its `method` (`Fast.fast_sum(...)` calls
        // `fast_sum`), not its first identifier (`xs.size` called `xs`).
        // 2: ffi gem `attach_function` defines the module method, recording
        // `ffi_import` with the `ffi_lib` library.
        // 3: calls on a constant (`Fast.fast_sum`) record `receiver_type`.
        extractor: "ruby",
        version: 3,
        languages: &["ruby"],
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
