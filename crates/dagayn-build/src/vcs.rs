//! Version-control facts recorded in graph metadata.

use std::path::Path;
use std::process::Command;

use crate::{jj, pyerr, svn};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vcs {
    Git,
    /// A git-backed jj workspace without its own `.git`.
    Jj,
    Svn,
    None,
}

/// Same markers, same precedence as `dagayn.incremental_files.detect_vcs`:
/// a `.jj` directory counts only as a git-backed workspace.
pub fn detect_vcs(root: &Path) -> Vcs {
    if root.join(".git").exists() {
        Vcs::Git
    } else if jj::is_jj_workspace(root) {
        Vcs::Jj
    } else if root.join(".svn").exists() {
        Vcs::Svn
    } else {
        Vcs::None
    }
}

/// `(branch, head_sha)`; each is empty when git cannot say. In a jj
/// workspace the branch is the nearest bookmark and the head is `@-`.
pub(crate) fn git_branch_info(repo_root: &Path) -> (String, String) {
    if jj::is_jj_workspace(repo_root) {
        return jj::working_copy(repo_root)
            .map(|wc| (wc.bookmark, wc.parent))
            .unwrap_or_default();
    }
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
/// Like Python's `$`, a single trailing newline is accepted.
pub fn is_safe_git_ref(reference: &str) -> bool {
    let reference = reference.strip_suffix('\n').unwrap_or(reference);
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
    let worktree = worktree_changes(repo_root);
    dedupe(base_diff.into_iter().chain(worktree))
}

/// Staged, unstaged, and untracked paths: the `worktree` group of
/// `get_changed_file_sources` in Python.
pub(crate) fn worktree_changes(repo_root: &Path) -> Vec<String> {
    git_raw(
        repo_root,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    )
    .map(|payload| parse_porcelain(&payload).worktree())
    .unwrap_or_default()
}

/// `get_changed_file_sources` for a git checkout: each list in first-seen
/// order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeSources {
    pub files: Vec<String>,
    pub base_diff: Vec<String>,
    pub worktree: Vec<String>,
    pub staged: Vec<String>,
    pub unstaged: Vec<String>,
    pub untracked: Vec<String>,
}

/// An exception Python lets escape while listing changes: its `str()`, and
/// whether `handle_tool_runtime_error` counts its type a tool runtime error
/// (`OSError`, `UnicodeDecodeError`) rather than an unexpected failure
/// (`JjWorkspaceError`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeError {
    pub message: String,
    pub runtime_error: bool,
}

impl ChangeError {
    fn runtime(message: String) -> Self {
        Self {
            message,
            runtime_error: true,
        }
    }
}

/// `get_changed_file_sources(repo_root, base)`, or what it raises: git
/// output that is not UTF-8, a jj working copy jj cannot read, a `git` or
/// `svn` that cannot be started. Outside a VCS git runs anyway, as in Python.
///
/// A ref Python rejects reports no files, as Python does after its warning.
pub fn change_file_sources(repo_root: &Path, base: &str) -> Result<ChangeSources, ChangeError> {
    let vcs = detect_vcs(repo_root);
    if vcs == Vcs::Svn {
        let files = svn::changed_files(repo_root, Some(base).filter(|b| svn::is_safe_svn_rev(b)))?;
        return Ok(ChangeSources {
            files: files.clone(),
            worktree: files.clone(),
            unstaged: files,
            ..ChangeSources::default()
        });
    }
    if !is_safe_git_ref(base) {
        return Ok(ChangeSources::default());
    }
    if vcs == Vcs::Jj {
        return jj_change_sources(repo_root, base);
    }
    let base_diff = match git_text(
        repo_root,
        &["diff", "--name-status", "-M", "-z", base, "HEAD", "--"],
    )? {
        Some((true, payload)) => parse_name_status(&payload),
        _ => Vec::new(),
    };
    let worktree = worktree_sources(repo_root)?;
    Ok(ChangeSources {
        files: dedupe(base_diff.iter().cloned().chain(worktree.worktree())),
        base_diff,
        worktree: worktree.worktree(),
        staged: dedupe(worktree.staged),
        unstaged: dedupe(worktree.unstaged),
        untracked: dedupe(worktree.untracked),
    })
}

/// `get_staged_and_unstaged`, or what it raises, as for
/// [`change_file_sources`].
pub fn staged_and_unstaged(repo_root: &Path) -> Result<Vec<String>, ChangeError> {
    match detect_vcs(repo_root) {
        Vcs::Svn => svn::changed_files(repo_root, None),
        Vcs::Jj => jj_change_sources(repo_root, "HEAD").map(|sources| sources.worktree),
        Vcs::Git | Vcs::None => Ok(worktree_sources(repo_root)?.worktree()),
    }
}

/// The base a review diffs against when the caller names none: `HEAD`
/// while a git checkout has staged or unstaged changes to tracked files (the
/// work in progress alone), `HEAD~1` otherwise (the last commit). Untracked
/// files alone keep `HEAD~1`; they are still listed as changed either way.
pub fn default_review_base(repo_root: &Path) -> &'static str {
    if detect_vcs(repo_root) != Vcs::Git {
        return "HEAD~1";
    }
    match worktree_sources(repo_root) {
        Ok(worktree) if !worktree.staged.is_empty() || !worktree.unstaged.is_empty() => "HEAD",
        _ => "HEAD~1",
    }
}

