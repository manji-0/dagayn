# Call resolution

<!-- constrained-by ./SCHEMA.md#edges -->
<!-- derived-from ./ARCHITECTURE.md#post-processing -->

How a `CALLS` (and related `IMPORTS_FROM` / `REFERENCES`) edge gets its
target: a node of the repository, a named package outside it, or, when
nothing settles it, the bare name as written. This covers the metadata the
extractors record for it, the post-processing passes that read that
metadata, and what the resulting confidence tiers mean.

TypeScript / JavaScript specifics beyond this document (owner paths,
`pkg::symbol` targets, object-literal containers) are in
[TYPESCRIPT-EXTRACTION.md](./TYPESCRIPT-EXTRACTION.md).

## Outcomes

<!-- constrained-by ./SCHEMA.md#edges -->

Every call edge ends in one of three states:

| State | Target | Tier | Example |
|---|---|---|---|
| Resolved | a node QN (`file::Owner.name`) | `EXTRACTED`, `HIGH`, or `MEDIUM` | `store.upsert_node()` -> `crates/dagayn-py/src/lib.rs::PyGraphStore.upsert_node` |
| External | the package (`subprocess`, `std`, `rusqlite`), `extra.external: true` | `HIGH` or `MEDIUM` | `subprocess.run(cmd)` -> `subprocess` |
| Unresolved | the bare name as written | `LOW` | `obj.frobnicate()` with `obj` of unknown type |

`LOW` therefore means "nothing in the graph or in a known package
accounts for this call". A call into a package is not `LOW`: its target is
never a node, but it is not unknown.

## Edge metadata

<!-- derived-from #outcomes -->

