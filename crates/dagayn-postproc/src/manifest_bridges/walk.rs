//! The one directory walk that finds every manifest.

use std::collections::HashMap;
use std::path::Path;

use dagayn_parser::IgnoreRules;

/// Repo-relative paths of files named `names`, sorted per name, in one
/// directory walk.
///
/// Ignored directories (`node_modules`, build outputs, ...) are pruned
/// instead of walked and filtered afterwards. Symlinks are never followed
/// nor reported, and `.git` is skipped.
pub(super) fn collect_named_files(
    repo_root: &Path,
    names: &[&str],
    ignore: &IgnoreRules,
) -> HashMap<String, Vec<String>> {
    let mut found: HashMap<String, Vec<String>> = names
        .iter()
        .map(|name| ((*name).to_string(), Vec::new()))
        .collect();
    let mut stack = vec![String::new()];
    while let Some(rel_dir) = stack.pop() {
        let dir = if rel_dir.is_empty() {
            repo_root.to_path_buf()
        } else {
            repo_root.join(&rel_dir)
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let prefix = if rel_dir.is_empty() {
            String::new()
        } else {
            format!("{rel_dir}/")
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if file_type.is_dir() {
                // `<dir>/_` so `build/**` prunes `build` itself.
                if name != ".git" && !ignore.is_ignored(&format!("{prefix}{name}/_")) {
                    stack.push(format!("{prefix}{name}"));
                }
                continue;
            }
            let Some(paths) = found.get_mut(&name) else {
                continue;
            };
            let rel = format!("{prefix}{name}");
            if !ignore.is_ignored(&rel) {
                paths.push(rel);
            }
        }
    }
    for paths in found.values_mut() {
        paths.sort();
    }
    found
}
