//! `source_of` rows (`dagayn.tools.node_source.read_live_node_source`).

use std::path::{Path, PathBuf};

use dagayn_graph::GraphNode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::coverage::splitlines;
use crate::pypath::realpath;

/// `SOURCE_OF_MAX_CHARS`.
const MAX_CHARS: usize = 4000;

/// What `_attach_source_of_coverage` reads off the row.
pub(crate) struct SourceCoverage {
    pub read_error: Option<&'static str>,
    pub stale: bool,
    truncated: bool,
    omitted_chars: usize,
    omitted_lines: usize,
}

impl SourceCoverage {
    pub(crate) fn value(&self) -> Value {
        json!({
            "max_chars": MAX_CHARS,
            "truncated": self.truncated,
            "source_stale": self.stale,
            "read_error": self.read_error,
            "omitted_chars": self.omitted_chars,
            "omitted_lines": self.omitted_lines,
        })
    }

    /// `_source_of_missingness`.
    pub(crate) fn missingness(&self) -> Vec<Value> {
        let mut items = Vec::new();
        if let Some(error) = self.read_error {
            items.push(json!({
                "reason_code": "source_unreadable",
                "severity": "medium",
                "claim_effect": format!("live source was not read ({error})"),
            }));
        }
        if self.stale {
            items.push(json!({
                "reason_code": "source_stale",
                "severity": "medium",
                "claim_effect": "worktree file_hash differs from the graph; the stored span may not match the live body",
            }));
        }
        if self.truncated {
            items.push(json!({
                "reason_code": "live_source_truncated",
                "severity": "low",
                "claim_effect": format!(
                    "{} character(s) omitted; Read the file for the rest",
                    self.omitted_chars
                ),
            }));
        }
        items
    }
}

/// `node_source_line_span`: a clamped `[start, end)`, and for a
/// `DocSection` the lines up to the next heading of its level or higher.
fn line_span(node: &GraphNode, lines: &[&str]) -> (usize, usize) {
    let count = lines.len() as i64;
    let line_start = if node.line_start == 0 {
        1
    } else {
        node.line_start
    };
    let line_end = if node.line_end == 0 {
        line_start
    } else {
        node.line_end
    };
    let start = (line_start - 1).max(0).min(count) as usize;
    let end = line_end.max(line_start).min(count) as usize;
    if node.kind != "DocSection" {
        return (start, end);
    }
    let heading = |line: &str| -> Option<usize> {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        let rest = &line[hashes..];
        ((1..=6).contains(&hashes) && rest.starts_with(char::is_whitespace)).then_some(hashes)
    };
    let level = lines.get(start).and_then(|line| heading(line));
    let end = (start + 1..lines.len())
        .find(|idx| {
            heading(lines[*idx]).is_some_and(|found| level.is_none_or(|level| found <= level))
        })
        .unwrap_or(lines.len());
    (start, end)
}

/// The base row `_base_payload` builds when the file is not read, with its
/// `read_error`.
fn unread_row(
    node: &GraphNode,
    error: &'static str,
) -> (Vec<(&'static str, Value)>, SourceCoverage) {
    let coverage = SourceCoverage {
        read_error: Some(error),
        stale: false,
        truncated: false,
        omitted_chars: 0,
        omitted_lines: 0,
    };
    let row = vec![
        ("signature", json!(node.signature)),
        ("params", json!(node.params)),
        ("return_type", json!(node.return_type)),
        ("source", json!("")),
        ("truncated", json!(false)),
        ("source_stale", json!(false)),
        ("read_error", json!(error)),
        ("omitted_chars", json!(0)),
        ("omitted_lines", json!(0)),
        ("max_chars", json!(MAX_CHARS)),
        ("span_line_start", json!(node.line_start)),
        ("span_line_end", json!(node.line_end)),
    ];
    (row, coverage)
}

/// The `source_of` row (before compaction) and its coverage, as
/// `read_live_node_source` reads the file: through `resolve_contained_path`
/// (Python's non-strict `resolve()`), with a `read_error` row for a missing
/// path, one that escapes the repository, one that is not a file, or one
/// that cannot be read. `None` only for a path holding a NUL, where Python
/// raises.
pub(crate) fn source_row(
    node: &GraphNode,
    root: &Path,
) -> Option<(Vec<(&'static str, Value)>, SourceCoverage)> {
    if node.file_path.is_empty() {
        return Some(unread_row(node, "missing_path"));
    }
    let joined = if node.file_path.starts_with('/') {
        node.file_path.clone()
    } else {
        format!("{}/{}", root.to_str()?, node.file_path)
    };
    let resolved = PathBuf::from(realpath(&joined)?);
    let root = PathBuf::from(realpath(root.to_str()?)?);
    if !resolved.starts_with(&root) {
        return Some(unread_row(node, "path_escapes_repo"));
    }
    if !resolved.is_file() {
        return Some(unread_row(node, "not_a_file"));
    }
    let Ok(raw) = std::fs::read(&resolved) else {
        return Some(unread_row(node, "unreadable"));
    };
    let live_hash = format!("{:x}", Sha256::digest(&raw));
    let stored = node.file_hash.as_deref().unwrap_or("");
    let stale = !stored.is_empty() && stored != live_hash;
    let text = String::from_utf8_lossy(&raw);
    let lines = splitlines(&text);
    let (start, end) = line_span(node, &lines);
    let span = lines[start..end].join("\n");
    let span_chars = span.chars().count();
    let truncated = span_chars > MAX_CHARS;
    let source: String = if truncated {
        span.chars().take(MAX_CHARS).collect()
    } else {
        span
    };
    let kept_chars = source.chars().count();
    let span_line_count = end - start;
    let omitted_lines = if span_line_count == 0 {
        0
    } else if source.is_empty() {
        span_line_count
    } else {
        span_line_count.saturating_sub(source.matches('\n').count() + 1)
    };
    let coverage = SourceCoverage {
        read_error: None,
        stale,
        truncated,
        omitted_chars: span_chars - kept_chars,
        omitted_lines,
    };
    let row = vec![
        ("signature", json!(node.signature)),
        ("params", json!(node.params)),
        ("return_type", json!(node.return_type)),
        ("source", json!(source)),
        ("truncated", json!(truncated)),
        ("source_stale", json!(stale)),
        ("read_error", Value::Null),
        ("omitted_chars", json!(coverage.omitted_chars)),
        ("omitted_lines", json!(omitted_lines)),
        ("max_chars", json!(MAX_CHARS)),
        (
            "span_line_start",
            json!(if lines.is_empty() { 0 } else { start + 1 }),
        ),
        ("span_line_end", json!(end)),
    ];
    Some((row, coverage))
}

#[cfg(test)]
mod tests {
    use crate::coverage::splitlines as split_lines;

    #[test]
    fn lines_split_as_python_splits_them() {
        assert_eq!(
            split_lines("a\nb\r\nc\rd\u{2028}e\n"),
            ["a", "b", "c", "d", "e"]
        );
        assert_eq!(split_lines("a\n\nb"), ["a", "", "b"]);
        assert!(split_lines("").is_empty());
        assert_eq!(split_lines("\n"), [""]);
    }
}
