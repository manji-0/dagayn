//! Workspace packages: a monorepo member imported by its package name
//! (`import { x } from "@scope/pkg"`) resolves to the member's files.
//!
//! The workspace root is the nearest directory, from the importer up to the
//! repository root, holding a `pnpm-workspace.yaml` or a `package.json` with
//! `workspaces` (npm / yarn: an array, or `{ "packages": [...] }`). Its
//! member globs (`!` excludes) select the directories whose `package.json`
//! `name` is importable. A specifier `name` or `name/sub` then goes through
//! the member's `exports` (conditions `types`, `import`, `default`,
//! `require`, then any other; `./*` patterns), then `types` / `typings`,
//! `module`, `main`, then `index.*` (or the subpath under the member
//! directory). The first candidate that exists in the repository wins, so a
//! `dist/` entry that was never built gives way to the next condition.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{GlobBuilder, GlobSetBuilder};
use serde_json::Value;

use super::js_resolve::{javascript_package_name, probe_javascript_module_candidate};

/// The members of one workspace, by package name.
pub(super) struct JavaScriptWorkspace {
    packages: HashMap<String, (PathBuf, Value)>,
}

/// Directory -> its workspace (`None`: no workspace above it), and
/// workspace root -> members, so each workspace is read once.
#[derive(Default)]
pub(super) struct JavaScriptWorkspaceCache {
    by_dir: RefCell<HashMap<PathBuf, Option<Arc<JavaScriptWorkspace>>>>,
    by_root: RefCell<HashMap<PathBuf, Arc<JavaScriptWorkspace>>>,
}

/// Directories never searched for members.
const SKIPPED_DIRS: [&str; 2] = ["node_modules", "bower_components"];

/// How deep a `**` member glob searches below its literal prefix.
const MAX_GLOB_DEPTH: usize = 8;

/// The repository file `module` names when it imports a workspace member,
/// or `None` when it names no member of the importer's workspace.
pub(super) fn resolve_javascript_workspace_package(
    module: &str,
    file_path: &str,
    repo_root: &Path,
    cache: Option<&JavaScriptWorkspaceCache>,
) -> Option<String> {
    let name = javascript_package_name(module)?;
    let start = repo_root.join(file_path).parent()?.to_path_buf();
    let workspace = find_workspace(&start, repo_root, cache)?;
    let (dir, manifest) = workspace.packages.get(name)?;
    let subpath = &module[name.len()..];
    let key = format!(".{subpath}");
    let probe =
        |target: &str| probe_javascript_module_candidate(&dir.join(target), Some(repo_root));
    if let Some(exports) = manifest.get("exports") {
        let mut targets = Vec::new();
        exports_targets(exports, &key, &mut targets);
        if let Some(found) = targets.iter().find_map(|target| probe(target)) {
            return Some(found);
        }
    }
    if subpath.is_empty() {
        ["types", "typings", "module", "main"]
            .iter()
            .filter_map(|field| manifest.get(*field).and_then(Value::as_str))
            .find_map(probe)
            .or_else(|| probe("."))
    } else {
        probe(subpath.trim_start_matches('/'))
    }
}

fn find_workspace(
    start: &Path,
    repo_root: &Path,
    cache: Option<&JavaScriptWorkspaceCache>,
) -> Option<Arc<JavaScriptWorkspace>> {
    if let Some(cache) = cache
        && let Some(cached) = cache.by_dir.borrow().get(start)
    {
        return cached.clone();
    }
    let mut found = None;
    let mut current = Some(start);
    while let Some(dir) = current.filter(|dir| dir.starts_with(repo_root)) {
        if let Some(globs) = workspace_globs(dir) {
            let cached = cache.and_then(|cache| cache.by_root.borrow().get(dir).cloned());
            let workspace = cached.unwrap_or_else(|| {
                let workspace = Arc::new(read_workspace(dir, &globs));
                if let Some(cache) = cache {
                    cache
                        .by_root
                        .borrow_mut()
                        .insert(dir.to_path_buf(), workspace.clone());
                }
                workspace
            });
            found = Some(workspace);
            break;
        }
        current = dir.parent();
    }
    if let Some(cache) = cache {
        cache
            .by_dir
            .borrow_mut()
            .insert(start.to_path_buf(), found.clone());
    }
    found
}

