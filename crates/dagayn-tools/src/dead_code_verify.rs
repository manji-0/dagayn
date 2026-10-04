//! The last check before a symbol is called dead: never claim one the
//! repository may still use.
//!
//! The graph's view (no callers, importers, references, tests, subclasses)
//! misses callbacks, type annotations, re-exports, string registries,
//! decorator registration, FFI exports, and trait dispatch. A candidate is
//! kept only when none of those can apply and its name appears nowhere in the
//! repository's text outside its own definition; every other candidate is
//! left out and counted by reason.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use dagayn_graph::{GraphNode, GraphStore};
use serde_json::{Value, json};

/// Larger files are not read; the verification then says it was partial.
const MAX_SCAN_BYTES: u64 = 4 * 1024 * 1024;
/// Directories a walk outside git skips: dependencies and build output.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".dagayn",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
    ".mypy_cache",
];
/// Decorators that wrap or describe a function without registering it.
const TRANSPARENT_DECORATORS: &[&str] = &[
    "abstractmethod",
    "abc.abstractmethod",
    "cache",
    "cached_property",
    "classmethod",
    "contextmanager",
    "asynccontextmanager",
    "contextlib.contextmanager",
    "contextlib.asynccontextmanager",
    "dataclass",
    "dataclasses.dataclass",
    "deprecated",
    "final",
    "functools.cache",
    "functools.cached_property",
    "functools.lru_cache",
    "functools.total_ordering",
    "functools.wraps",
    "lru_cache",
    "overload",
    "override",
    "property",
    "staticmethod",
    "total_ordering",
    "typing.final",
    "typing.overload",
    "typing.override",
    "typing_extensions.override",
    "wraps",
];
fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// How the scan went.
#[derive(Default)]
pub(crate) struct Verification {
    pub status: &'static str,
    pub files_scanned: usize,
    pub files_skipped: usize,
}

impl Verification {
    pub(crate) fn value(&self) -> Value {
        json!({
            "status": self.status,
            "files_scanned": self.files_scanned,
            "files_skipped": self.files_skipped,
        })
    }
}

