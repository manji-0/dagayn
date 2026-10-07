//! The `complex_hotspot` and `undocumented_surface` findings of
//! `refactor_tool(mode="suggest")`
//! (docs/plans/REFACTOR-TOOL-TARGET.md#finding-kinds).

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use dagayn_graph::{GraphNode, GraphStore};
use serde_json::{Value, json};

use crate::dead_code::source_lines;
use crate::findings::is_production_code;

/// The window `complex_hotspot` counts commits in.
const WINDOW: &str = "--since=90.days";
/// Commits in the window that changed a function's lines before its size
/// counts as a hotspot.
pub(crate) const HOTSPOT_MIN_COMMITS: usize = 5;

/// `git` in `root`, its stdout lines; `None` when git fails (not a
/// repository, no git).
fn git_lines(root: &Path, args: &[&str]) -> Option<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    })
}

/// A graph path as git names it: relative to `root`.
fn repo_relative(root: &Path, file: &str) -> String {
    Path::new(file).strip_prefix(root).map_or_else(
        |_| file.to_string(),
        |rel| rel.to_string_lossy().into_owned(),
    )
}

/// `complex_hotspot`: the split candidates (`split` suggestions) whose
/// lines changed in at least [`HOTSPOT_MIN_COMMITS`] commits of the window,
/// most changed first. `None` when the repository has no git history.
pub(crate) fn complex_hotspots(
    store: &GraphStore,
    root: &Path,
    splits: &[&Value],
) -> Option<Vec<Value>> {
    // One pass over the window first: a file changed fewer times cannot
    // hold a function changed more.
    let mut file_commits: HashMap<String, usize> = HashMap::new();
    for file in git_lines(root, &["log", WINDOW, "--format=", "--name-only"])? {
        *file_commits.entry(file).or_default() += 1;
    }
    let mut found: Vec<(usize, Value)> = Vec::new();
    for split in splits {
        let Some(qn) = split["symbols"].get(0).and_then(Value::as_str) else {
            continue;
        };
        let Ok(Some(node)) = store.get_node(qn) else {
            continue;
        };
        if node.kind != "Function" {
            continue;
        }
        let rel = repo_relative(root, &node.file_path);
        if file_commits.get(&rel).copied().unwrap_or(0) < HOTSPOT_MIN_COMMITS {
            continue;
        }
        let range = format!("-L{},{}:{rel}", node.line_start, node.line_end);
        let Some(commits) = git_lines(root, &["log", WINDOW, "--format=%H", "-s", &range]) else {
            continue;
        };
        if commits.len() < HOTSPOT_MIN_COMMITS {
            continue;
        }
        let evidence = &split["evidence"];
        found.push((
            commits.len(),
            json!({
                "kind": "complex_hotspot",
                "qualified_name": node.qualified_name,
                "file": node.file_path,
                "line": node.line_start,
                "claim": format!(
                    "{} is long ({} lines) and changed in {} commits in the last 90 days.",
                    node.name,
                    node.line_end - node.line_start + 1,
                    commits.len()
                ),
                "evidence": {
                    "commits_last_90_days": commits.len(),
                    "line_count": evidence.get("line_count").cloned().unwrap_or(Value::Null),
                    "branch_count": evidence.get("branch_count").cloned().unwrap_or(Value::Null),
                },
                "action": "Split it: extract one responsibility at a time, starting with the part that changes most.",
            }),
        ));
    }
    found.sort_by_key(|(commits, _)| std::cmp::Reverse(*commits));
    Some(found.into_iter().map(|(_, finding)| finding).collect())
}

