//! Python's path operations where a tool reports their result: the
//! non-strict `Path.resolve()` (`posixpath.realpath`), `paths.is_project_root`
//! with its jj workspace check.

use std::path::{Path, PathBuf};

/// `os.path.realpath(path, strict=False)` (what `Path.resolve()` runs) on a
/// POSIX path: symlinks resolved as far as the path exists, the rest joined
/// on, `..` taken lexically after a missing or unreadable component, and a
/// symlink loop left unresolved. `None` for an embedded NUL, where Python
/// raises `ValueError`.
pub(crate) fn realpath(path: &str) -> Option<String> {
    use std::collections::HashMap;
    if path.contains('\0') {
        return None;
    }
    // `None` marks a resolved symlink target; the link's path sits below it.
    let mut rest: Vec<Option<String>> =
        path.split('/').rev().map(|p| Some(p.to_string())).collect();
    let mut part_count = rest.len();
    let mut resolved = if path.starts_with('/') {
        "/".to_string()
    } else {
        std::env::current_dir().ok()?.to_str()?.to_string()
    };
    let mut seen: HashMap<String, Option<String>> = HashMap::new();
    while part_count > 0 {
        let Some(name) = rest.pop()? else {
            let Some(Some(link)) = rest.pop() else {
                return None;
            };
            seen.insert(link, Some(resolved.clone()));
            continue;
        };
        part_count -= 1;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            let cut = resolved.rfind('/').unwrap_or(0);
            resolved.truncate(cut);
            if resolved.is_empty() {
                resolved.push('/');
            }
            continue;
        }
        let next = if resolved == "/" {
            format!("/{name}")
        } else {
            format!("{resolved}/{name}")
        };
        let Ok(meta) = std::fs::symlink_metadata(&next) else {
            resolved = next;
            continue;
        };
        if !meta.file_type().is_symlink() {
            resolved = next;
            continue;
        }
        if let Some(known) = seen.get(&next) {
            match known {
                Some(target) => resolved = target.clone(),
                // A loop: non-strict keeps the link's own path.
                None => resolved = next,
            }
            continue;
        }
        let Ok(target) = std::fs::read_link(&next) else {
            resolved = next;
            continue;
        };
        let target = target.to_str()?.to_string();
        if target.starts_with('/') {
            resolved = "/".to_string();
        }
        seen.insert(next.clone(), None);
        rest.push(Some(next));
        rest.push(None);
        let parts: Vec<&str> = target.split('/').collect();
        part_count += parts.len();
        rest.extend(parts.into_iter().rev().map(|p| Some(p.to_string())));
    }
    Some(resolved)
}

/// `str.isspace()`: Rust's whitespace plus the four information separators.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `jj_workspace._read_link`: the path a jj pointer file holds, relative to
/// the file's directory. `None` where Python reads none; `Err` for a file that
/// is not UTF-8, where Python raises.
fn read_jj_link(path: &Path) -> Result<Option<PathBuf>, ()> {
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(None);
    };
    let text = String::from_utf8(bytes).map_err(|_| ())?;
    let text = text.trim_matches(is_py_space);
    if text.is_empty() {
        return Ok(None);
    }
    let target = PathBuf::from(text);
    Ok(Some(if target.is_absolute() {
        target
    } else {
        path.parent().unwrap_or(Path::new("")).join(target)
    }))
}

/// `jj_workspace.jj_git_dir(root) is not None`.
fn has_jj_git_dir(root: &Path) -> Result<bool, ()> {
    let jj = root.join(".jj");
    if !jj.is_dir() {
        return Ok(false);
    }
    let repo = jj.join("repo");
    let repo_dir = if repo.is_file() {
        read_jj_link(&repo)?
    } else if repo.is_dir() {
        Some(repo)
    } else {
        None
    };
    let Some(repo_dir) = repo_dir else {
        return Ok(false);
    };
    let Some(git_dir) = read_jj_link(&repo_dir.join("store").join("git_target"))? else {
        return Ok(false);
    };
    let Some(git_dir) = git_dir.to_str().and_then(realpath) else {
        return Err(());
    };
    Ok(Path::new(&git_dir).is_dir())
}

/// `paths.is_project_root`: a `.git` or `.svn` checkout, a git-backed jj
/// workspace, or a directory holding a graph. `None` where Python raises.
pub(crate) fn is_project_root(path: &Path) -> Option<bool> {
    if path.join(".git").exists() || path.join(".svn").exists() {
        return Some(true);
    }
    // `is_jj_workspace`: no `.git` of its own (checked above).
    if has_jj_git_dir(path).ok()? {
        return Some(true);
    }
    Some(path.join(".dagayn").join("graph.db").is_file())
}

#[cfg(test)]
mod tests {
    use super::realpath;

    #[test]
    fn missing_components_are_joined_and_dotdot_is_lexical_after_them() {
        let base = std::env::temp_dir().canonicalize().expect("tmp");
        let base = base.to_str().expect("utf-8");
        assert_eq!(
            realpath(&format!("{base}/no-such-dir-zz/../x/./y//")).as_deref(),
            Some(format!("{base}/x/y").as_str())
        );
        assert_eq!(realpath("/").as_deref(), Some("/"));
        assert_eq!(realpath("/..").as_deref(), Some("/"));
        assert!(realpath("/a\0b").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_resolve_and_loops_stay() {
        let dir = std::env::temp_dir()
            .canonicalize()
            .expect("tmp")
            .join(format!("dagayn-pypath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real")).expect("mkdir");
        std::os::unix::fs::symlink("real", dir.join("link")).expect("link");
        std::os::unix::fs::symlink("loop", dir.join("loop")).expect("loop");
        let d = dir.to_str().expect("utf-8");
        assert_eq!(
            realpath(&format!("{d}/link/file")).as_deref(),
            Some(format!("{d}/real/file").as_str())
        );
        assert_eq!(
            realpath(&format!("{d}/loop/x")).as_deref(),
            Some(format!("{d}/loop/x").as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
