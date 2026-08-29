use std::path::Path;

use sha2::{Digest, Sha256};

use super::types::{ParsedEdge, ParsedNode};

pub(super) fn ends_with_ascii_ignore_case(value: &str, suffix: &str) -> bool {
    let bytes = value.as_bytes();
    let suffix = suffix.as_bytes();
    bytes
        .get(bytes.len().saturating_sub(suffix.len())..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
}

pub(super) fn starts_with_ascii_ignore_case(value: &str, prefix: &str) -> bool {
    value
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

pub(super) fn contains_ascii_ignore_case(value: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    !needle.is_empty()
        && value
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

pub(super) fn sha256_hex(source: &[u8]) -> String {
    let digest = Sha256::digest(source);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub(super) fn node_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let text = node_text_bytes(node, source);
    match std::str::from_utf8(text) {
        Ok(text) => text.to_owned(),
        Err(_) => String::from_utf8_lossy(text).into_owned(),
    }
}

pub(super) fn node_text_bytes<'source>(
    node: tree_sitter::Node<'_>,
    source: &'source [u8],
) -> &'source [u8] {
    &source[node.start_byte()..node.end_byte()]
}

pub(super) fn node_text_is(node: tree_sitter::Node<'_>, source: &[u8], expected: &str) -> bool {
    node_text_bytes(node, source) == expected.as_bytes()
}

pub(super) fn node_text_tf_string_is(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    expected: &str,
) -> bool {
    let text = trim_ascii_bytes(node_text_bytes(node, source));
    let unquoted = match text {
        [b'"', inner @ .., b'"'] | [b'\'', inner @ .., b'\''] => inner,
        _ => text,
    };
    unquoted == expected.as_bytes()
}

pub(super) fn trim_ascii_bytes(mut value: &[u8]) -> &[u8] {
    while let Some((first, rest)) = value.split_first() {
        if !first.is_ascii_whitespace() {
            break;
        }
        value = rest;
    }
    while let Some((last, rest)) = value.split_last() {
        if !last.is_ascii_whitespace() {
            break;
        }
        value = rest;
    }
    value
}

pub(super) fn line_count(source: &[u8]) -> i64 {
    memchr::memchr_iter(b'\n', source).count() as i64 + 1
}

pub(super) fn line_for_offset(text: &str, offset: usize) -> i64 {
    text.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as i64
        + 1
}

pub(super) fn normalize_relative_path(path: &Path) -> String {
    let mut parts = Vec::<String>::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(part) => {
                parts.push(part.to_string_lossy().to_string());
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                parts.push(component.as_os_str().to_string_lossy().to_string());
            }
        }
    }
    parts.join("/")
}

pub(super) fn dedupe_edges(edges: Vec<ParsedEdge>) -> Vec<ParsedEdge> {
    let mut seen = std::collections::HashSet::new();
    edges
        .into_iter()
        .filter(|edge| {
            seen.insert((
                edge.kind,
                edge.source.clone(),
                edge.target.clone(),
                edge.line,
            ))
        })
        .collect()
}

pub(super) fn is_test_file(file_path: &str) -> bool {
    contains_ascii_ignore_case(file_path, "/test/")
        || contains_ascii_ignore_case(file_path, "/tests/")
        || starts_with_ascii_ignore_case(file_path, "test/")
        || starts_with_ascii_ignore_case(file_path, "tests/")
        || starts_with_ascii_ignore_case(file_path, "test_")
        || ends_with_ascii_ignore_case(file_path, "_test.md")
        || ends_with_ascii_ignore_case(file_path, ".test.md")
        || ends_with_ascii_ignore_case(file_path, "_test.py")
        || ends_with_ascii_ignore_case(file_path, ".test.py")
        || ends_with_ascii_ignore_case(file_path, ".spec.py")
}

