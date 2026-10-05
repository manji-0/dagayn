//! `dagayn.incremental_files.find_project_root`: the repository a command or
//! tool call means when it names none, from `CRG_REPO_ROOT`, the working
//! directory's checkout, and the editor's workspace hints.
//!
//! Only git checkouts are resolved. A walk that meets a jj workspace or an
//! SVN working copy, a home-relative hint Python would expand differently, or
//! hints that name several repositories (Python's
//! `AmbiguousWorkspaceRootError`) is answered with
//! [`ProjectRoot::Unsupported`], for the caller to leave to Python.

use std::path::{Path, PathBuf};

/// What [`find_project_root`] resolved.
#[derive(Debug, PartialEq, Eq)]
pub enum ProjectRoot {
    /// The root Python would pick.
    Found(PathBuf),
    /// A case only Python resolves, and why.
    Unsupported(String),
}

/// The nearest ancestor of `start` holding `.git` (`find_repo_root` for a
/// git checkout); `Err` when the walk first meets a jj workspace or finds no
/// checkout but an SVN working copy, which only Python resolves.
fn find_git_root(start: &Path) -> Result<Option<PathBuf>, String> {
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            return Ok(Some(dir.to_path_buf()));
        }
        if dir.join(".jj").is_dir() {
            return Err(format!("a jj workspace at {}", dir.display()));
        }
    }
    if start.ancestors().any(|dir| dir.join(".svn").exists()) {
        return Err(format!("an SVN working copy above {}", start.display()));
    }
    Ok(None)
}

/// `Path(raw).expanduser().resolve()` for a path that exists; `None` when it
/// does not, and `Err` for a `~` form this does not expand.
fn resolve_existing(raw: &str) -> Result<Option<PathBuf>, String> {
    let path = if raw == "~" || raw.starts_with("~/") {
        let Some(home) = std::env::var_os("HOME") else {
            return Err(format!("expanding {raw} without HOME"));
        };
        PathBuf::from(home).join(raw.trim_start_matches('~').trim_start_matches('/'))
    } else if raw.starts_with('~') {
        return Err(format!("expanding {raw}"));
    } else {
        PathBuf::from(raw)
    };
    Ok(path.canonicalize().ok())
}

/// `_workspace_folder_candidates`: the existing, distinct workspace folders
/// the editor hints at, in Python's order.
fn workspace_candidates() -> Result<Vec<PathBuf>, String> {
    let mut raw_candidates: Vec<String> = Vec::new();
    for var in ["CURSOR_PROJECT_DIR", "CLAUDE_PROJECT_DIR"] {
        let value = std::env::var(var).unwrap_or_default();
        let value = value.trim();
        if !value.is_empty() {
            raw_candidates.push(value.to_string());
        }
    }
    let folders = std::env::var("WORKSPACE_FOLDER_PATHS").unwrap_or_default();
    let folders = folders.trim();
    if !folders.is_empty() {
        if folders.starts_with('[') {
            if let Ok(serde_json::Value::Array(items)) = serde_json::from_str(folders) {
                for item in items {
                    raw_candidates.push(match item {
                        serde_json::Value::String(text) => text,
                        // `str(item)` of a non-string is Python's repr.
                        _ => return Err("a non-string WORKSPACE_FOLDER_PATHS entry".to_string()),
                    });
                }
            }
        } else if folders.contains(',') {
            raw_candidates.extend(
                folders
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_string),
            );
        } else {
            raw_candidates.extend(
                folders
                    .split(':')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_string),
            );
        }
    }
    let mut resolved: Vec<PathBuf> = Vec::new();
    for raw in raw_candidates {
        if let Some(path) = resolve_existing(&raw)?
            && !resolved.contains(&path)
        {
            resolved.push(path);
        }
    }
    Ok(resolved)
}

/// `_contains_path(root, path)`: `path` is `root` or lies under it.
fn contains_path(root: &Path, path: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(&root)
}