/// The member globs `dir` declares, if it is a workspace root.
fn workspace_globs(dir: &Path) -> Option<Vec<String>> {
    if let Ok(text) = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")) {
        return Some(pnpm_workspace_globs(&text));
    }
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let manifest: Value = serde_json::from_str(&text).ok()?;
    let workspaces = manifest.get("workspaces")?;
    let globs = workspaces
        .as_array()
        .or_else(|| workspaces.get("packages").and_then(Value::as_array))?;
    Some(
        globs
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    )
}

/// The `packages` list of a `pnpm-workspace.yaml`, block (`- glob`) or flow
/// (`[a, b]`) style. Only that key is read, so a line-based reader does
/// instead of a YAML dependency.
fn pnpm_workspace_globs(text: &str) -> Vec<String> {
    let mut globs = Vec::new();
    let mut in_packages = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if in_packages && let Some(item) = trimmed.strip_prefix('-') {
            globs.push(yaml_scalar(item));
            continue;
        }
        if line.starts_with([' ', '\t']) {
            continue;
        }
        in_packages = false;
        if let Some(rest) = trimmed.strip_prefix("packages:") {
            let rest = rest.trim();
            if let Some(flow) = rest.strip_prefix('[') {
                let flow = flow.split(']').next().unwrap_or(flow);
                globs.extend(
                    flow.split(',')
                        .map(yaml_scalar)
                        .filter(|glob| !glob.is_empty()),
                );
            } else {
                in_packages = rest.is_empty() || rest.starts_with('#');
            }
        }
    }
    globs
}

/// A plain or quoted YAML scalar, without a trailing comment.
fn yaml_scalar(raw: &str) -> String {
    let raw = raw.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = raw.strip_prefix(quote) {
            return inner.split(quote).next().unwrap_or(inner).to_string();
        }
    }
    raw.split(" #").next().unwrap_or(raw).trim().to_string()
}

fn read_workspace(root: &Path, globs: &[String]) -> JavaScriptWorkspace {
    let mut include = GlobSetBuilder::new();
    let mut exclude = GlobSetBuilder::new();
    let mut prefixes = Vec::new();
    for glob in globs {
        let (negated, pattern) = match glob.strip_prefix('!') {
            Some(pattern) => (true, pattern),
            None => (false, glob.as_str()),
        };
        let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
        let Ok(compiled) = GlobBuilder::new(pattern).literal_separator(true).build() else {
            continue;
        };
        if negated {
            exclude.add(compiled);
            continue;
        }
        include.add(compiled);
        let literal: Vec<&str> = pattern
            .split('/')
            .take_while(|part| !part.contains(['*', '?', '[', '{']))
            .collect();
        let depth = if pattern.contains("**") {
            MAX_GLOB_DEPTH
        } else {
            pattern.split('/').count() - literal.len()
        };
        prefixes.push((literal.join("/"), depth));
    }
    let (Ok(include), Ok(exclude)) = (include.build(), exclude.build()) else {
        return JavaScriptWorkspace {
            packages: HashMap::new(),
        };
    };
    let mut dirs = Vec::new();
    for (prefix, depth) in prefixes {
        collect_dirs(root, &prefix, depth, &mut dirs);
    }
    dirs.sort();
    dirs.dedup();
    let mut packages = HashMap::new();
    for rel in dirs {
        if !include.is_match(&rel) || exclude.is_match(&rel) {
            continue;
        }
        let dir = root.join(&rel);
        let Some(manifest) = std::fs::read_to_string(dir.join("package.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        if let Some(name) = manifest.get("name").and_then(Value::as_str) {
            packages.entry(name.to_string()).or_insert((dir, manifest));
        }
    }
    JavaScriptWorkspace { packages }
}

/// `rel` (relative to `root`) and its subdirectories down to `depth` more
/// levels, as relative paths.
fn collect_dirs(root: &Path, rel: &str, depth: usize, out: &mut Vec<String>) {
    let dir = root.join(rel);
    if !dir.is_dir() {
        return;
    }
    out.push(rel.to_string());
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.')
            || SKIPPED_DIRS.contains(&name.as_str())
            || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        collect_dirs(root, &child, depth - 1, out);
    }
}

