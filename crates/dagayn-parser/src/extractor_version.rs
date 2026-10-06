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
        // 1: a `dagayn:` directive inside a code span or fence is an example
        // and creates no edge.
        extractor: "markdown",
        version: 1,
        languages: &["markdown"],
    },
    ExtractorVersion {
        // Passes every file's output goes through after its extractor
        // (`parse_file_in_repo`), so every language re-parses when they change.
        // 1: nodes sharing a qualified name merge into one
        // (`merged_declarations`), and CONTAINS edges start at a node of the
        // file (the File node for members of a type declared elsewhere).
        // Symbols with an empty or multi-line name or target (error recovery
        // in a file mid-edit) are dropped; an unparseable notebook keeps its
        // File node.
        // 2: same-file resolution leaves calls on a receiver typed by a class of
        // another file (`receiver_type`) to resolution across files.
        extractor: "shared",
        version: 2,
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
        // 6: Node.js builtin modules (`fs`, `node:fs`) and globals (`console`,
        // `JSON`, `setTimeout`) are the standard library: edges target the
        // package (`node:fs`, `globalThis`).
        // 7: calls into external packages carry `HIGH`.
        // 8: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 9: tree-sitter-vue ce8011a.
        extractor: "javascript",
        version: 9,
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
        // 5: calls and imports into the standard library target its package
        // (`subprocess`, `builtins`), marked `external` with the name in
        // `external_symbol`, and add no TESTED_BY.
        // 6: receivers are typed by parameter annotations and `self` attributes
        // (`receiver_type` for a class of another module, `receiver_unknown`
        // otherwise); calls and imports into third-party packages are
        // `external`; `_LAZY_EXPORTS`-style tables of a module `__getattr__`
        // are IMPORTS_FROM with the names they lend.
        // 7: names of relative imports (`from .graph import helper`) resolve
        // calls; pytest fixtures (`tmp_path`, `monkeypatch`) and third-party
        // types type their receivers.
        // 8: a member call on the result of another call records it
        // (`receiver_from`), for its declared return type to type the receiver.
        // 9: `super().m()` is typed by the enclosing class's first base
        // (`receiver_type`), or unknown without one, never the caller's own `m`.
        // 10: parsed with Ruff's parser instead of tree-sitter
        // (docs/plans/RUFF-PYTHON-PARSER.md): better recovery from syntax
        // errors, decoded string literals, union annotations type receivers,
        // and spans end at the last statement, not at trailing comments.
        extractor: "python",
        version: 10,
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
        // Types of other files are referenced; supertraits are INHERITS.
        // 11: calls and `use`s into the standard library target its crate
        // (`std`, `core`, `alloc`), marked `external`; calls keep the name in
        // `external_symbol`, `use`s the imported `paths`.
        // 12: calls and `use`s into the package's `Cargo.toml` dependencies target
        // the crate (`external`, `serde_json`).
        // 13: `name.workspace = true` dependencies count; types record their
        // `deref_target`; a destructuring `let Some(x)` binds no type; types
        // written with their crate (`tree_sitter::Node`) type their variables.
        // 14: functions record their `return_type`; a member call on the result of
        // another call records it (`receiver_from`, `unwrap` for `?` /
        // `.unwrap()`).
        // 15: calls in macro arguments record the call their receiver came
        // from, and a chain repeating a method (`.flag(a).flag(b)`) takes the
        // receiver before the repeats.
        // 16: a closure bound by `let` in a function body is a function of it
        // (`run.call`), whose calls are its own.
        // 17: enums record their `variants`.
        // 18: a function is a test by its attribute (`#[test]`,
        // `#[tokio::test]`, ...) or a `tests/` path, not by a `test` name
        // prefix: `test_node_json` and `tests_to_run` are helpers.
        // 19: the grammar carries local patches (vendor/grammar-patches/rust):
        // `~` in macro token trees, `where` on unit structs, and
        // `pub type` in extern blocks.
        extractor: "rust",
        version: 19,
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
        // 6: calls and imports into the standard library target its package
        // (`fmt`, `net/http`; predeclared functions `builtin`).
        // 7: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 8: a cgo call (`C.f()`, `receiver: "C"`) never binds to a function
        // of the file.
        extractor: "go",
        version: 8,
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
        // 5: standard headers and calls into them target `libc`, `posix`, `std`, or
        // the Apple framework (`Foundation`).
        // 6: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 7: tree-sitter-c b780e47 and tree-sitter-cpp c009222, on the tree-sitter
        // 0.27 runtime.
        extractor: "c_like",
        version: 7,
        languages: &["c", "cpp", "objc"],
    },
    ExtractorVersion {
        // 1: `native` methods record `ffi_import` with their JNI symbol.
        // 2: base classes and interfaces are named without type arguments
        // (`JpaRepository`, not `JpaRepository<User, Integer>`); a local
        // class is `Outer.method.Local`.
        // 3: calls and imports into the class library target the package
        // (`java.util`, `java.lang`), with the qualifier kept.
        // 4: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        extractor: "java",
        version: 4,
        languages: &["java"],
    },
    ExtractorVersion {
        // 1: `external fun` records `ffi_import` with its JNI symbol.
        // 2: bases are the constructed / named type (`B` of `B<String>()`),
        // not the last type argument.
        // 3: calls and imports into the standard library target the package
        // (`kotlin`, `java.io`).
        // 4: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 5: tree-sitter-kotlin 1852ea1.
        extractor: "kotlin",
        version: 5,
        languages: &["kotlin"],
    },
    ExtractorVersion {
        // 1: `[DllImport]` / `[LibraryImport]` methods emit a
        // `loads_shared_library` bridge carrying the C symbol they bind.
        // 2: calls on a PascalCase receiver (`Native.Total`) record
        // `receiver_type`.
        // 3: so do calls on a variable of a type declared in another file
        // (`var n = new Native(); n.Total()`) and `new Native()` itself.
        // 4: calls and `using`s into the base class library target the namespace
        // (`System`, `System.Collections.Generic`).
        // 5: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 6: tree-sitter-c-sharp 8c0abe0 (C# 14).
        // 7: a local grammar patch (vendor/grammar-patches/csharp) parses
        // `#if` around a binary operand (`a\n#if X\n&& b\n#endif`) and around
        // initializer elements.
        extractor: "csharp",
        version: 7,
        languages: &["csharp"],
    },
    ExtractorVersion {
        // 1: `ccall((:sym, "lib"), ...)` and `@ccall lib.sym(...)` name the
        // library and record the C `symbol`.
        // 2: a member of a type declared in another file is CONTAINED by the
        // File node, not by a `file::Type` node that does not exist.
        // Calls in a local `f(x) = ...` come from its `outer.f` node, and a
        // call binds to a function nested in the caller first.
        // 3: calls and imports into `Base` and the standard library target the
        // module (`Base`, `LinearAlgebra`).
        // 4: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        extractor: "julia",
        version: 4,
        languages: &["julia"],
    },
    ExtractorVersion {
        // 1: LuaJIT `ffi.load("lib")` emits `loads_shared_library`.
        // 2: a member of a type declared in another file is CONTAINED by the
        // File node, not by a `file::Type` node that does not exist.
        // `local function f` in a function body is `outer.f`.
        // 3: calls into the standard libraries target them (`string`, `_G`).
        // 4: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        extractor: "lua",
        version: 4,
        languages: &["lua"],
    },
    ExtractorVersion {
        // 1: `DynamicLibrary.open("lib")` emits `loads_shared_library`, and
        // `@Native` externals record `ffi_import`.
        // 2: type arguments are no longer bases, and `implements` emits
        // IMPLEMENTS (role `implements`), `with` INHERITS (role `mixin`);
        // local functions are `outer.f`.
        // 3: calls and imports into `dart:` libraries target them (`dart:core`,
        // `dart:io`).
        // 4: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 5: tree-sitter-dart be07cf7.
        extractor: "dart",
        version: 5,
        languages: &["dart"],
    },
    ExtractorVersion {
        // 1: type arguments of a base (`Q<Int>`) are no longer bases, and
        // nested functions are `outer.f`.
        // 2: calls and imports into the standard library and Apple frameworks
        // target them (`Swift`, `Foundation`).
        // 3: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 4: tree-sitter-swift 0.7.4 (82bb3a5), from its with-generated-files
        // branch.
        extractor: "swift",
        version: 4,
        languages: &["swift"],
    },
    ExtractorVersion {
        // 1: a callee is a sub name; error recovery no longer turns an
        // expression (`input_avail && do { ... }`) into a call target.
        // 2: calls and imports into core modules and builtins target them
        // (`POSIX`, `CORE`).
        // 3: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 4: tree-sitter-perl 2.x (04477ee).
        extractor: "perl",
        version: 4,
        languages: &["perl"],
    },
    ExtractorVersion {
        // 1: `fun.(x)` (an anonymous function in a variable) is not a call
        // edge with an empty target.
        // 2: calls and imports into the standard library target the module
        // (`Enum`, `IO`, `:lists`, `Kernel`).
        // 3: tree-sitter-elixir 4b0c711.
        extractor: "elixir",
        version: 3,
        languages: &["elixir"],
    },
    ExtractorVersion {
        // 1: a `def` nested in a function body is `outer.f`, local to it.
        // 2: calls and imports into the standard library target the package
        // (`scala`, `scala.collection.mutable`).
        // 3: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 4: tree-sitter-scala db390f3.
        extractor: "scala",
        version: 4,
        languages: &["scala"],
    },
    ExtractorVersion {
        // 1: `f <- function` in a function body is `outer.f`, local to it,
        // and a call binds to a function nested in the caller first.
        // 2: calls and imports into base R packages target them (`base`, `stats`).
        // 3: tree-sitter-r 58a2279.
        extractor: "r",
        version: 3,
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
        // 3: `@import("std")` and calls through it target `std`.
        // 4: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        extractor: "zig",
        version: 4,
        languages: &["zig"],
    },
    ExtractorVersion {
        // 1: a call's name is its `method` (`Fast.fast_sum(...)` calls
        // `fast_sum`), not its first identifier (`xs.size` called `xs`).
        // 2: ffi gem `attach_function` defines the module method, recording
        // `ffi_import` with the `ffi_lib` library.
        // 3: calls on a constant (`Fast.fast_sum`) record `receiver_type`.
        // 4: calls and requires into core and the standard library target them
        // (`core`, `json`).
        // 5: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        extractor: "ruby",
        version: 5,
        languages: &["ruby"],
    },
    ExtractorVersion {
        // 1: shell builtins (`echo`, `printf`) are calls into `bash`.
        extractor: "bash",
        version: 1,
        languages: &["bash"],
    },
    ExtractorVersion {
        // 1: global functions, built-in types, and engine classes (`print`,
        // `Vector2`, `extends Node`) target `godot`.
        // 2: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 3: tree-sitter-gdscript 8ecb27e.
        extractor: "gdscript",
        version: 3,
        languages: &["gdscript"],
    },
    ExtractorVersion {
        // 1: builtin functions and classes (`strlen`, `\DateTime`) target `php`.
        // 2: member calls record what types their receiver (`receiver_type` for a
        // class of another file, `receiver_unknown`, and `receiver_from` for the
        // call it came from) and functions their declared `return_type`.
        // 3: tree-sitter-php 92b5271.
        extractor: "php",
        version: 3,
        languages: &["php"],
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
