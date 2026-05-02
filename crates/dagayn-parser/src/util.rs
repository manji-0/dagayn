use std::path::Path;

use sha2::{Digest, Sha256};

use super::types::ParsedEdge;

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
                edge.kind.clone(),
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
