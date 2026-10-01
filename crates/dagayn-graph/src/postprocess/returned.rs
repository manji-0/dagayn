//! Member calls on the result of another call, typed by what the called
//! function declares it returns.
//!
//! `store_conn(store).execute(...)`, `conn = store_conn(store);
//! conn.execute(...)`, `Store::open(p)?.connection().prepare(...)`: the
//! extractor sees one file, so the receiver's type is unknown there
//! (`receiver_unknown`), but it records the call the receiver came from
//! (`receiver_from`: name, line, unwrapped). Once resolution has bound that
//! call to a function of the repository, its declared return type
//! (`-> sqlite3.Connection`, `-> Result<Self>`) types the receiver: a class
//! of the repository resolves the method, a type of the standard library or
//! of a package points the call at it. A call bound this way can type the
//! next one in a chain, so the pass repeats until nothing changes. A call
//! the origin binds to a package types the receiver too: a Rust method of
//! what a package returned is one of its types, and a Python or JavaScript
//! package function of known return type (`re.match` a `re.Match`,
//! `vscode.workspace.getConfiguration` a `vscode.WorkspaceConfiguration`)
//! types its result.
//!
//! Python calls on a class that a Rust extension defines (`#[pyclass(name =
//! "GraphStore")]`, `ffi_export` `abi: "pyo3"`) bind to its `#[pymethods]`.

use crate::helpers::*;
use crate::*;

use super::bare_names::{INFERRED_CONFIDENCE, import_targets_tx, language_family};

/// How many links of a call chain are followed.
const MAX_ROUNDS: usize = 6;

/// Python builtin types a function may return.
const PYTHON_BUILTIN_TYPES: &[&str] = &[
    "bool",
    "bytearray",
    "bytes",
    "dict",
    "float",
    "frozenset",
    "int",
    "list",
    "set",
    "str",
    "tuple",
];

/// Rust standard types a function may return without a path.
const RUST_STD_TYPES: &[&str] = &[
    "Arc", "BTreeMap", "BTreeSet", "Box", "Cell", "Cow", "Duration", "HashMap", "HashSet",
    "Instant", "Mutex", "Option", "OsString", "Path", "PathBuf", "Rc", "RefCell", "Result",
    "RwLock", "String", "Vec", "VecDeque", "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64",
    "i128", "isize", "str", "u8", "u16", "u32", "u64", "u128", "usize",
];

/// Python standard-library functions by `(package, function)` and the type
/// they return. A method on what any other one returned is left alone: the
/// result is often a builtin (`path.read_text().splitlines()` is `str`'s,
/// `json.loads(text).get(..)` a `dict`'s), or anything at all
/// (`importlib.import_module`, `pytest.importorskip`). `execute` is
/// `sqlite3.Connection`'s and `sqlite3.Cursor`'s alike; `match` is `re`'s
/// and `re.Pattern`'s.
const PYTHON_STDLIB_RETURNS: &[(&str, &str, &str)] = &[
    ("argparse", "ArgumentParser", "argparse.ArgumentParser"),
    ("argparse", "add_argument_group", "argparse._ArgumentGroup"),
    (
        "argparse",
        "add_mutually_exclusive_group",
        "argparse._MutuallyExclusiveGroup",
    ),
    ("argparse", "add_parser", "argparse.ArgumentParser"),
    ("argparse", "add_subparsers", "argparse._SubParsersAction"),
    ("hashlib", "blake2b", "hashlib.blake2b"),
    ("hashlib", "md5", "hashlib._Hash"),
    ("hashlib", "new", "hashlib._Hash"),
    ("hashlib", "sha1", "hashlib._Hash"),
    ("hashlib", "sha256", "hashlib._Hash"),
    ("hashlib", "sha512", "hashlib._Hash"),
    ("logging", "getLogger", "logging.Logger"),
    ("pathlib", "Path", "pathlib.Path"),
    ("pathlib", "absolute", "pathlib.Path"),
    ("pathlib", "expanduser", "pathlib.Path"),
    ("pathlib", "joinpath", "pathlib.Path"),
    ("pathlib", "open", "io.IOBase"),
    ("pathlib", "relative_to", "pathlib.Path"),
    ("pathlib", "resolve", "pathlib.Path"),
    ("pathlib", "with_name", "pathlib.Path"),
    ("pathlib", "with_suffix", "pathlib.Path"),
    ("re", "compile", "re.Pattern"),
    ("re", "fullmatch", "re.Match"),
    ("re", "match", "re.Match"),
    ("re", "search", "re.Match"),
    ("sqlite3", "connect", "sqlite3.Connection"),
    ("sqlite3", "cursor", "sqlite3.Cursor"),
    ("sqlite3", "execute", "sqlite3.Cursor"),
    ("sqlite3", "executemany", "sqlite3.Cursor"),
    ("sqlite3", "executescript", "sqlite3.Cursor"),
    ("subprocess", "Popen", "subprocess.Popen"),
    ("subprocess", "run", "subprocess.CompletedProcess"),
    ("threading", "Thread", "threading.Thread"),
];