/// The repository's files: tracked and untracked-but-not-ignored under git,
/// a walk otherwise. Sorted, relative to `root`.
fn repository_files(root: &Path) -> Option<Vec<String>> {
    let listed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .ok()
        .filter(|out| out.status.success());
    let mut files: Vec<String> = match listed {
        Some(out) => String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect(),
        None => {
            let mut found = Vec::new();
            let mut stack = vec![PathBuf::new()];
            while let Some(dir) = stack.pop() {
                let entries = std::fs::read_dir(root.join(&dir)).ok()?;
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let rel = dir.join(&name);
                    let kind = entry.file_type().ok()?;
                    if kind.is_dir() {
                        if !SKIPPED_DIRS.contains(&name.as_str()) {
                            stack.push(rel);
                        }
                    } else if kind.is_file() {
                        found.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
            found
        }
    };
    files.sort();
    files.dedup();
    Some(files)
}

/// Where each of `names` appears as a whole identifier: `(file, line)`.
fn occurrences(
    root: &Path,
    names: &HashSet<&str>,
    verification: &mut Verification,
) -> Option<HashMap<String, Vec<(String, i64)>>> {
    let mut found: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    for rel in repository_files(root)? {
        let path = root.join(&rel);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue; // listed but gone: nothing to read
        };
        if meta.len() > MAX_SCAN_BYTES {
            verification.files_skipped += 1;
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            verification.files_skipped += 1;
            continue;
        };
        if bytes.contains(&0) {
            continue; // binary
        }
        verification.files_scanned += 1;
        let text = String::from_utf8_lossy(&bytes);
        for (index, line) in text.lines().enumerate() {
            for token in line.split(|c: char| !is_identifier_char(c)) {
                if !token.is_empty() && names.contains(token) {
                    found
                        .entry(token.to_string())
                        .or_default()
                        .push((rel.clone(), index as i64 + 1));
                }
            }
        }
    }
    Some(found)
}

/// Annotation and attribute names that describe an item without making it
/// reachable (compared lowercased, last path segment).
const TRANSPARENT_ANNOTATIONS: &[&str] = &[
    "allow",
    "available",
    "cfg",
    "cfg_attr",
    "checkreturnvalue",
    "cold",
    "debuggerhidden",
    "debuggerstepthrough",
    "deny",
    "deprecated",
    "discardableresult",
    "doc",
    "expect",
    "functionalinterface",
    "inlinable",
    "inline",
    "mainactor",
    "methodimpl",
    "moduledoc",
    "must_use",
    "nonnull",
    "notnull",
    "nullable",
    "obsolete",
    "pure",
    "returntypewillchange",
    "safevarargs",
    "sendable",
    "sensitiveparameter",
    "spec",
    "suppresswarnings",
    "track_caller",
    "typedoc",
    "usablefrominline",
    "visiblefortesting",
    "warn",
];
/// How far above a definition its annotations are looked for.
const MAX_HEADER_LINES: usize = 12;
/// Annotations that mark an override or a callback implementation.
const OVERRIDE_ANNOTATIONS: &[&str] = &["override", "impl"];
/// Languages whose definitions can carry an `override` (or `operator`)
/// keyword on their own line.
const OVERRIDE_KEYWORD_LANGUAGES: &[&str] = &[
    "kotlin",
    "swift",
    "scala",
    "csharp",
    "typescript",
    "tsx",
    "javascript",
    "cpp",
    "dart",
    "vue",
    "svelte",
];

/// Methods a language or its runtime calls by protocol, never by name in
/// the code that relies on them.
fn implicit_method(language: &str, name: &str, is_method: bool) -> bool {
    let listed: &[&str] = match language {
        "java" => &[
            "toString",
            "equals",
            "hashCode",
            "compareTo",
            "clone",
            "finalize",
            "close",
            "run",
            "call",
            "iterator",
            "readObject",
            "writeObject",
            "readResolve",
            "writeReplace",
            "readObjectNoData",
        ],
        "kotlin" => &[
            "toString",
            "equals",
            "hashCode",
            "compareTo",
            "close",
            "invoke",
            "iterator",
            "next",
            "hasNext",
            "getValue",
            "setValue",
            "provideDelegate",
            "contains",
            "get",
            "set",
            "plus",
            "minus",
            "times",
            "div",
            "rem",
            "unaryMinus",
            "unaryPlus",
            "not",
            "inc",
            "dec",
            "rangeTo",
        ],
        "scala" => &[
            "apply",
            "unapply",
            "unapplySeq",
            "update",
            "toString",
            "equals",
            "hashCode",
            "map",
            "flatMap",
            "withFilter",
            "foreach",
            "filter",
        ],
        "csharp" => &[
            "ToString",
            "Equals",
            "GetHashCode",
            "Dispose",
            "DisposeAsync",
            "Finalize",
            "GetEnumerator",
            "CompareTo",
            "Deconstruct",
            "Main",
        ],
        "swift" => &[
            "description",
            "debugDescription",
            "hash",
            "encode",
            "init",
            "deinit",
            "makeIterator",
            "next",
            "callAsFunction",
        ],
        "go" => {
            if !is_method {
                return name == "init";
            }
            &[
                "String",
                "Error",
                "GoString",
                "Format",
                "ServeHTTP",
                "MarshalJSON",
                "UnmarshalJSON",
                "MarshalText",
                "UnmarshalText",
                "MarshalYAML",
                "UnmarshalYAML",
                "Read",
                "Write",
                "Close",
                "Len",
                "Less",
                "Swap",
                "Scan",
                "Value",
                "Unwrap",
                "Is",
                "As",
                "Lock",
                "Unlock",
                "Seek",
                "ReadFrom",
                "WriteTo",
            ]
        }
        "ruby" => &[
            "initialize",
            "to_s",
            "to_str",
            "to_a",
            "to_ary",
            "to_h",
            "to_hash",
            "to_proc",
            "to_i",
            "to_int",
            "to_f",
            "inspect",
            "each",
            "call",
            "hash",
            "eql?",
            "method_missing",
            "respond_to_missing?",
            "coerce",
            "included",
            "extended",
            "inherited",
            "prepended",
            "initialize_copy",
        ],
        "javascript" | "typescript" | "tsx" | "vue" | "svelte" => &[
            "toString",
            "valueOf",
            "toJSON",
            "then",
            "handleEvent",
            "connectedCallback",
            "disconnectedCallback",
            "attributeChangedCallback",
            "adoptedCallback",
        ],
        "php" => {
            return name.starts_with("__")
                || [
                    "jsonSerialize",
                    "offsetGet",
                    "offsetSet",
                    "offsetExists",
                    "offsetUnset",
                    "getIterator",
                    "count",
                    "current",
                    "key",
                    "next",
                    "rewind",
                    "valid",
                    "serialize",
                    "unserialize",
                ]
                .contains(&name);
        }
        "lua" => return name.starts_with("__"),
        "gdscript" => return name.starts_with('_'),
        "dart" => &["toString", "hashCode", "noSuchMethod", "call"],
        "elixir" => &[
            "init",
            "handle_call",
            "handle_cast",
            "handle_info",
            "handle_continue",
            "terminate",
            "code_change",
            "child_spec",
            "start_link",
            "format_status",
        ],
        "perl" => &[
            "DESTROY",
            "AUTOLOAD",
            "import",
            "unimport",
            "BUILD",
            "BUILDARGS",
            "DEMOLISH",
        ],
        "objc" => &["init", "dealloc", "description"],
        _ => &[],
    };
    listed.contains(&name)
}

/// The annotation, attribute, and decorator names in the header of the item
/// at `line` (1-based): the definition line's own prefix, and the lines above
/// it back to the previous statement, a blank line, or the file's start.
fn header_annotations(lines: &[String], line: i64, language: &str) -> Vec<String> {
    let Ok(at) = usize::try_from(line - 1) else {
        return Vec::new();
    };
    if at >= lines.len() {
        return Vec::new();
    }
    let mut header: Vec<&str> = Vec::new();
    for text in lines[..at].iter().rev().take(MAX_HEADER_LINES) {
        let trimmed = text.trim();
        if trimmed.is_empty()
            || trimmed.ends_with(';')
            || trimmed.ends_with('{')
            || trimmed.ends_with('}')
            || trimmed == "end"
            || trimmed.ends_with(" end")
        {
            break;
        }
        header.push(trimmed);
    }
    // The definition line up to its parameters: `@objc func f(`,
    // `[HttpGet] public IActionResult Get(`.
    let own = lines[at].trim();
    header.push(own.split('(').next().unwrap_or(own));
    let mut names = Vec::new();
    for text in header {
        // Comments name tags (`@param`, `@typedef`), not annotations.
        let comment = text.starts_with("//")
            || text.starts_with("/*")
            || text.starts_with('*')
            || text.starts_with("--")
            || (text.starts_with('#') && !text.starts_with("#[") && !text.starts_with("#!["));
        if comment {
            continue;
        }
        if language == "csharp" && text.starts_with('[') {
            let body = text.trim_start_matches('[');
            let body = body.split(']').next().unwrap_or(body);
            for part in body.split(',') {
                let ident: String = part
                    .trim()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
                    .collect();
                if !ident.is_empty() {
                    names.push(ident);
                }
            }
            continue;
        }
        if let Some(body) = text.strip_prefix("#[").or_else(|| text.strip_prefix("#![")) {
            let body = body.trim_start_matches('\\');
            let ident: String = body
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':' || *c == '\\')
                .collect();
            if !ident.is_empty() {
                names.push(ident);
            }
            continue;
        }
        // `@Name`, `@pkg.Name(...)`, several on one line.
        let mut rest = text;
        while let Some(at) = rest.find('@') {
            let after = &rest[at + 1..];
            let ident: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            if !ident.is_empty() {
                names.push(ident);
            }
            rest = after;
        }
    }
    names
}

