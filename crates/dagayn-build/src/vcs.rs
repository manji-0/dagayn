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