/// JavaScript / TypeScript package functions by `(package, function)` and
/// the type they return, on the same terms as [`PYTHON_STDLIB_RETURNS`]
/// (`fs.readFileSync(p).toString()` is a `String`'s, so `node:fs` is not
/// here). [`SAME_TYPE`] marks a method that returns its receiver's type
/// (`selection.attr(..).attr(..)`, `zoom().scaleExtent(..).on(..)`).
const JAVASCRIPT_PACKAGE_RETURNS: &[(&str, &str, &str)] = &[
    ("d3", "append", SAME_TYPE),
    ("d3", "attr", SAME_TYPE),
    ("d3", "call", SAME_TYPE),
    ("d3", "classed", SAME_TYPE),
    ("d3", "data", SAME_TYPE),
    ("d3", "delay", SAME_TYPE),
    ("d3", "drag", "d3.DragBehavior"),
    ("d3", "duration", SAME_TYPE),
    ("d3", "enter", SAME_TYPE),
    ("d3", "exit", SAME_TYPE),
    ("d3", "force", SAME_TYPE),
    ("d3", "forceSimulation", "d3.Simulation"),
    ("d3", "insert", SAME_TYPE),
    ("d3", "join", SAME_TYPE),
    ("d3", "on", SAME_TYPE),
    ("d3", "scaleExtent", SAME_TYPE),
    ("d3", "select", "d3.Selection"),
    ("d3", "selectAll", "d3.Selection"),
    ("d3", "style", SAME_TYPE),
    ("d3", "text", SAME_TYPE),
    ("d3", "transition", "d3.Transition"),
    ("d3", "zoom", "d3.ZoomBehavior"),
    (
        "node:child_process",
        "spawn",
        "node:child_process.ChildProcess",
    ),
    (
        "vscode",
        "createFileSystemWatcher",
        "vscode.FileSystemWatcher",
    ),
    ("vscode", "createOutputChannel", "vscode.OutputChannel"),
    ("vscode", "createStatusBarItem", "vscode.StatusBarItem"),
    ("vscode", "createTerminal", "vscode.Terminal"),
    ("vscode", "createTreeView", "vscode.TreeView"),
    ("vscode", "createWebviewPanel", "vscode.WebviewPanel"),
    (
        "vscode",
        "getConfiguration",
        "vscode.WorkspaceConfiguration",
    ),
];

/// A returned type that is the receiver's own.
const SAME_TYPE: &str = "";

/// What the package call `symbol` of `package` returns, by the table of
/// `family`. `symbol` is the call's `external_symbol` (`re.match`,
/// `d3.Selection.attr`) or the function's name; a [`SAME_TYPE`] method
/// returns the type before its name.
fn package_returned(family: &str, package: &str, stdlib: bool, symbol: &str) -> Option<Returned> {
    let table = match family {
        "python" => PYTHON_STDLIB_RETURNS,
        "javascript" => JAVASCRIPT_PACKAGE_RETURNS,
        _ => return None,
    };
    let (owner, function) = symbol.rsplit_once('.').unwrap_or(("", symbol));
    let (_, _, type_path) = table
        .iter()
        .find(|(declaring, name, _)| *declaring == package && *name == function)?;
    let type_path = if *type_path == SAME_TYPE {
        owner
            .strip_prefix(package)
            .is_some_and(|rest| rest.starts_with('.'))
            .then_some(owner)?
    } else {
        type_path
    };
    // A type of another module of the same library (`pathlib`'s `open` an
    // `io` object) is the standard library's when the call was.
    let type_package = type_path.split('.').next().unwrap_or(type_path);
    Some(Returned::External(
        type_package.to_string(),
        stdlib,
        type_path.to_string(),
    ))
}

/// Rust accessors whose result derefs to what the cell or lock holds
/// (`Ref<T>`, `MutexGuard<T>`), as the owner and method an
/// `external_symbol` spells; `borrow`, `borrow_mut`, and `lock` also
/// without one.
const RUST_GUARD_ACCESSORS: &[&str] = &[
    "Mutex::get_mut",
    "Mutex::lock",
    "Mutex::try_lock",
    "RefCell::borrow",
    "RefCell::borrow_mut",
    "RefCell::try_borrow",
    "RefCell::try_borrow_mut",
    "RwLock::read",
    "RwLock::try_read",
    "RwLock::try_write",
    "RwLock::write",
    "borrow",
    "borrow_mut",
    "lock",
];

/// Whether the value a Rust call chain gives (`external_symbol`
/// `RefCell::borrow`, `Mutex::lock()::unwrap`) is a guard of a cell or a
/// lock, unwrapped or not: a method on it is the contents'
/// (`bindings.borrow().snapshot()` is the bindings type's), not the
/// standard library's.
fn derefs_to_contents(symbol: &str) -> bool {
    let mut chain = symbol.trim_end_matches("()");
    while let Some(rest) = ["()::unwrap", "()::expect"]
        .iter()
        .find_map(|suffix| chain.strip_suffix(suffix))
    {
        chain = rest;
    }
    let last = chain.rsplit("()::").next().unwrap_or(chain);
    RUST_GUARD_ACCESSORS.iter().any(|accessor| {
        last == *accessor || (accessor.contains("::") && last.ends_with(&format!("::{accessor}")))
    })
}

/// What a returned type is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Returned {
    /// A class of the repository (its QN) and its name.
    Class(String, String),
    /// A type of a package: package, standard library, type as written.
    External(String, bool, String),
}