/// `_hinted_repo_roots`: each hinted folder's checkout, or the folder itself
/// when it holds a `.dagayn` directory, without repeats.
fn hinted_repo_roots(candidates: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for workspace in candidates {
        let root = match find_git_root(workspace)? {
            Some(root) => root,
            None if workspace.join(".dagayn").is_dir() => workspace.clone(),
            None => continue,
        };
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// `_pick_workspace_root`: the innermost hinted root holding `prefer`, else
/// the only hinted root, else `None`.
fn pick_workspace_root(candidates: &[PathBuf], prefer: &Path) -> Result<Option<PathBuf>, String> {
    let (mut holding, other): (Vec<PathBuf>, Vec<PathBuf>) = hinted_repo_roots(candidates)?
        .into_iter()
        .partition(|root| contains_path(root, prefer));
    if !holding.is_empty() {
        // Stable, like Python's `sort(reverse=True)` on equal keys.
        holding.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        return Ok(Some(holding.swap_remove(0)));
    }
    Ok((other.len() == 1).then(|| other[0].clone()))
}

/// `find_project_root()` with no `start`, from the working directory `cwd`.
pub fn find_project_root(cwd: &Path) -> ProjectRoot {
    match find_project_root_inner(cwd) {
        Ok(root) => ProjectRoot::Found(root),
        Err(reason) => ProjectRoot::Unsupported(reason),
    }
}

fn find_project_root_inner(cwd: &Path) -> Result<PathBuf, String> {
    let env_override = std::env::var("CRG_REPO_ROOT").unwrap_or_default();
    let env_override = env_override.trim();
    if !env_override.is_empty()
        && let Some(root) = resolve_existing(env_override)?
    {
        return Ok(root);
    }

    let root = find_git_root(cwd)?;
    let candidates = workspace_candidates()?;
    if !candidates.is_empty() {
        let covered = root.as_ref().is_some_and(|root| {
            candidates
                .iter()
                .any(|candidate| contains_path(candidate, root) || contains_path(root, candidate))
        });
        if !covered {
            if let Some(picked) = pick_workspace_root(&candidates, cwd)? {
                return Ok(picked);
            }
            let hinted = hinted_repo_roots(&candidates)?;
            if hinted.len() > 1 {
                return Err("workspace hints that name more than one repository".to_string());
            }
        }
    }
    Ok(root.unwrap_or_else(|| cwd.to_path_buf()))
}

/// `dagayn.paths.unsafe_root_reason`: the home directory or the filesystem
/// root, which no project is, unless `DAGAYN_ALLOW_WIDE_ROOT` is set.
pub fn unsafe_root_reason(root: &Path) -> Option<&'static str> {
    let allowed = std::env::var("DAGAYN_ALLOW_WIDE_ROOT")
        .map(|value| matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    if allowed {
        return None;
    }
    let resolved = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if resolved.parent().is_none() {
        return Some("the filesystem root");
    }
    let is_home = std::env::var_os("HOME")
        .and_then(|home| PathBuf::from(home).canonicalize().ok())
        .is_some_and(|home| home == resolved);
    is_home.then_some("your home directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dagayn-project-root-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn git_walk_stops_at_jj_and_svn() {
        let base = tempdir("walk");
        std::fs::create_dir_all(base.join("repo/.git")).unwrap();
        std::fs::create_dir_all(base.join("repo/src/deep")).unwrap();
        assert_eq!(
            find_git_root(&base.join("repo/src/deep")),
            Ok(Some(base.join("repo")))
        );
        std::fs::create_dir_all(base.join("repo/ws/.jj")).unwrap();
        assert!(find_git_root(&base.join("repo/ws")).is_err());
        std::fs::create_dir_all(base.join("svn/.svn")).unwrap();
        std::fs::create_dir_all(base.join("svn/a")).unwrap();
        assert!(find_git_root(&base.join("svn/a")).is_err());
        std::fs::create_dir_all(base.join("plain")).unwrap();
        assert_eq!(find_git_root(&base.join("plain")), Ok(None));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn workspace_pick_prefers_the_innermost_holder_then_a_single_root() {
        let base = tempdir("pick");
        for repo in ["outer", "outer/inner", "other"] {
            std::fs::create_dir_all(base.join(repo).join(".git")).unwrap();
        }
        let outer = base.join("outer");
        let inner = base.join("outer/inner");
        let other = base.join("other");
        let both = [outer.clone(), inner.clone()];
        assert_eq!(
            pick_workspace_root(&both, &inner.join("x")),
            Ok(Some(inner.clone()))
        );
        assert_eq!(
            pick_workspace_root(std::slice::from_ref(&other), &base),
            Ok(Some(other.clone()))
        );
        assert_eq!(pick_workspace_root(&[outer, other], &base), Ok(None));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn contains_path_is_component_wise() {
        assert!(contains_path(Path::new("/a/b"), Path::new("/a/b/c")));
        assert!(contains_path(Path::new("/a/b"), Path::new("/a/b")));
        assert!(!contains_path(Path::new("/a/b"), Path::new("/a/bc")));
    }

    #[test]
    fn tilde_forms_other_than_home_are_left_to_python() {
        assert!(resolve_existing("~someone/x").is_err());
        assert_eq!(resolve_existing("/definitely/not/here"), Ok(None));
    }
}
