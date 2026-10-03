//! Version-control facts recorded in graph metadata.

use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vcs {
    Git,
    /// A git-backed jj workspace without its own `.git`.
    Jj,
    Svn,
    None,
}

/// Same markers, same precedence as `dagayn.incremental_files.detect_vcs`.
pub fn detect_vcs(root: &Path) -> Vcs {
    if root.join(".git").exists() {
        Vcs::Git
    } else if root.join(".jj").is_dir() {
        Vcs::Jj
    } else if root.join(".svn").exists() {
        Vcs::Svn
    } else {
        Vcs::None
    }
}

/// `(branch, head_sha)`; each is empty when git cannot say.
pub(crate) fn git_branch_info(repo_root: &Path) -> (String, String) {
    let branch = git_stdout(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let sha = git_stdout(repo_root, &["rev-parse", "HEAD"]);
    (branch, sha)
}

fn git_stdout(repo_root: &Path, args: &[&str]) -> String {
    match Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
    {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => String::new(),
    }
}

/// `_SAFE_GIT_REF`: a ref is passed to git only if it matches this shape.
pub(crate) fn is_safe_git_ref(reference: &str) -> bool {
    !reference.is_empty()
        && reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.~^/@{}-".contains(c))
}

/// The full sha `reference` names, or `None` when git cannot resolve it.
pub(crate) fn resolve_commit_sha(repo_root: &Path, reference: &str) -> Option<String> {
    if !is_safe_git_ref(reference) {
        return None;
    }
    let sha = git_stdout(
        repo_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ],
    );
    (!sha.is_empty()).then_some(sha)
}

/// Files changed between `base` and HEAD plus staged, unstaged, and untracked
/// working-tree paths. Both sides of a rename are reported: the old path's
/// nodes have to be pruned and the new one parsed.
///
/// `get_changed_file_sources(...)["files"]` in Python: the base diff first,
/// then the working tree, in first-seen order.
pub(crate) fn changed_files(repo_root: &Path, base: &str) -> Vec<String> {
    if !is_safe_git_ref(base) {
        return Vec::new();
    }
    let base_diff = git_raw(
        repo_root,
        &["diff", "--name-status", "-M", "-z", base, "HEAD", "--"],
    )
    .map(|payload| parse_name_status(&payload))
    .unwrap_or_default();
    let worktree = git_raw(
        repo_root,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    )
    .map(|payload| parse_porcelain(&payload))
    .unwrap_or_default();
    dedupe(base_diff.into_iter().chain(worktree))
}

fn git_raw(repo_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn nul_fields(payload: &str) -> Vec<&str> {
    payload
        .split('\0')
        .filter(|field| !field.is_empty())
        .collect()
}

/// `git diff --name-status -z`: `R100 old new` records carry two paths.
fn parse_name_status(payload: &str) -> Vec<String> {
    let fields = nul_fields(payload);
    let mut files = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let wanted = if fields[index].starts_with(['R', 'C']) {
            2
        } else {
            1
        };
        for offset in 1..=wanted {
            if let Some(path) = fields.get(index + offset) {
                files.push((*path).to_string());
            }
        }
        index += wanted + 1;
    }
    dedupe(files)
}

/// `git status --porcelain -z`, staged then unstaged then untracked; a rename
/// is two fields, new path first.
fn parse_porcelain(payload: &str) -> Vec<String> {
    let entries = nul_fields(payload);
    let (mut staged, mut unstaged, mut untracked) = (Vec::new(), Vec::new(), Vec::new());
    let mut index = 0;
    while index < entries.len() {
        let entry = entries[index];
        index += 1;
        let bytes = entry.as_bytes();
        if bytes.len() <= 3 {
            continue;
        }
        let (x, y) = (bytes[0], bytes[1]);
        let mut paths = vec![entry[3..].to_string()];
        if (matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C'))
            && let Some(old) = entries.get(index)
        {
            paths.push((*old).to_string());
            index += 1;
        }
        if x == b'?' && y == b'?' {
            untracked.extend(paths);
            continue;
        }
        if x != b' ' {
            staged.extend(paths.iter().cloned());
        }
        if y != b' ' {
            unstaged.extend(paths);
        }
    }
    dedupe(staged.into_iter().chain(unstaged).chain(untracked))
}

pub(crate) fn dedupe(paths: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// True when `repo_root` is a linked git worktree (its `.git` is a file).
pub fn is_linked_worktree(repo_root: &Path) -> bool {
    repo_root.join(".git").is_file()
}

/// The main checkout owning a linked worktree, or `None` for a main checkout.
pub fn main_checkout(repo_root: &Path) -> Option<std::path::PathBuf> {
    let common = git_stdout(
        repo_root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    let main = Path::new(common.strip_suffix("/.git")?);
    (main != repo_root).then(|| main.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_keeps_both_sides_of_a_rename() {
        let payload = "M\0a.py\0R100\0old.py\0new.py\0D\0gone.py\0M\0a.py\0";
        assert_eq!(
            parse_name_status(payload),
            vec!["a.py", "old.py", "new.py", "gone.py"]
        );
    }

    #[test]
    fn porcelain_orders_staged_unstaged_untracked() {
        let payload = "?? new.py\0 M edited.py\0R  moved.py\0orig.py\0M  staged.py\0";
        assert_eq!(
            parse_porcelain(payload),
            vec!["moved.py", "orig.py", "staged.py", "edited.py", "new.py"]
        );
    }

    #[test]
    fn unsafe_refs_are_rejected() {
        assert!(is_safe_git_ref("HEAD~1"));
        assert!(is_safe_git_ref("origin/main@{1}"));
        assert!(!is_safe_git_ref("--output=/tmp/x"));
        assert!(!is_safe_git_ref("a b"));
        assert!(!is_safe_git_ref("$(rm)"));
        assert!(!is_safe_git_ref(""));
    }
}