#[derive(Default)]
struct Graph {
    nodes: HashSet<String>,
    /// Function QN -> (return type as written, owner class name).
    returns: HashMap<String, (String, Option<String>)>,
    /// (language family, class name) -> class QNs.
    classes: HashMap<(&'static str, String), Vec<String>>,
    /// (language family, class name, method name) -> method QNs.
    methods: HashMap<(&'static str, String, String), Vec<String>>,
    /// File -> local name -> (package, standard library), from its imports
    /// of packages.
    external_names: HashMap<String, HashMap<String, (String, bool)>>,
    /// File -> local name -> class QN, from its imports of repository files.
    imported_classes: HashMap<String, HashMap<String, String>>,
    /// (language family, function name) -> files declaring one.
    defined: HashMap<(&'static str, String), HashSet<String>>,
    /// File -> module files it glob-imports (`use crate::*`).
    globs: HashMap<String, Vec<String>>,
    /// File -> the packages it imports (`net/http`, `java.util`).
    imported_packages: HashMap<String, Vec<(String, bool)>>,
}

impl Graph {
    /// Whether a function named `name` is declared in `file` or in a file
    /// `file` imports.
    fn visible(
        &self,
        family: &'static str,
        name: &str,
        file: &str,
        import_targets: &HashMap<String, HashSet<String>>,
    ) -> bool {
        self.defined
            .get(&(family, name.to_string()))
            .is_some_and(|files| {
                files.iter().any(|declaring| {
                    declaring == file
                        || import_targets
                            .get(file)
                            .is_some_and(|targets| targets.contains(declaring))
                })
            })
    }

    /// The package `name` comes from in `file`: its own imports, then
    /// those of the modules it glob-imports.
    fn external_name(&self, file: &str, name: &str) -> Option<&(String, bool)> {
        let mut pending = vec![file];
        let mut seen = HashSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current) {
                continue;
            }
            if let Some(found) = self
                .external_names
                .get(current)
                .and_then(|names| names.get(name))
            {
                return Some(found);
            }
            pending.extend(
                self.globs
                    .get(current)
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            );
        }
        None
    }
}

fn load_graph(tx: &Transaction<'_>) -> Result<Graph> {
    let mut graph = Graph::default();
    let mut stmt = tx.prepare(
        "SELECT qualified_name, kind, name, parent_name, file_path, return_type FROM nodes",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut class_names_by_qn: HashMap<String, String> = HashMap::new();
    for row in rows {
        let (qualified, kind, name, parent, file_path, return_type) = row?;
        let family = language_family(&file_path);
        match kind.as_str() {
            "Class" => {
                class_names_by_qn.insert(qualified.clone(), name.clone());
                if let Some(family) = family {
                    graph
                        .classes
                        .entry((family, name.clone()))
                        .or_default()
                        .push(qualified.clone());
                }
            }
            "Function" | "Test" => {
                if let Some(family) = family {
                    graph
                        .defined
                        .entry((family, name.clone()))
                        .or_default()
                        .insert(file_path.clone());
                }
                if let (Some(family), Some(owner)) = (family, parent.as_deref()) {
                    let owner = owner.rsplit('.').next().unwrap_or(owner).to_string();
                    graph
                        .methods
                        .entry((family, owner, name.clone()))
                        .or_default()
                        .push(qualified.clone());
                }
                if let Some(return_type) = return_type.filter(|text| !text.trim().is_empty()) {
                    let owner = parent
                        .as_deref()
                        .map(|owner| owner.rsplit('.').next().unwrap_or(owner).to_string());
                    graph
                        .returns
                        .insert(qualified.clone(), (return_type, owner));
                }
            }
            _ => {}
        }
        graph.nodes.insert(qualified);
    }
    let mut stmt = tx.prepare(
        "SELECT file_path, target_qualified, extra FROM edges WHERE kind = 'IMPORTS_FROM'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (file, target, extra) = row?;
        let extra: Value = serde_json::from_str(&extra).unwrap_or(Value::Null);
        if extra.get("glob").and_then(Value::as_bool) == Some(true) {
            graph
                .globs
                .entry(file.clone())
                .or_default()
                .push(target.clone());
        }
        let names: Vec<(String, String)> = extra
            .get("names")
            .and_then(|names| serde_json::from_value(names.clone()).ok())
            .unwrap_or_default();
        if extra.get("external").and_then(Value::as_bool) == Some(true) {
            let package = extra
                .get("external_package")
                .and_then(Value::as_str)
                .unwrap_or(&target)
                .to_string();
            let stdlib = extra.get("stdlib").and_then(Value::as_bool) == Some(true);
            graph
                .imported_packages
                .entry(file.clone())
                .or_default()
                .push((package.clone(), stdlib));
            let local = graph.external_names.entry(file).or_default();
            for (_, alias) in &names {
                local.insert(alias.clone(), (package.clone(), stdlib));
            }
            // `use rusqlite::Connection` (`paths`), `import sqlite3 as db`.
            for path in extra
                .get("paths")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if let Some(name) = path.rsplit("::").next() {
                    local.insert(name.to_string(), (package.clone(), stdlib));
                }
            }
            let module = extra
                .get("alias")
                .and_then(Value::as_str)
                .or_else(|| extra.get("module").and_then(Value::as_str))
                .map(|module| module.split('.').next().unwrap_or(module).to_string());
            if let Some(module) = module.filter(|_| names.is_empty()) {
                local.insert(module, (package.clone(), stdlib));
            }
            continue;
        }
        for (name, alias) in names {
            let class = format!("{target}::{name}");
            if class_names_by_qn.contains_key(&class) {
                graph
                    .imported_classes
                    .entry(file.clone())
                    .or_default()
                    .insert(alias, class);
            }
        }
    }
    Ok(graph)
}

/// The top-level generic arguments of `text` (`HashMap<K, Vec<V>>` gives
/// `K`, `Vec<V>`).
fn split_arguments(text: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut depth = 0_i32;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                arguments.push(std::mem::take(&mut current).trim().to_string());
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        arguments.push(current.trim().to_string());
    }
    arguments
}