/// The lowercased last segment of an annotation name.
fn annotation_key(name: &str) -> String {
    name.rsplit(['.', ':', '\\'])
        .next()
        .unwrap_or(name)
        .to_lowercase()
}

/// The definition's own text up to its body: the line and, for a signature
/// broken over lines, the next two until one opens the body.
fn definition_text(lines: &[String], line: i64) -> String {
    let Ok(at) = usize::try_from(line - 1) else {
        return String::new();
    };
    let mut text = String::new();
    for part in lines.iter().skip(at).take(3) {
        text.push_str(part);
        text.push(' ');
        if part.contains('{') || part.trim_end().ends_with(':') || part.contains("=>") {
            break;
        }
    }
    text
}

fn has_token(text: &str, token: &str) -> bool {
    text.split(|c: char| !is_identifier_char(c))
        .any(|word| word == token)
}

/// `Some("trait_impl_method")` for a method in `impl Trait for Type`, and
/// `Some("trait_method")` for one declared in a `trait`; found by the nearest
/// less-indented `impl`/`trait` header above it.
fn rust_trait_context(lines: &[String], line: i64) -> Option<&'static str> {
    let own = lines.get(usize::try_from(line - 1).ok()?)?;
    let indent = own.len() - own.trim_start().len();
    for text in lines[..usize::try_from(line - 1).ok()?].iter().rev() {
        let trimmed = text.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let depth = text.len() - trimmed.len();
        if depth >= indent {
            continue;
        }
        let header = trimmed
            .trim_start_matches("pub(crate) ")
            .trim_start_matches("pub ")
            .trim_start_matches("unsafe ");
        if header.starts_with("trait ") {
            return Some("trait_method");
        }
        if header.starts_with("impl") {
            // `#[pymethods] impl`: every method is exported to Python.
            let at = lines
                .iter()
                .position(|l| std::ptr::eq(l, text))
                .unwrap_or(0);
            let exported = lines[at.saturating_sub(3)..at]
                .iter()
                .any(|l| l.trim_start().starts_with("#[pymethods"));
            if exported {
                return Some("ffi_export");
            }
            let head = header.split('{').next().unwrap_or(header);
            return head.contains(" for ").then_some("trait_impl_method");
        }
        return None;
    }
    None
}