/// What `_git_diff_cache_stamp` raises before the diff is parsed: the
/// `git rev-parse HEAD` and `git status --porcelain` text it decodes as
/// UTF-8 (paths stay quoted unless `core.quotePath` is off). `None` in an
/// SVN working copy or a jj workspace, which read neither.
pub fn diff_stamp_error(repo_root: &Path) -> Option<ChangeError> {
    if repo_root.join(".svn").exists() || jj::is_jj_workspace(repo_root) {
        return None;
    }
    for args in [
        &["rev-parse", "HEAD"][..],
        &["status", "--porcelain", "--untracked-files=all"][..],
    ] {
        // `_stdout_or_empty` swallows an `OSError`, not a decoding error.
        if let Ok(output) = Command::new("git")
            .args(args)
            .current_dir(repo_root)
            .output()
            && let Some(message) =
                pyerr::utf8_error(&output.stdout).or_else(|| pyerr::utf8_error(&output.stderr))
        {
            return Some(ChangeError::runtime(message));
        }
    }
    None
}

/// `subprocess.run(["git", *args], capture_output=True, text=True)`:
/// `(succeeded, stdout)`, `None` when git is not installed (which Python's
/// callers catch), or what Python raises: another `OSError`, or stdout or
/// stderr that does not decode.
fn git_text(repo_root: &Path, args: &[&str]) -> Result<Option<(bool, String)>, ChangeError> {
    let output = match Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(ChangeError::runtime(pyerr::os_error(&err, "git"))),
    };
    if let Some(message) =
        pyerr::utf8_error(&output.stdout).or_else(|| pyerr::utf8_error(&output.stderr))
    {
        return Err(ChangeError::runtime(message));
    }
    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    Ok(Some((output.status.success(), stdout)))
}

/// `_jj_diff_files`: both sides of every rename between two commits.
fn jj_diff_files(repo_root: &Path, old: &str, new: &str) -> Vec<String> {
    jj::run_git(
        repo_root,
        &["diff", "--name-status", "-M", "-z", old, new, "--"],
    )
    .filter(|out| !out.is_empty())
    .map(|out| parse_name_status(&out))
    .unwrap_or_default()
}

/// `_get_jj_changed_file_sources`: `base..@-` plus `@-..@`, the working-copy
/// change reported as unstaged, or the `JjWorkspaceError` Python raises when
/// jj cannot read the working copy.
fn jj_change_sources(repo_root: &Path, base: &str) -> Result<ChangeSources, ChangeError> {
    let wc = jj::require_working_copy(repo_root).map_err(|message| ChangeError {
        message,
        runtime_error: false,
    })?;
    let base_diff = match jj::resolve_commit(repo_root, base, Some(&wc)) {
        Some(resolved) => jj_diff_files(repo_root, &resolved, &wc.parent),
        None => Vec::new(),
    };
    let worktree = jj_diff_files(repo_root, &wc.parent, &wc.commit);
    Ok(ChangeSources {
        files: dedupe(base_diff.iter().chain(&worktree).cloned()),
        base_diff,
        worktree: worktree.clone(),
        unstaged: worktree,
        ..ChangeSources::default()
    })
}

/// The `worktree` group of `get_changed_file_sources(repo_root, "HEAD")` in
/// a git checkout or jj workspace, as the freshness assessment reads it: any
/// failure is no dirtiness.
pub(crate) fn dirty_files(repo_root: &Path) -> Vec<String> {
    if jj::is_jj_workspace(repo_root) {
        jj::working_copy(repo_root)
            .map(|wc| jj_diff_files(repo_root, &wc.parent, &wc.commit))
            .unwrap_or_default()
    } else {
        // Output that is not UTF-8 raises in Python, which counts as clean.
        worktree_sources(repo_root)
            .map(|worktree| worktree.worktree())
            .unwrap_or_default()
    }
}

/// `git status --porcelain -z`; Python reads whatever it printed, even on
/// failure.
fn worktree_sources(repo_root: &Path) -> Result<Worktree, ChangeError> {
    Ok(
        match git_text(
            repo_root,
            &["status", "--porcelain", "-z", "--untracked-files=all"],
        )? {
            Some((_, payload)) => parse_porcelain(&payload),
            None => Worktree::default(),
        },
    )
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

#[derive(Default)]
struct Worktree {
    staged: Vec<String>,
    unstaged: Vec<String>,
    untracked: Vec<String>,
}

impl Worktree {
    /// Staged, then unstaged, then untracked, each path once.
    fn worktree(&self) -> Vec<String> {
        dedupe(
            self.staged
                .iter()
                .chain(&self.unstaged)
                .chain(&self.untracked)
                .cloned(),
        )
    }
}

/// `git status --porcelain -z`; a rename is two fields, new path first.
fn parse_porcelain(payload: &str) -> Worktree {
    let entries = nul_fields(payload);
    let mut changes = Worktree::default();
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
            changes.untracked.extend(paths);
            continue;
        }
        if x != b' ' {
            changes.staged.extend(paths.iter().cloned());
        }
        if y != b' ' {
            changes.unstaged.extend(paths);
        }
    }
    changes
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
        let changes = parse_porcelain(payload);
        assert_eq!(
            changes.worktree(),
            vec!["moved.py", "orig.py", "staged.py", "edited.py", "new.py"]
        );
        assert_eq!(changes.staged, vec!["moved.py", "orig.py", "staged.py"]);
        assert_eq!(changes.unstaged, vec!["edited.py"]);
        assert_eq!(changes.untracked, vec!["new.py"]);
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