/// Records the namespaces (or packages) a file declares on its `File` node.
///
/// Bare-name call resolution needs a namespace index. Languages whose imports
/// name a namespace rather than a file (C# `using`, PHP `use`, Kotlin/Scala
/// `import`) never produce file-to-file IMPORTS_FROM edges, and files in the
/// same namespace need no import statement at all — so without this the
/// resolver has no evidence that two files can see each other.
pub(super) fn set_declared_namespaces(nodes: &mut [ParsedNode], namespaces: Vec<String>) {
    let mut unique = Vec::<String>::new();
    for namespace in namespaces {
        let namespace: String = namespace.split_whitespace().collect();
        if namespace.is_empty() || unique.contains(&namespace) {
            continue;
        }
        unique.push(namespace);
    }
    if unique.is_empty() {
        return;
    }
    if let Some(file) = nodes.first_mut() {
        if let Some(map) = file.extra.as_object_mut() {
            map.insert("namespaces".to_string(), serde_json::json!(unique));
        }
    }
}

/// Records the type names a file defines as its declared namespaces.
///
/// Elixir `defmodule` and Julia `module` names are namespaces rather than
/// classes: they are exactly what another file's `alias` or `using` refers to.
pub(super) fn set_namespaces_from_type_names(nodes: &mut [ParsedNode]) {
    let names: Vec<String> = nodes
        .iter()
        .filter(|node| node.kind == crate::core::types::NodeKind::Class)
        .map(|node| node.name.clone())
        .collect();
    set_declared_namespaces(nodes, names);
}

/// Resolves an include/import path to a repo-relative file.
///
/// The literal alone (`util.h`, `../util.dart`) matches no file in the graph.
/// `search_subdirs` stands in for directories a build system puts on the
/// search path (`include/` for C, `lib/` for Dart), and `walk_up` extends the
/// search to ancestor directories -- which suits a compiler search path but
/// not a strictly file-relative form such as Julia's `include`.
pub(super) fn resolve_import_path(
    literal: &str,
    file_path: &str,
    repo_root: Option<&Path>,
    search_subdirs: &[&str],
    walk_up: bool,
) -> Option<String> {
    if literal.is_empty() || literal.starts_with('/') {
        return None;
    }
    let mut current = Path::new(file_path).parent()?.to_path_buf();
    loop {
        let candidate = current.join(literal);
        if import_candidate_exists(&candidate, repo_root) {
            return Some(normalize_relative_path(&candidate));
        }
        for subdir in search_subdirs {
            let candidate = current.join(subdir).join(literal);
            if import_candidate_exists(&candidate, repo_root) {
                return Some(normalize_relative_path(&candidate));
            }
        }
        if !walk_up || !current.pop() {
            break;
        }
    }
    None
}

pub(super) fn import_candidate_exists(candidate: &Path, repo_root: Option<&Path>) -> bool {
    repo_root
        .map(|root| root.join(candidate).is_file())
        .unwrap_or_else(|| candidate.is_file())
}

/// Collects dotted namespace paths from every `kinds` declaration in the tree.
///
/// The path comes from `name_field` when the grammar exposes one, else from the
/// first child matching `name_kinds`.
pub(super) fn collect_namespace_paths(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    name_field: Option<&str>,
    name_kinds: &[&str],
) -> Vec<String> {
    let mut found = Vec::new();
    collect_namespace_paths_into(node, source, kinds, name_field, name_kinds, &mut found);
    found
}

fn collect_namespace_paths_into(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    name_field: Option<&str>,
    name_kinds: &[&str],
    found: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            let name = name_field
                .and_then(|field| child.child_by_field_name(field))
                .or_else(|| {
                    let mut inner = child.walk();
                    let candidate = child
                        .children(&mut inner)
                        .find(|candidate| name_kinds.contains(&candidate.kind()));
                    candidate
                });
            if let Some(name) = name {
                found.push(node_text(name, source).trim().to_string());
            }
        }
        collect_namespace_paths_into(child, source, kinds, name_field, name_kinds, found);
    }
}

pub(super) fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && matches!(bytes[0], b'\'' | b'"')
        && bytes.last().is_some_and(|last| *last == bytes[0])
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}