/// The enclosing class of a method, as far as the graph knows it.
pub(crate) struct ClassFacts {
    /// `type_role`: `interface`, `trait`, `protocol`, `abstract_class`, ...
    role: Option<String>,
    /// Registered by a framework annotation or decorator.
    registered: bool,
}

/// Whether `annotations` (and decorators) include one that registers.
fn registering(names: &[String]) -> bool {
    names.iter().any(|name| {
        let key = annotation_key(name);
        !TRANSPARENT_ANNOTATIONS.contains(&key.as_str())
            && !OVERRIDE_ANNOTATIONS.contains(&key.as_str())
            && !key.starts_with("clippy")
            && !key.starts_with("rustfmt")
            && !TRANSPARENT_DECORATORS.contains(&name.as_str())
    })
}

fn decorator_names(node: &GraphNode) -> Vec<String> {
    match node.extra.get("decorators") {
        Some(Value::Array(decorators)) => decorators
            .iter()
            .filter_map(Value::as_str)
            .map(|name| {
                name.split('(')
                    .next()
                    .unwrap_or(name)
                    .trim_start_matches('@')
                    .to_string()
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// [`ClassFacts`] for a class node, from the graph and its source.
pub(crate) fn class_facts(class: &GraphNode, lines: &[String]) -> ClassFacts {
    let decorators = decorator_names(class);
    let python_registered = decorators
        .iter()
        .any(|name| !TRANSPARENT_DECORATORS.contains(&name.as_str()));
    let annotated = class.language != "python"
        && registering(&header_annotations(
            lines,
            class.line_start,
            &class.language,
        ));
    ClassFacts {
        role: class
            .extra
            .get("type_role")
            .and_then(Value::as_str)
            .map(str::to_string),
        registered: python_registered || annotated,
    }
}

/// Why `node` must not be called dead, before looking at its name's uses.
/// `derived` holds the classes that inherit from or implement something;
/// `classes` what is known of each candidate method's class.
fn structural_reason(
    node: &GraphNode,
    lines: &[String],
    derived: &HashSet<String>,
    classes: &HashMap<String, ClassFacts>,
) -> Option<&'static str> {
    if lines.is_empty() {
        return Some("source_unavailable");
    }
    let extra = &node.extra;
    if extra.get("type_role").and_then(Value::as_str) == Some("module") {
        return Some("scope_container");
    }
    let class_qn = node
        .parent_name
        .as_ref()
        .and_then(|_| node.qualified_name.rsplit_once('.'))
        .map(|(class, _)| class);
    let is_method = node.kind == "Function" && class_qn.is_some();
    // A method of a class with a base or an interface may override or
    // implement one, which a caller of the base reaches.
    if is_method && class_qn.is_some_and(|class| derived.contains(class)) {
        return Some("overrides_or_implements");
    }
    if extra.get("ffi_export").is_some_and(|v| !v.is_null()) {
        return Some("ffi_export");
    }
    let decorators = decorator_names(node);
    if decorators
        .iter()
        .any(|name| !TRANSPARENT_DECORATORS.contains(&name.as_str()))
    {
        return Some("registration_decorator");
    }
    let definition = definition_text(lines, node.line_start);
    if node.language != "python" {
        let annotations = header_annotations(lines, node.line_start, &node.language);
        if annotations
            .iter()
            .any(|name| OVERRIDE_ANNOTATIONS.contains(&annotation_key(name).as_str()))
        {
            return Some("overrides_or_implements");
        }
        if registering(&annotations) {
            return Some("attribute_registration");
        }
    }
    if OVERRIDE_KEYWORD_LANGUAGES.contains(&node.language.as_str()) {
        if has_token(&definition, "override") {
            return Some("overrides_or_implements");
        }
        if has_token(&definition, "operator") {
            return Some("implicit_protocol_method");
        }
    }
    if node.kind == "Function" && implicit_method(&node.language, &node.name, is_method) {
        return Some("implicit_protocol_method");
    }
    if node.language == "kotlin" && node.name.starts_with("component") {
        return Some("implicit_protocol_method");
    }
    if let Some(class) = class_qn.and_then(|class| classes.get(class)) {
        match class.role.as_deref() {
            Some("interface" | "trait" | "protocol" | "abstract_type" | "mixin") => {
                return Some("contract_method");
            }
            Some("abstract_class") if has_token(&definition, "abstract") => {
                return Some("contract_method");
            }
            _ => {}
        }
        if is_method && class.registered {
            return Some("framework_class_method");
        }
    }
    if node.language == "rust"
        && node.parent_name.is_some()
        && let Some(reason) = rust_trait_context(lines, node.line_start)
    {
        return Some(reason);
    }
    None
}

/// What [`verify`] kept and left out.
pub(crate) struct Verified {
    pub dead: Vec<Value>,
    pub suppressed: BTreeMap<&'static str, usize>,
    pub verification: Verification,
}

/// Keep the candidates nothing in the repository may still use; `sources`
/// holds each candidate file's lines.
pub(crate) fn verify(
    store: &GraphStore,
    candidates: Vec<(GraphNode, Value)>,
    sources: &HashMap<String, Vec<String>>,
) -> Option<Verified> {
    let mut out = Verified {
        dead: Vec::new(),
        suppressed: BTreeMap::new(),
        verification: Verification {
            status: "complete",
            ..Verification::default()
        },
    };
    let mut derived: HashSet<String> = HashSet::new();
    for kind in ["INHERITS", "IMPLEMENTS"] {
        derived.extend(
            store
                .get_edges_by_kind(kind, false)
                .ok()?
                .into_iter()
                .map(|edge| edge.source_qualified),
        );
    }
    let class_names: Vec<String> = candidates
        .iter()
        .filter(|(node, _)| node.parent_name.is_some())
        .filter_map(|(node, _)| {
            node.qualified_name
                .rsplit_once('.')
                .map(|(c, _)| c.to_string())
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut class_sources: HashMap<String, Vec<String>> = HashMap::new();
    let mut classes: HashMap<String, ClassFacts> = HashMap::new();
    for (qn, class) in store.get_nodes_by_qualified_names(&class_names).ok()? {
        let lines = match sources.get(&class.file_path) {
            Some(lines) => lines,
            None => class_sources
                .entry(class.file_path.clone())
                .or_insert_with(|| crate::dead_code::source_lines(store, &class.file_path)),
        };
        classes.insert(qn, class_facts(&class, lines));
    }
    let mut pending: Vec<(GraphNode, Value)> = Vec::new();
    for (node, record) in candidates {
        let empty = Vec::new();
        let lines = sources.get(&node.file_path).unwrap_or(&empty);
        let reason = if node.name.is_empty() || !node.name.chars().all(is_identifier_char) {
            Some("name_not_checkable")
        } else {
            structural_reason(&node, lines, &derived, &classes)
        };
        match reason {
            Some(reason) => *out.suppressed.entry(reason).or_default() += 1,
            None => pending.push((node, record)),
        }
    }
    if pending.is_empty() {
        return Some(out);
    }
    let root = store
        .get_metadata("repo_root")
        .ok()
        .flatten()
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .filter(|root| root.is_dir());
    let names: HashSet<&str> = pending.iter().map(|(node, _)| node.name.as_str()).collect();
    let found = root
        .as_deref()
        .and_then(|root| occurrences(root, &names, &mut out.verification));
    let Some(found) = found else {
        // Nothing could be checked, so nothing is claimed.
        out.verification.status = "unavailable";
        *out.suppressed.entry("source_scan_unavailable").or_default() += pending.len();
        return Some(out);
    };
    if out.verification.files_skipped > 0 {
        out.verification.status = "partial";
    }
    let root_prefix = root
        .as_deref()
        .map(|root| format!("{}/", root.to_string_lossy().trim_end_matches('/')));
    for (node, mut record) in pending {
        // A graph built with absolute paths names files by them.
        let own_file = root_prefix
            .as_deref()
            .and_then(|prefix| node.file_path.strip_prefix(prefix))
            .unwrap_or(&node.file_path);
        let elsewhere = found
            .get(&node.name)
            .into_iter()
            .flatten()
            .any(|(file, line)| {
                file != own_file || *line < node.line_start || *line > node.line_end
            });
        if elsewhere {
            *out.suppressed
                .entry("name_referenced_in_source")
                .or_default() += 1;
            continue;
        }
        if let Some(Value::Array(reasons)) = record.get_mut("reason_codes") {
            reasons.push(json!("name_unreferenced_in_source"));
        }
        out.dead.push(record);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{header_annotations, implicit_method, rust_trait_context};

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn trait_impls_and_declarations_are_recognised() {
        let src = lines(
            "impl fmt::Display for Path {\n    fn fmt(&self) {}\n}\nimpl Path {\n    fn own(&self) {}\n}\npub trait Visit {\n    fn visit(&self) {}\n}",
        );
        assert_eq!(rust_trait_context(&src, 2), Some("trait_impl_method"));
        assert_eq!(rust_trait_context(&src, 5), None);
        assert_eq!(rust_trait_context(&src, 8), Some("trait_method"));
        let py = lines("#[pymethods]\nimpl Store {\n    fn close(&self) {}\n}");
        assert_eq!(rust_trait_context(&py, 3), Some("ffi_export"));
    }

    #[test]
    fn annotations_are_read_from_the_header_and_the_definition_line() {
        let src = lines("#[pyfunction]\n/// Docs.\n#[inline]\nfn exported() {}");
        assert_eq!(
            header_annotations(&src, 4, "rust"),
            ["inline", "pyfunction"]
        );
        let java = lines(
            "  }\n  @GetMapping(\n      value = \"/x\")\n  @Deprecated public String get() {",
        );
        assert_eq!(
            header_annotations(&java, 4, "java"),
            ["GetMapping", "Deprecated"]
        );
        let cs = lines("}\n[HttpGet, Authorize(Roles = \"a\")]\npublic IActionResult Get() {");
        assert_eq!(
            header_annotations(&cs, 3, "csharp"),
            ["HttpGet", "Authorize"]
        );
        let jsdoc = lines("/** @typedef {{a: number}} Foo */\nexport class JsClass {");
        assert!(header_annotations(&jsdoc, 2, "javascript").is_empty());
        let swift = lines("@objc func tapped(_ sender: Any) {");
        assert_eq!(header_annotations(&swift, 1, "swift"), ["objc"]);
    }

    #[test]
    fn protocol_methods_are_known_per_language() {
        assert!(implicit_method("ruby", "to_s", true));
        assert!(implicit_method("go", "init", false));
        assert!(!implicit_method("go", "String", false));
        assert!(implicit_method("go", "String", true));
        assert!(implicit_method("php", "__construct", true));
        assert!(implicit_method("gdscript", "_ready", true));
        assert!(!implicit_method("python", "to_s", true));
    }
}