/// Whether `node` carries documentation: a Python docstring, or a comment
/// block right above it (attributes and decorators skipped).
pub(crate) fn has_doc_comment(lines: &[String], node: &GraphNode) -> bool {
    let start = usize::try_from(node.line_start.max(1) - 1).unwrap_or(0);
    if start >= lines.len() {
        return true;
    }
    if node.language == "python" {
        // The body starts after the line that closes the signature.
        let Some(colon) = (start..lines.len().min(start + 30)).find(|&i| {
            lines[i]
                .split('#')
                .next()
                .unwrap_or("")
                .trim_end()
                .ends_with(':')
        }) else {
            return false;
        };
        return lines[colon + 1..]
            .iter()
            .map(|line| line.trim())
            .find(|line| !line.is_empty())
            .is_some_and(|line| {
                let line = line.trim_start_matches(['r', 'R', 'u', 'U', 'b', 'B']);
                line.starts_with("\"\"\"") || line.starts_with("'''") || line.starts_with('"')
            });
    }
    let mut above = start;
    while above > 0 {
        above -= 1;
        let line = lines[above].trim();
        if line.starts_with("#[") || line.starts_with('@') {
            continue;
        }
        return ["///", "//!", "//", "/**", "/*", "*", "--", "#"]
            .iter()
            .any(|marker| line.starts_with(marker));
    }
    false
}

/// `undocumented_surface`: each unit's `surface` symbols (the ones other
/// units use most) without documentation.
pub(crate) fn undocumented_surface(store: &GraphStore, root: &Path) -> Option<Vec<Value>> {
    let nodes = store.get_all_nodes_filtered(false).ok()?;
    let edges = store.get_all_edges().ok()?;
    let (units, _) = crate::units::unit_map(root, &nodes, &edges, true);
    let by_qn: HashMap<&str, &GraphNode> = nodes
        .iter()
        .map(|n| (n.qualified_name.as_str(), n))
        .collect();
    let mut cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut found = Vec::new();
    for unit in &units {
        for entry in unit["surface"].as_array().into_iter().flatten() {
            let Some(node) = entry["qualified_name"]
                .as_str()
                .and_then(|qn| by_qn.get(qn))
            else {
                continue;
            };
            if !matches!(node.kind.as_str(), "Function" | "Class" | "Type")
                || !is_production_code(node, &node.file_path)
            {
                continue;
            }
            let lines = cache
                .entry(node.file_path.clone())
                .or_insert_with(|| source_lines(store, &node.file_path));
            if lines.is_empty() || has_doc_comment(lines, node) {
                continue;
            }
            found.push(json!({
                "kind": "undocumented_surface",
                "qualified_name": node.qualified_name,
                "file": node.file_path,
                "line": node.line_start,
                "claim": format!(
                    "{} is among the symbols other units use most from {} and has no documentation.",
                    node.name,
                    unit["name"].as_str().unwrap_or("its unit")
                ),
                "evidence": {"unit": unit["name"], "used_by": entry["used_by"]},
                "action": "Document its contract: other units depend on it.",
            }));
        }
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(language: &str, line_start: i64) -> GraphNode {
        GraphNode {
            id: 0,
            kind: "Function".into(),
            name: "f".into(),
            qualified_name: "a::f".into(),
            file_path: "a".into(),
            line_start,
            line_end: line_start + 2,
            language: language.into(),
            parent_name: None,
            params: None,
            return_type: None,
            is_test: false,
            file_hash: None,
            extra: Value::Null,
            signature: None,
        }
    }

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn documentation_is_a_docstring_or_a_comment_above() {
        let python = lines("def f(\n    x,\n):\n    \"\"\"Doc.\"\"\"\n    return x\n");
        assert!(has_doc_comment(&python, &node("python", 1)));
        let bare = lines("def f(x):\n    return x\n");
        assert!(!has_doc_comment(&bare, &node("python", 1)));
        let rust = lines("/// Doc.\n#[inline]\npub fn f() {}\n");
        assert!(has_doc_comment(&rust, &node("rust", 3)));
        let undocumented = lines("}\n\n#[derive(Debug)]\npub struct F;\n");
        assert!(!has_doc_comment(&undocumented, &node("rust", 4)));
    }
}