/// Words of a declaration that are not the type (`const Repo&`, `final`).
const TYPE_MODIFIERS: &[&str] = &[
    "const",
    "final",
    "static",
    "mut",
    "volatile",
    "struct",
    "class",
    "enum",
    "unsigned",
    "signed",
    "readonly",
    "inline",
    "virtual",
    "public",
    "private",
    "protected",
    "override",
    "async",
    "__strong",
    "__weak",
    "nonnull",
    "nullable",
    "_Nonnull",
    "_Nullable",
];

/// Types that say nothing about a value's methods.
const OPAQUE_TYPES: &[&str] = &[
    "Any",
    "None",
    "object",
    "void",
    "Void",
    "Unit",
    "any",
    "unknown",
    "never",
    "dynamic",
    "id",
    "auto",
    "var",
    "interface{}",
    "Object",
    "Nothing",
    "noreturn",
    "null",
    "undefined",
    "mixed",
];

/// Return types naming the class that declares the method (`-> Self`,
/// PHP `: static` / `: self`, Objective-C `instancetype`, TypeScript
/// `this`).
const OWN_TYPES: &[&str] = &["Self", "self", "static", "instancetype", "this"];

/// The type a return type annotation names, as `(path, generic arguments)`,
/// in the syntax of any language: `&'a mut Store`, `*Store`, `const Repo&`,
/// `Repo*`, `(NSString *)`, `?Repo` and `Repo?` / `Repo!` give the type;
/// `Optional[T]` and `T | None` / `T | null` give `T`; Go's `(*Store,
/// error)` and Zig's `!Store` / `E!Store` give `Store`; `Result<Self, E>`
/// gives `Result` with `[Self, E]`. `impl Trait`, `dyn Trait`, and opaque
/// types (`Any`, `void`, `id`) name none.
fn parse_type(text: &str) -> Option<(String, Vec<String>)> {
    let mut text = text
        .trim()
        .trim_start_matches("->")
        .trim_start_matches(':')
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .trim();
    // `(NSString *)`, Go's `(*Store, error)`: the first element.
    if text.starts_with('(') && text.ends_with(')') {
        let inner = &text[1..text.len() - 1];
        let first = split_arguments(inner).into_iter().next()?;
        return parse_type(&first);
    }
    // Zig's error unions: `!Store`, `anyerror!Store`, `error{A}!Store`.
    if let Some((_, value)) = text.rsplit_once('!')
        && !text.ends_with('!')
    {
        text = value.trim();
    }
    loop {
        let before = text;
        text = text
            .trim_start_matches(['&', '*', '?', '^', '@'])
            .trim_end_matches(['&', '*', '?', '!'])
            .trim();
        if text.starts_with('\'')
            && let Some((_, rest)) = text.split_once(' ')
        {
            text = rest.trim_start();
        }
        for modifier in TYPE_MODIFIERS {
            if let Some(rest) = text.strip_prefix(modifier)
                && rest.starts_with([' ', '*', '&'])
            {
                text = rest.trim_start();
            }
            if let Some(rest) = text.strip_suffix(modifier)
                && rest.ends_with([' ', '*', '&'])
            {
                text = rest.trim_end();
            }
        }
        if before == text {
            break;
        }
    }
    if text.starts_with("impl ") || text.starts_with("dyn ") || text.is_empty() {
        return None;
    }
    // `T | None`, `T | null | undefined`
    if text.contains('|') {
        let parts = text
            .split('|')
            .map(str::trim)
            .filter(|part| !matches!(*part, "None" | "null" | "undefined"))
            .collect::<Vec<_>>();
        return match parts.as_slice() {
            [single] => parse_type(single),
            _ => None,
        };
    }
    // Go and Rust slices / arrays: `[]T`, `[T]`, `[]*T`.
    if let Some(element) = text.strip_prefix("[]") {
        return Some(("[]".to_string(), vec![element.to_string()]));
    }
    let (base, arguments) = match text.find(['<', '[']) {
        Some(index) if index > 0 => (
            text[..index].trim(),
            split_arguments(&text[index + 1..text.len().saturating_sub(1)]),
        ),
        _ => (text, Vec::new()),
    };
    let last = base.rsplit(['.', ':']).next().unwrap_or(base);
    match last {
        "Optional" if !arguments.is_empty() => parse_type(arguments.first()?),
        // A smart pointer is used through its pointee (`ptr->save()`).
        "unique_ptr" | "shared_ptr" | "weak_ptr" | "Ref" | "RefPtr" if !arguments.is_empty() => {
            parse_type(arguments.first()?)
        }
        _ if OPAQUE_TYPES.contains(&last) || OPAQUE_TYPES.contains(&base) => None,
        _ => Some((base.to_string(), arguments)),
    }
}

/// Wrappers a call's result is taken out of when it is unwrapped (Rust `?`
/// / `.unwrap()`, `await`, Swift `try` / `!`).
const UNWRAPPED_WRAPPERS: &[&str] = &[
    "Result",
    "Option",
    "Promise",
    "PromiseLike",
    "Task",
    "ValueTask",
    "Future",
    "FutureOr",
    "Deferred",
    "CompletableFuture",
    "Optional",
];