/// Conditions tried first, in order; any other condition follows in
/// manifest order.
const PREFERRED_CONDITIONS: [&str; 4] = ["types", "import", "default", "require"];

/// The targets `exports` gives for subpath `key` (`.` or `./sub`), most
/// preferred first.
fn exports_targets(exports: &Value, key: &str, out: &mut Vec<String>) {
    let subpaths = exports
        .as_object()
        .filter(|map| map.keys().any(|name| name.starts_with('.')));
    let Some(subpaths) = subpaths else {
        // A string, an array, or a conditions object: the `.` entry.
        if key == "." {
            condition_targets(exports, None, out);
        }
        return;
    };
    if let Some(value) = subpaths.get(key) {
        condition_targets(value, None, out);
        return;
    }
    // `./*` patterns: the longest prefix wins, as in Node.
    let best = subpaths
        .iter()
        .filter_map(|(pattern, value)| {
            let (prefix, suffix) = pattern.split_once('*')?;
            let star = key.strip_prefix(prefix)?.strip_suffix(suffix)?;
            Some((prefix.len(), star, value))
        })
        .max_by_key(|(len, _, _)| *len);
    if let Some((_, star, value)) = best {
        condition_targets(value, Some(star), out);
    }
}

fn condition_targets(value: &Value, star: Option<&str>, out: &mut Vec<String>) {
    match value {
        Value::String(target) => out.push(match star {
            Some(star) => target.replace('*', star),
            None => target.clone(),
        }),
        Value::Array(items) => {
            for item in items {
                condition_targets(item, star, out);
            }
        }
        Value::Object(conditions) => {
            for name in PREFERRED_CONDITIONS {
                if let Some(value) = conditions.get(name) {
                    condition_targets(value, star, out);
                }
            }
            for (name, value) in conditions {
                if !PREFERRED_CONDITIONS.contains(&name.as_str()) {
                    condition_targets(value, star, out);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pnpm_workspace_packages_in_block_and_flow_style() {
        let block = "# members\npackages:\n  - \"packages/*\"\n  - 'apps/**' # all apps\n  - tools\n- '!**/test/**'\ncatalog:\n  - not-a-glob\n";
        assert_eq!(
            pnpm_workspace_globs(block),
            ["packages/*", "apps/**", "tools", "!**/test/**"]
        );
        assert_eq!(
            pnpm_workspace_globs("packages: ['a/*', \"b\"]\n"),
            ["a/*", "b"]
        );
        assert!(pnpm_workspace_globs("catalog:\n  x: 1\n").is_empty());
    }

    #[test]
    fn picks_exports_targets_by_subpath_and_condition() {
        let exports: Value = serde_json::from_str(
            r#"{
  ".": { "node": "./n.js", "types": "./t.d.ts", "default": "./d.js" },
  "./util": ["./u.js"],
  "./feature/*": "./src/feature/*.ts",
  "./feature/deep/*": { "import": "./deep/*.js" },
  "./gone": null
}"#,
        )
        .unwrap();
        let targets = |key: &str| {
            let mut out = Vec::new();
            exports_targets(&exports, key, &mut out);
            out
        };
        assert_eq!(targets("."), ["./t.d.ts", "./d.js", "./n.js"]);
        assert_eq!(targets("./util"), ["./u.js"]);
        assert_eq!(targets("./feature/x"), ["./src/feature/x.ts"]);
        assert_eq!(targets("./feature/deep/y"), ["./deep/y.js"]);
        assert!(targets("./gone").is_empty());
        assert!(targets("./missing").is_empty());

        let mut out = Vec::new();
        exports_targets(&Value::String("./main.js".into()), ".", &mut out);
        assert_eq!(out, ["./main.js"]);
        out.clear();
        exports_targets(&Value::String("./main.js".into()), "./sub", &mut out);
        assert!(out.is_empty());
    }
}
