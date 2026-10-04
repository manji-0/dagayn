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
/// Rust attributes that do not make an item reachable from outside.
const TRANSPARENT_ATTRIBUTES: &[&str] = &[
    "allow",
    "cfg",
    "cfg_attr",
    "cold",
    "deny",
    "deprecated",
    "doc",
    "expect",
    "inline",
    "must_use",
    "track_caller",
    "warn",
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

/// The names of the `#[...]` attributes right above `line` (1-based).
fn rust_attributes(lines: &[String], line: i64) -> Vec<String> {
    let mut names = Vec::new();
    let mut index = line - 2;
    while index >= 0 {
        let text = lines[index as usize].trim();
        if let Some(body) = text.strip_prefix("#[") {
            let end = body
                .find(|c: char| c == '(' || c == ']' || c == '=' || c.is_whitespace())
                .unwrap_or(body.len());
            names.push(body[..end].to_string());
        } else if !(text.starts_with("///") || text.starts_with("//")) {
            break;
        }
        index -= 1;
    }
    names
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
            let head = header.split('{').next().unwrap_or(header);
            return head.contains(" for ").then_some("trait_impl_method");
        }
        return None;
    }
    None
}

/// Why `node` must not be called dead, before looking at its name's uses.
/// `derived` holds the classes that inherit from or implement something.
fn structural_reason(
    node: &GraphNode,
    lines: &[String],
    derived: &HashSet<String>,
) -> Option<&'static str> {
    if lines.is_empty() {
        return Some("source_unavailable");
    }
    let extra = &node.extra;
    if extra.get("type_role").and_then(Value::as_str) == Some("module") {
        return Some("scope_container");
    }
    // A method of a class with a base or an interface may override or
    // implement one, which a caller of the base reaches.
    if node.kind == "Function"
        && node.parent_name.is_some()
        && let Some((class, _)) = node.qualified_name.rsplit_once('.')
        && derived.contains(class)
    {
        return Some("overrides_or_implements");
    }
    if extra.get("ffi_export").is_some_and(|v| !v.is_null()) {
        return Some("ffi_export");
    }
    if let Some(Value::Array(decorators)) = extra.get("decorators") {
        let registering = decorators.iter().any(|d| {
            let name = d.as_str().unwrap_or("");
            let name = name
                .split('(')
                .next()
                .unwrap_or(name)
                .trim_start_matches('@');
            !TRANSPARENT_DECORATORS.contains(&name)
        });
        if registering {
            return Some("registration_decorator");
        }
    }
    if node.language == "rust" {
        let attributes = rust_attributes(lines, node.line_start);
        if attributes.iter().any(|attr| {
            !TRANSPARENT_ATTRIBUTES.contains(&attr.as_str())
                && !attr.starts_with("clippy::")
                && !attr.starts_with("rustfmt::")
        }) {
            return Some("attribute_registration");
        }
        if node.parent_name.is_some()
            && let Some(reason) = rust_trait_context(lines, node.line_start)
        {
            return Some(reason);
        }
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
    let mut pending: Vec<(GraphNode, Value)> = Vec::new();
    for (node, record) in candidates {
        let empty = Vec::new();
        let lines = sources.get(&node.file_path).unwrap_or(&empty);
        let reason = if node.name.is_empty() || !node.name.chars().all(is_identifier_char) {
            Some("name_not_checkable")
        } else {
            structural_reason(&node, lines, &derived)
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
    for (node, mut record) in pending {
        let elsewhere = found
            .get(&node.name)
            .into_iter()
            .flatten()
            .any(|(file, line)| {
                *file != node.file_path || *line < node.line_start || *line > node.line_end
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
    use super::{rust_attributes, rust_trait_context};

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
    }

    #[test]
    fn attributes_above_an_item_are_read_past_doc_comments() {
        let src = lines("#[pyfunction]\n/// Docs.\n#[inline]\nfn exported() {}");
        assert_eq!(rust_attributes(&src, 4), ["inline", "pyfunction"]);
    }
}