/// The package a built-in type of `family` belongs to (`dict` of Python,
/// `String` of Java, `Map` of JavaScript), for a type written without a
/// path; `file` tells Java, Kotlin, and Scala apart.
fn builtin_type_package(family: &str, file: &str, name: &str) -> Option<&'static str> {
    let is = |names: &[&str]| names.contains(&name);
    let package = match family {
        "python" if is(PYTHON_BUILTIN_TYPES) => "builtins",
        "rust" if is(RUST_STD_TYPES) => "std",
        "javascript"
            if is(&[
                "Array",
                "Map",
                "Set",
                "WeakMap",
                "WeakSet",
                "Promise",
                "String",
                "Number",
                "Boolean",
                "Date",
                "RegExp",
                "Error",
                "URL",
                "URLSearchParams",
                "Uint8Array",
                "ArrayBuffer",
                "Buffer",
                "ReadonlyArray",
                "Record",
                "string",
                "number",
                "boolean",
                "bigint",
                "symbol",
            ]) =>
        {
            "globalThis"
        }
        "jvm"
            if (file.ends_with(".kt") || file.ends_with(".kts"))
                && is(&[
                    "String",
                    "Int",
                    "Long",
                    "Double",
                    "Float",
                    "Boolean",
                    "Char",
                    "Byte",
                    "Short",
                    "List",
                    "MutableList",
                    "Map",
                    "MutableMap",
                    "Set",
                    "MutableSet",
                    "Array",
                    "Sequence",
                    "Pair",
                    "Triple",
                    "Collection",
                    "Iterable",
                    "Regex",
                    "Result",
                ]) =>
        {
            "kotlin"
        }
        "jvm" if file.ends_with(".kt") || file.ends_with(".kts") => return None,
        "jvm"
            if (file.ends_with(".scala") || file.ends_with(".sc"))
                && is(&[
                    "String", "Int", "Long", "Double", "Boolean", "List", "Seq", "Vector", "Map",
                    "Set", "Option", "Either", "Future", "Array", "Iterator",
                ]) =>
        {
            "scala"
        }
        "jvm" if file.ends_with(".scala") || file.ends_with(".sc") => return None,
        "jvm"
            if is(&[
                "String",
                "Integer",
                "Long",
                "Double",
                "Float",
                "Boolean",
                "Character",
                "Byte",
                "Short",
                "StringBuilder",
                "StringBuffer",
                "Thread",
                "Class",
                "Iterable",
                "Number",
                "Enum",
                "Record",
                "CharSequence",
                "Runnable",
                "Exception",
                "RuntimeException",
            ]) =>
        {
            "java.lang"
        }
        "jvm"
            if is(&[
                "List",
                "ArrayList",
                "LinkedList",
                "Map",
                "HashMap",
                "LinkedHashMap",
                "TreeMap",
                "Set",
                "HashSet",
                "LinkedHashSet",
                "TreeSet",
                "Optional",
                "Collection",
                "Iterator",
                "Deque",
                "ArrayDeque",
                "Queue",
                "Stream",
            ]) =>
        {
            "java.util"
        }
        "csharp"
            if is(&[
                "List",
                "Dictionary",
                "HashSet",
                "IEnumerable",
                "IList",
                "IDictionary",
                "ICollection",
                "Queue",
                "Stack",
                "SortedDictionary",
                "LinkedList",
            ]) =>
        {
            "System.Collections.Generic"
        }
        "csharp" if is(&["Task", "ValueTask"]) => "System.Threading.Tasks",
        "csharp" if is(&["StringBuilder"]) => "System.Text",
        "csharp"
            if is(&[
                "string",
                "String",
                "int",
                "long",
                "double",
                "bool",
                "object",
                "DateTime",
                "TimeSpan",
                "Guid",
                "Uri",
                "Exception",
                "Array",
                "Span",
            ]) =>
        {
            "System"
        }
        "go" if is(&[
            "string", "error", "int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16",
            "uint32", "uint64", "byte", "rune", "float32", "float64", "bool", "[]", "map",
        ]) =>
        {
            "builtin"
        }
        "swift"
            if is(&[
                "String",
                "Int",
                "Double",
                "Float",
                "Bool",
                "Array",
                "Dictionary",
                "Set",
                "Character",
                "Substring",
                "Result",
                "Optional",
            ]) =>
        {
            "Swift"
        }
        "swift" if is(&["Date", "URL", "Data", "UUID", "FileManager", "JSONDecoder"]) => {
            "Foundation"
        }
        "dart"
            if is(&[
                "String", "int", "double", "num", "bool", "List", "Map", "Set", "Iterable",
                "Future", "Stream", "DateTime", "Duration", "Uri", "RegExp",
            ]) =>
        {
            "dart:core"
        }
        "c" if name.starts_with("NS") => "Foundation",
        "c" if name.starts_with("UI") => "UIKit",
        "php"
            if is(&[
                "DateTime",
                "DateTimeImmutable",
                "DateInterval",
                "Exception",
                "ArrayObject",
                "ArrayIterator",
                "PDO",
                "PDOStatement",
                "SplStack",
                "SplQueue",
                "Closure",
                "Generator",
                "SplObjectStorage",
            ]) =>
        {
            "php"
        }
        "julia"
            if is(&[
                "String", "Int", "Int64", "Float64", "Vector", "Matrix", "Array", "Dict", "Set",
                "Tuple", "Symbol",
            ]) =>
        {
            "Base"
        }
        "gdscript"
            if is(&[
                "Node",
                "Node2D",
                "Node3D",
                "Control",
                "Vector2",
                "Vector3",
                "Vector2i",
                "Color",
                "Array",
                "Dictionary",
                "String",
                "Resource",
                "PackedScene",
                "Timer",
                "Tween",
                "Label",
                "Button",
                "Sprite2D",
                "Area2D",
                "CharacterBody2D",
                "Camera2D",
                "AnimationPlayer",
                "Texture2D",
                "Rect2",
                "Transform2D",
                "Object",
                "RefCounted",
            ]) =>
        {
            "godot"
        }
        _ => return None,
    };
    Some(package)
}