Keys of `extra` on call-related edges. "Extractor" means the per-language
parser; "post-processing" means the passes in
[Post-processing passes](#post-processing-passes).

| Key | Set by | Meaning |
|---|---|---|
| `external` | extractor, post-processing | The target is a package outside the repository, not a node. Same-file and bare-name resolution never rebind such an edge. |
| `stdlib` | extractor, post-processing | The package is the language's standard library. Absent for third-party packages. |
| `external_package` | extractor, post-processing | The package, as the language names it (see [Package names](#package-names)). Equal to the target. |
| `external_symbol` | extractor, post-processing | What was called, as written or resolved: `subprocess.run`, `pathlib.Path.read_text`, `Vec::new`, `format!`, `http.Client.Get`. Rust chains spell the receiver's call: `PathBuf::canonicalize()::ok()::map`. |
| `confidence_tier` / `confidence` | extractor, post-processing | `HIGH` / `0.9` or `MEDIUM` / `0.6`; see [Confidence tiers](#confidence-tiers). |
| `paths` | extractor | On a Rust `IMPORTS_FROM` to a crate: the paths one `use` imports from it (`["std::collections::HashMap", "std::collections::HashSet"]`). |
| `inferred_from` | post-processing | `"observed_method"` when the package was inferred from typed calls of the same method name; `"return_table"` when the receiver was typed by a table of package return types (`re.match(..)` a `re.Match`). |
| `receiver_type` | extractor, post-processing | The receiver's type when it is a class of another file (`store: GraphStore`). The target is the bare method name until resolution binds it. |
| `receiver_unknown` | extractor | The receiver's type is unknown (`x.m()` with `x` untyped). The call is never bound to a same-named function of the file. |
| `receiver_from` | extractor | `{"call": name, "line": L, "unwrap": bool}`: the receiver is the result of the call `name` on line `L` of the same caller (`store_conn(s).execute()`, or a variable assigned from it). `unwrap` is set when the result was taken out of a wrapper (`?`, `.unwrap()`, `await`, `try`, Go's first of several results). |
| `enum_variant` | post-processing | On a Rust call of an enum variant (`Kind::File(..)`) bound to the enum node: the variant. |
| `ffi_abi` | post-processing | `"pyo3"` on a Python call bound to a Rust `#[pymethods]` method. |
| `alias`, `module_file`, `path` | extractor | Rust: the alias a `use ... as` gave the callee; the module file of a `module::f()` path. |

Node metadata the passes read:

| Node key | Set by | Meaning |
|---|---|---|
| `return_type` (column) | extractor | The declared return type as written (`sqlite3.Connection`, `Result<Self>`, `(*Store, error)`, `Promise<User>`, `Store?`). |
| `extra.variants` | Rust extractor | An enum's variant names. |
| `extra.deref_target` | Rust extractor | What a type dereferences to (`impl Deref for FilePath { type Target = str; }` gives `str`; a slice is `slice`). |
| `extra.rust_kind: "closure"` | Rust extractor | A function node for a `let`-bound closure (`run.call`). |
| `extra.ffi_export` (`abi: "pyo3"`) | Rust extractor | A `#[pyclass]` / `#[pymethods]` item and its Python name. |

## Confidence tiers

<!-- constrained-by ./SCHEMA.md#edges -->

External edges carry the tier their evidence gives them:

- **`HIGH` (certain):** the call is written through the package itself: an
  import of it, a path rooted at it (`std::fs::read`, `fmt.Println`,
  `\strlen`), a name imported from it, or a member of a value it just
  constructed (`Path(p).read_text()`). Rust crates and JavaScript packages
  named through an import are `HIGH` too.
- **`MEDIUM` (likely):** the package is inferred: a builtin or prelude name
  the file does not shadow (`len`, `println`, `console.log`), a method of a
  variable typed by its binding, a Python module that is neither standard
  nor in the repository, a method name only a standard type defines, or one
  that typed calls of the method reach.

`demote_unresolved_endpoint_edges` lowers an edge whose source or target is
not a node to `LOW`, except an `external` edge whose extractor gave it a
`confidence_tier`. Calls bound to a node by post-processing take the tier of
the pass that bound them.

## Package names

<!-- derived-from #edge-metadata -->

Calls and imports into the standard library target the package below, with
`stdlib: true`. The tables behind each row live in
`crates/dagayn-parser/src/stdlib/<language>.rs`.

| Language | Package | `HIGH` | `MEDIUM` |
|---|---|---|---|
| Python | top-level module (`subprocess`, `os` for `os.path`), `builtins` | imports; calls through an import of a standard module or a name imported from one; members of a value a standard class or factory (`logging.getLogger`, `re.compile`) constructed | bare builtins the file does not shadow; variables bound to a standard value; pytest fixtures (`tmp_path` is `pathlib.Path`) |
| Rust | `std`, `core`, `alloc` | `std::` paths; names a `use` of the standard library brought in; prelude names and `std` macros when the file has no glob import | the same with a glob import in the file; methods of variables typed by their binding |
| Go | import path (`fmt`, `net/http`); `builtin` | imports and calls through the import name | predeclared functions (`len`, `append`) |
| Zig | `std`, `builtin` | `@import("std")` and calls through constants bound to it | — |
| Bash | `bash` | — | shell builtins (`echo`, `printf`) the file defines no function for |
| Lua | the library (`string`, `table`, `os`); `_G` | `string.format(...)` when the file does not rebind `string` | base functions (`print`, `pairs`) |
| Java | the package (`java.util`, `java.lang`) | imports, static imports, `java.lang` classes, fully qualified calls, members of `new T()` | variables typed by their declaration |
| Kotlin | `kotlin`, imported packages (`kotlin.math`, `java.io`) | imported names and their constructors | auto-imported functions (`println`, `listOf`) |
| Scala | `scala`, imported packages | import-bound names, `scala.` / `java.` paths | `Predef` / `scala._` names (`println`, `List`) |
| C# | the namespace (`System`, `System.Collections.Generic`) | a `using` of it, fully qualified calls, `System` types, keyword types (`string.Join`) | other base-class-library types (implicit global usings) |
| JavaScript / TypeScript | `node:<module>` (`fs` and `node:fs` alike); `globalThis` | Node.js builtin modules and their imports | globals (`console.log`, `JSON.parse`, `new Map()`) the file does not declare |
| Dart | the `dart:` library (`dart:io`); `dart:core` | `dart:` imports and calls through their prefix | `dart:core` names; unprefixed names of an imported `dart:` library |
| Swift | `Swift`; the framework (`Foundation`, `UIKit`) | framework imports; module-qualified calls (`Swift.print`) | standard names (`print`, `String`); framework names when the framework is imported |
| GDScript | `godot` | — | global functions, built-in types, engine classes (`print`, `Vector2`, `Node`) |
| C | `libc` (ISO C), `posix` | standard `#include <...>`; a function whose declaring header is included | a known function whose header is not included |
| C++ | `std` | standard headers; `std::` calls | known functions under `using namespace std`; methods of `std::`-typed variables |
| Objective-C | the framework (`Foundation`, `UIKit`) | `#import <Framework/...>` / `@import`; framework classes (`NS*`, `UI*`) when the framework is imported | the same without the import |
| PHP | `php` | fully qualified `\strlen`; `use DateTime;` | unqualified builtin functions; builtin classes in a file without a namespace |
| Ruby | the `require` name (`json`, `net`); `core` | `require` of a shipped library; core classes (`File.read`); library constants when the file requires the library | `Kernel` methods (`puts`, `raise`); library constants without the `require` |
| Perl | the core module (`POSIX`); `CORE` | `use` / `require`; `Module::f`; names in `qw(...)` | builtin functions (`print`, `push`) |
| R | the base package (`base`, `stats`) | `library()` / `require()`; `pkg::f` | bare functions of `base`, `stats`, `utils` |
| Julia | `Base`, `Core`, the stdlib module (`LinearAlgebra`) | `using` / `import`; `Module.f`; names imported one by one | bare `Base` exports; exports of a whole-module `using` |
| Elixir | the module (`Enum`, `IO.ANSI`); Erlang modules (`:lists`); `Kernel` | `Module.f`; directives; `import X, only:` names | bare `Kernel` functions |

A name the file declares, or imports from the repository, is never the
standard library's. A Go package of the repository's own module (the
`module` line of `go.mod`) is never standard, even under a root like
`crypto/`.

### Third-party packages

<!-- derived-from #package-names -->

Calls and imports into packages that are neither the repository nor the
standard library are `external` without `stdlib`:

- **Rust:** crates the package's `Cargo.toml` depends on (`[dependencies]`,
  `[dev-dependencies]`, `[build-dependencies]`, `name.workspace = true`),
  named through a path or a `use` of one (`serde_json::to_string`,
  `node.kind()` with `use tree_sitter::Node`, or a variable typed
  `tree_sitter::Node<'_>`). Workspace crates of the repository are not
  external. `HIGH`.
- **JavaScript / TypeScript:** bare import specifiers keep the
  `pkg::symbol` target described in TYPESCRIPT-EXTRACTION.md. `HIGH`.
- **Python:** modules that resolve neither to a file of the repository (by
  walking up from the importing file, and under `src/`) nor to the standard
  library (`yaml.safe_load`, `from pytest import raises`). `MEDIUM`: the
  module may be the repository's under a layout the resolver misses.

## Receiver typing

<!-- constrained-by #edge-metadata -->

Member calls (`x.m()`, `x->m()`, `[x m]`, `$x->m()`, `x:m()`) are typed by
what the syntax of the file says about `x`:

1. A class of the same file: the call binds to its method in the file.
2. A class of another file: `receiver_type`, bound across files by type.
3. A standard-library or package type: the call is `external` to it.
4. Otherwise `receiver_unknown`, plus `receiver_from` when `x` is a call's
   result or a variable assigned from one.

What types a receiver, by language:

| Language | Typed by |
|---|---|
| Python | parameter annotations (`Optional[T]`, `T \| None`, `"T"`), `self.x` assigned a constructor or annotated parameter, class-body annotations, constructor calls, pytest fixture names in tests and `conftest.py` |
| Rust | parameter and `let` types, `Type::new()` / `?` / `.unwrap()` bindings, enum variants, fields of structs of the file, types written with their crate (`tree_sitter::Node<'_>`) |
| JavaScript / TypeScript | annotated parameters, variables, and class fields (`constructor(private store: GraphStore)`) |
| Java, Kotlin, Scala, C# | parameters, locals, fields and properties, `new T()` / constructor calls, casts, `var x = new T()` |
| Go | method receivers, parameters, `var x T`, `T{}`, `&T{}`, `new(T)`, `x.(T)`, struct fields of the file |
| Swift, Dart | parameters, annotated locals, initializer calls, stored properties / fields, `self.` / `this.` |
| C++, Objective-C | locals, parameters, fields of the owning class (also in out-of-line methods), smart pointers (by pointee), `new` / `make_unique`, `[[T alloc] init]` |
| PHP | typed parameters, typed and promoted properties, `$x = new T()` |
| Ruby, Lua, Perl | constructor calls (`Store.new`, `Store:new()`, `Store->new`) |
| GDScript, Zig, Julia | annotations (`var s: Store`, `const s: Store`, `s::Store`), constructors (`Store.new()`, `Store.init(..)`) |

Elixir and R calls name modules or functions, not methods of values; they
carry no receiver metadata.

A chain repeating a method (`Build::new().flag(a).flag(b)`) records the
receiver before the repeats: one line holds a single edge per target.

Python `super().m()` is typed by the enclosing class's first base
(`receiver_type`), or `receiver_unknown` when the class names none; it
never binds to the caller's own `m`. A call on an imported module
(`receiver`, Python `query.run()`, Go's cgo `C.f()`) never binds to a
function of the file.

## Post-processing passes

<!-- derived-from ./ARCHITECTURE.md#post-processing -->

`resolve_bare_call_targets` (the "Bare-name edge resolution (calls)" step of
both the native pipeline and the Python step-by-step path) runs, in one
transaction:

1. **Re-exports.** A target `pkg/__init__.py::Name` that is no node follows
   the file's imports (named, star, `_LAZY_EXPORTS`-style tables of a module
   `__getattr__`, and a class `__getattr__` defines) to the declaration.
   `HIGH`.
2. **Bare names.** A bare target binds through its module path, its
   `receiver_type`, or import visibility (Python `__init__.py` imports
   count as re-exports), as [SCHEMA.md](./SCHEMA.md#edges) describes. Calls
   with `receiver_from` wait for step 4; Rust calls with `receiver_unknown`
   are never bound by name, and those of other languages never to a member
   of a class or function enclosing the caller (`this.item.show()` is not
   the caller class's `show`).
3. **Enum variants.** A Rust call of a variant (`receiver_type` naming an
   enum whose `variants` holds the name) binds to the enum. `HIGH`.
4. **Twice, so a call typed in the first round can type the next:**
   - *Return types.* A `receiver_from` call follows its origin call to the
     function it resolved to, reads its `return_type` (unwrapping `Result`,
     `Option`, `Optional`, `Promise`, `Task`, `Future` when `unwrap`;
     `Self` / `self` / `static` / `instancetype` / `this` name the owner),
     and binds the method of a class of the repository or marks the call
     external to the type's package (the declaring file's imports, glob
     imports included, package qualifiers such as `http.Client` against
     `net/http`, and each language's built-in types). A Rust method of what
     a package call returned belongs to that package, except through a
     cell or lock guard (`RefCell::borrow`, `borrow_mut`, `Mutex::lock`,
     `RwLock::read` / `write`), whose value derefs to its contents
     (`bindings.borrow().snapshot()` is not `std`'s). In Python and
     JavaScript / TypeScript, a package call of known return type types its
     result by a table (`re.match` a `re.Match`, `execute` a
     `sqlite3.Cursor`, `vscode.workspace.getConfiguration` a
     `vscode.WorkspaceConfiguration`, d3's `select` a `d3.Selection` that
     `.attr(..)` and `.append(..)` return again), `inferred_from:
     "return_table"`; a method on what any other package call returned is
     left to the passes below, since it is often a builtin's
     (`path.read_text().splitlines()`, `fs.readFileSync(p).toString()`).
     `MEDIUM`.
   - *PyO3.* A Python call on a `receiver_type` that a Rust
     `#[pyclass(name = ...)]` exports binds to its `#[pymethods]` method.
   - *Glob-imported crate names.* A Rust name a `use super::*` brings in from
     a module that imported it from a crate points at that crate.
   - *Deref and `Result`.* A Rust method a repository type lacks is that of
     its `deref_target` (`file_path.as_str()` is `str`'s), or of the
     `Result` a fallible constructor returns (`Store::open(p).expect(..)`).
   - *Standard-type methods.* A `receiver_unknown` call whose name is a
     method of a standard type (`iter`, `strip`, `get`, `map`) points at the
     standard library, unless a function of that name is visible to the
     calling file (its own, or one of a file it imports).
5. **Bare names again** for `receiver_from` calls step 4 could not type
   (an untyped JavaScript `makeBox()`).
6. **Observed methods.** A `receiver_unknown` call takes the package that at
   least two, and at least 90 %, of the typed calls of the same method name
   reach (`child.kind()` is `tree_sitter`'s), unless a function of that
   name is visible to the calling file. Only receivers whose type is written
   count as observations, not those a return table typed (a
   `WorkspaceConfiguration.get` would take every untyped `map.get(key)`). `MEDIUM`, `inferred_from: "observed_method"`.
7. **Return types again**, for `receiver_from` calls whose origin step 6
   typed (`conn.prepare(..)?.query_map(..)` once `prepare` is `rusqlite`'s).
8. **`TESTED_BY`**: see below.

Native-binding resolution, which runs later in the pipeline, reads the
called name of an `external` Python call from `external_symbol`, so calls
into an extension module the repository builds still bridge to it.

## TESTED_BY

<!-- constrained-by ./SCHEMA.md#edges -->

`TESTED_BY target -> test` follows the calls a test makes once resolution
is done:

- An edge whose tested symbol is not a node (a bare `helper`, a package) is
  dropped; it says nothing about code of the repository.
- A call a test makes to a node that has no `TESTED_BY` gets one, with the
  call's tier, including a call a later update resolved.
- Calls to assertion / mock APIs (`test_api`) and to external packages make
  none.

Dead-code analysis reads a test's unresolved calls from its bare `CALLS`
edges, not from `TESTED_BY`.

## Guarantees for consumers

<!-- derived-from #outcomes -->

- `callers_of("<package>")` lists the callers of a package
  (`callers_of("subprocess")`, `callers_of("rusqlite")`), as it does for a
  JavaScript `pkg::symbol`.
- An `external` edge never shares a name with a repository symbol for
  query-time fallbacks or dead-code analysis.
- `LOW` is reserved for calls nothing settled; `MEDIUM` marks inference,
  `HIGH` evidence written in the code.

## Known limitations

<!-- derived-from #receiver-typing -->

- Typing is lexical: variables are tracked per function in most languages,
  not per block or per control-flow path.
- A method name a repository function and a standard type share (`as_str`
  of a repository enum and of `String`) stays unresolved without a typed
  receiver.
- Go: methods promoted from embedded structs carry the outer struct's
  `receiver_type`; package-level untyped variables are never marked.
- Swift, Dart, GDScript, Objective-C: a capitalized name counts as a type.
- Objective-C: `self.prop` and ivars are not typed.
- Python: unannotated parameters stay unknown; `PyO3` binding needs the
  Python receiver type to equal the `#[pyclass]` name.
- Inferred packages (`MEDIUM`) may be wrong when a value of another
  package has a method of the same name (pandas' `df.append()` as
  `builtins`).

## Measured effect

<!-- derived-from #post-processing-passes -->

On this repository (Rust, Python, TypeScript, and fixtures of every
language), unresolved (`LOW`) edges went from about 47,000 to about 4,000,
of which about 11,000 were mirrored `TESTED_BY` edges and about 18,000 calls
into standard libraries.
