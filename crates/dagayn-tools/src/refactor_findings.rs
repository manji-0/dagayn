//! The `complex_hotspot` findings of `refactor_tool(mode="suggest")`
//! (docs/plans/REFACTOR-TOOL-TARGET.md#finding-kinds).

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use dagayn_graph::GraphStore;
use serde_json::{Value, json};

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