/// What `type_path` names in `file` (a function of `family` declared
/// there returns it, `owner` being its class).
fn resolve_type(
    graph: &Graph,
    type_path: &str,
    file: &str,
    family: &'static str,
    owner: Option<&str>,
) -> Option<Returned> {
    let segments = type_path
        .split("::")
        .flat_map(|part| part.split('.'))
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let root = *segments.first()?;
    let last = *segments.last()?;
    let name = if OWN_TYPES.contains(&type_path) {
        owner?
    } else {
        last
    };
    // A class of the repository: the file's own, one it imports, or the
    // only one of that name.
    let classes = graph
        .classes
        .get(&(family, name.to_string()))
        .map(Vec::as_slice)
        .unwrap_or_default();
    let class = classes
        .iter()
        .find(|class| class.starts_with(&format!("{file}::")))
        .or_else(|| {
            graph
                .imported_classes
                .get(file)
                .and_then(|imported| imported.get(name))
        })
        .or(match classes {
            [only] => Some(only),
            _ => None,
        });
    if let Some(class) = class {
        return Some(Returned::Class(class.clone(), name.to_string()));
    }
    // A package qualifier (`http.Client`, `json.Decoder`) is the last
    // element of a package the file imports (`net/http`).
    let qualified_package = || {
        (segments.len() > 1).then_some(())?;
        graph
            .imported_packages
            .get(file)?
            .iter()
            .find(|(package, _)| package.rsplit(['/', '.', ':']).next() == Some(root))
    };
    if let Some((package, stdlib)) = graph.external_name(file, root).or_else(qualified_package) {
        return Some(Returned::External(
            package.clone(),
            *stdlib,
            type_path.to_string(),
        ));
    }
    // A path through a standard crate or namespace (`std::path::PathBuf`,
    // `std::vector`).
    if segments.len() > 1
        && match family {
            "rust" => matches!(root, "std" | "core" | "alloc"),
            "c" => root == "std",
            _ => false,
        }
    {
        return Some(Returned::External(
            "std".to_string(),
            true,
            type_path.to_string(),
        ));
    }
    let package = builtin_type_package(family, file, if segments.len() == 1 { root } else { "" })?;
    Some(Returned::External(
        package.to_string(),
        true,
        type_path.to_string(),
    ))
}

/// What the function `function` returns, unwrapped from its `Result` /
/// `Option` / `Promise` / `Task` / `Future` when the call was (`?`,
/// `.unwrap()`, `await`, `try`).
fn returned_by(graph: &Graph, function: &str, unwrap: bool) -> Option<Returned> {
    let (return_type, owner) = graph.returns.get(function)?;
    let file = function.split_once("::").map(|(file, _)| file)?;
    let family = language_family(file)?;
    let (mut base, mut arguments) = parse_type(return_type)?;
    let last = |base: &str| base.rsplit(['.', ':']).next().unwrap_or(base).to_string();
    if unwrap && UNWRAPPED_WRAPPERS.contains(&last(&base).as_str()) {
        (base, arguments) = parse_type(arguments.first()?)?;
    }
    drop(arguments);
    resolve_type(graph, &base, file, family, owner.as_deref())
}

type PendingCall = (i64, String, String, String, Value);

fn pending_calls(tx: &Transaction<'_>) -> Result<Vec<PendingCall>> {
    let mut stmt = tx.prepare(
        "SELECT id, source_qualified, file_path, target_qualified, extra FROM edges \
         WHERE kind = 'CALLS' AND json_extract(extra, '$.receiver_from') IS NOT NULL \
           AND COALESCE(json_extract(extra, '$.external'), 0) = 0 \
           AND NOT EXISTS (SELECT 1 FROM nodes n \
                           WHERE n.qualified_name = edges.target_qualified)",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut calls = Vec::new();
    for row in rows {
        let (id, source, file, target, extra) = row?;
        calls.push((id, source, file, target, serde_json::from_str(&extra)?));
    }
    Ok(calls)
}

/// The call on `line` of `source` that names `name`: its target and
/// metadata.
/// Not the call itself: a chain repeating a method
/// (`.flag("a").flag("b")`) has both on one line. A resolved call of the
/// name comes before an unresolved one.
fn origin_call(
    tx: &Transaction<'_>,
    id: i64,
    source: &str,
    file: &str,
    line: i64,
    name: &str,
    nodes: &HashSet<String>,
) -> Result<Option<(String, Value)>> {
    let mut stmt = tx.prepare_cached(
        "SELECT id, target_qualified, extra FROM edges \
         WHERE kind = 'CALLS' AND source_qualified = ? AND file_path = ? AND line = ?",
    )?;
    let rows = stmt.query_map(params![source, file, line], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut unresolved = None;
    for row in rows {
        let (edge_id, target, extra) = row?;
        if edge_id == id {
            continue;
        }
        let extra: Value = serde_json::from_str(&extra)?;
        let leaf = |text: &str| {
            text.trim_end_matches('!')
                .rsplit(['.', ':'])
                .next()
                .unwrap_or(text)
                .to_string()
        };
        let named = leaf(&target) == name
            || extra
                .get("external_symbol")
                .and_then(Value::as_str)
                .is_some_and(|symbol| leaf(symbol) == name);
        if !named {
            continue;
        }
        let resolved =
            nodes.contains(&target) || extra.get("external").and_then(Value::as_bool) == Some(true);
        if resolved {
            return Ok(Some((target, extra)));
        }
        unresolved.get_or_insert((target, extra));
    }
    Ok(unresolved)
}

fn mark_external(extra: &mut Value, package: &str, stdlib: bool, symbol: String) {
    if let Some(map) = extra.as_object_mut() {
        map.remove("receiver_unknown");
    }
    extra["external"] = json!(true);
    extra["external_package"] = json!(package);
    extra["external_symbol"] = json!(symbol);
    if stdlib {
        extra["stdlib"] = json!(true);
    }
    extra["confidence"] = json!(INFERRED_CONFIDENCE);
    extra["confidence_tier"] = json!("MEDIUM");
}

fn update_call(tx: &Transaction<'_>, id: i64, target: &str, extra: &Value) -> Result<()> {
    tx.execute(
        "UPDATE edges SET target_qualified = ?, target_name = ?, extra = ?, \
         confidence = ?, confidence_tier = ? WHERE id = ?",
        params![
            target,
            edge_target_name(target),
            serde_json::to_string(extra)?,
            INFERRED_CONFIDENCE,
            ConfidenceTier::Medium.as_str(),
            id
        ],
    )?;
    Ok(())
}

/// Resolves member calls on the result of another call by the return type
/// of the function that call resolved to (see the module docs). Returns
/// how many changed.
pub(crate) fn resolve_returned_receivers(tx: &Transaction<'_>) -> Result<i64> {
    let graph = load_graph(tx)?;
    let import_targets = import_targets_tx(tx)?;
    let mut changed = 0_i64;
    for _ in 0..MAX_ROUNDS {
        let mut round = 0_i64;
        for (id, source, file, method, mut extra) in pending_calls(tx)? {
            let Some(family) = language_family(&file) else {
                continue;
            };
            let origin = &extra["receiver_from"];
            let (Some(name), Some(line)) = (
                origin.get("call").and_then(Value::as_str),
                origin.get("line").and_then(Value::as_i64),
            ) else {
                continue;
            };
            let unwrap = origin.get("unwrap").and_then(Value::as_bool) == Some(true);
            let Some((inner_target, inner_extra)) =
                origin_call(tx, id, &source, &file, line, name, &graph.nodes)?
            else {
                continue;
            };
            let separator = if family == "rust" { "::" } else { "." };
            // Typed by a table of package return types, not by a type the
            // code writes: observed-method inference does not learn from it.
            let mut by_table = false;
            let returned = if graph.nodes.contains(&inner_target) {
                returned_by(&graph, &inner_target, unwrap)
            } else if family == "rust"
                && inner_extra.get("external").and_then(Value::as_bool) == Some(true)
                && (!unwrap || !graph.visible(family, &method, &file, &import_targets))
                && !inner_extra
                    .get("external_symbol")
                    .and_then(Value::as_str)
                    .is_some_and(derefs_to_contents)
            {
                // A Rust method of what a package returned (`iter().map()`,
                // `tree.root_node().kind()`) is one of the package's types.
                // Unwrapped (`prepare(..)?.query_map(..)`), it is too, unless
                // the repository has a function of the name the file sees:
                // `items.get(0).unwrap().save()` is the item's `save`.
                let package = inner_extra
                    .get("external_package")
                    .and_then(Value::as_str)
                    .unwrap_or(&inner_target);
                let stdlib = inner_extra.get("stdlib").and_then(Value::as_bool) == Some(true);
                let symbol = inner_extra
                    .get("external_symbol")
                    .and_then(Value::as_str)
                    .unwrap_or(name);
                Some(Returned::External(
                    package.to_string(),
                    stdlib,
                    format!("{symbol}()"),
                ))
            } else if matches!(family, "python" | "javascript")
                && inner_extra.get("external").and_then(Value::as_bool) == Some(true)
                && !graph.visible(family, &method, &file, &import_targets)
            {
                // A package function whose return type is known
                // (`conn.execute(..).fetchall()` is a `sqlite3.Cursor`'s,
                // `vscode.window.createOutputChannel(..).appendLine(..)` a
                // `vscode.OutputChannel`'s). A JavaScript call names its
                // function in the target (`vscode::window.createOutputChannel`).
                let package = inner_extra
                    .get("external_package")
                    .and_then(Value::as_str)
                    .unwrap_or(&inner_target);
                let stdlib = inner_extra.get("stdlib").and_then(Value::as_bool) == Some(true);
                let symbol = inner_extra
                    .get("external_symbol")
                    .and_then(Value::as_str)
                    .or_else(|| inner_target.split_once("::").map(|(_, symbol)| symbol))
                    .unwrap_or(name);
                by_table = true;
                package_returned(family, package, stdlib, symbol)
            } else {
                None
            };
            match returned {
                Some(Returned::Class(class, class_name)) => {
                    let Some(targets) =
                        graph
                            .methods
                            .get(&(family, class_name.clone(), method.clone()))
                    else {
                        // No such method: leave the type for later passes
                        // (a `#[pyclass]` of the same name).
                        if extra.get("receiver_type").is_none() {
                            extra["receiver_type"] = json!(class_name);
                            tx.execute(
                                "UPDATE edges SET extra = ? WHERE id = ?",
                                params![serde_json::to_string(&extra)?, id],
                            )?;
                        }
                        continue;
                    };
                    let prefix = format!("{class}.");
                    let target = targets
                        .iter()
                        .find(|target| target.starts_with(&prefix))
                        .or(match targets.as_slice() {
                            [only] => Some(only),
                            _ => None,
                        });
                    let Some(target) = target else {
                        continue;
                    };
                    if let Some(map) = extra.as_object_mut() {
                        map.remove("receiver_unknown");
                    }
                    extra["receiver_type"] = json!(class_name);
                    update_call(tx, id, target, &extra)?;
                    round += 1;
                }
                Some(Returned::External(package, stdlib, type_name)) => {
                    mark_external(
                        &mut extra,
                        &package,
                        stdlib,
                        format!("{type_name}{separator}{method}"),
                    );
                    if by_table {
                        extra["inferred_from"] = json!("return_table");
                    }
                    update_call(tx, id, &package, &extra)?;
                    round += 1;
                }
                None => {}
            }
        }
        changed += round;
        if round == 0 {
            break;
        }
    }
    Ok(changed)
}

/// Binds Python calls on a class a Rust extension exports (`#[pyclass(name
/// = "GraphStore")]`) that no Python class of that name answers to the
/// `#[pymethods]` method of that Python name. Returns how many changed.
pub(crate) fn resolve_pyo3_methods(tx: &Transaction<'_>) -> Result<i64> {
    // Python class name -> Rust struct QN, then (struct QN, Python method
    // name) -> method QN.
    let mut classes: HashMap<String, Vec<String>> = HashMap::new();
    let mut methods: HashMap<(String, String), String> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT qualified_name, kind, parent_name, file_path, \
                    json_extract(extra, '$.ffi_export.kind'), \
                    json_extract(extra, '$.ffi_export.name') FROM nodes \
             WHERE json_extract(extra, '$.ffi_export.abi') = 'pyo3'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        for row in rows {
            let (qualified, kind, parent, file, export_kind, export_name) = row?;
            let Some(export_name) = export_name else {
                continue;
            };
            match (kind.as_str(), export_kind.as_deref(), parent) {
                ("Class", Some("class"), _) => {
                    classes.entry(export_name).or_default().push(qualified);
                }
                ("Function", Some("method"), Some(parent)) => {
                    methods.insert((format!("{file}::{parent}"), export_name), qualified);
                }
                _ => {}
            }
        }
    }
    if classes.is_empty() {
        return Ok(0);
    }
    let edges = {
        let mut stmt = tx.prepare(
            "SELECT id, target_qualified, json_extract(extra, '$.receiver_type'), extra \
             FROM edges \
             WHERE kind = 'CALLS' AND file_path LIKE '%.py' \
               AND json_extract(extra, '$.receiver_type') IS NOT NULL \
               AND COALESCE(json_extract(extra, '$.external'), 0) = 0 \
               AND NOT EXISTS (SELECT 1 FROM nodes n \
                               WHERE n.qualified_name = edges.target_qualified)",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut resolved = 0_i64;
    for (id, method, receiver_type, extra) in edges {
        let Some([class]) = classes.get(&receiver_type).map(Vec::as_slice) else {
            continue;
        };
        let Some(target) = methods.get(&(class.clone(), method.clone())) else {
            continue;
        };
        let mut extra: Value = serde_json::from_str(&extra)?;
        extra["ffi_abi"] = json!("pyo3");
        update_call(tx, id, target, &extra)?;
        resolved += 1;
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::parse_type;

    #[test]
    fn return_types_of_every_language_name_their_type() {
        let base = |text: &str| parse_type(text).map(|(base, _)| base);
        for (text, expected) in [
            ("sqlite3.Connection", Some("sqlite3.Connection")),
            ("Optional[Store]", Some("Store")),
            ("Store | None", Some("Store")),
            ("&'a mut Store", Some("Store")),
            ("Result<Self, Error>", Some("Result")),
            ("(*Store, error)", Some("Store")),
            ("*http.Client", Some("http.Client")),
            ("[]string", Some("[]")),
            ("Store?", Some("Store")),
            ("Store!", Some("Store")),
            ("?Store", Some("Store")),
            ("(NSString *)", Some("NSString")),
            ("const Repo&", Some("Repo")),
            ("Repo*", Some("Repo")),
            ("std::unique_ptr<Store>", Some("Store")),
            ("!Store", Some("Store")),
            ("error{Oops}!*Store", Some("Store")),
            ("Promise<User>", Some("Promise")),
            ("User | null", Some("User")),
            ("List<User>", Some("List")),
            ("void", None),
            ("Any", None),
            ("impl Iterator<Item = u8>", None),
            ("id", None),
        ] {
            assert_eq!(base(text).as_deref(), expected, "{text}");
        }
        assert_eq!(
            parse_type("Promise<User>").map(|(_, arguments)| arguments),
            Some(vec!["User".to_string()])
        );
    }
}
