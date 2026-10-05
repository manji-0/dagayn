//! Port of `dagayn.jj_workspace`: a git-backed jj workspace without its own
//! `.git`.
//!
//! Git run inside such a workspace walks up to the enclosing checkout, so
//! every git-derived answer is taken from commit-to-commit commands against
//! the workspace's backing git directory instead: `@-` plays `HEAD`, `@-..@`
//! is the uncommitted change.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `_WC_TEMPLATE`, byte for byte.
const WC_TEMPLATE: &str = concat!(
    r#"commit_id ++ " " ++ parents.map(|c| c.commit_id()).join(",") ++ " ""#,
    r#" ++ local_bookmarks.map(|b| b.name()).join(",") ++ " ""#,
    r#" ++ parents.map(|c| c.local_bookmarks().map(|b| b.name()).join(",")).join(",")"#,
    r#" ++ "\n""#,
);

/// `_read_link`: a jj pointer file holding an absolute or relative path.
fn read_link(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let target = PathBuf::from(text);
    Some(if target.is_absolute() {
        target
    } else {
        path.parent().unwrap_or(Path::new("")).join(target)
    })
}

/// `jj_git_dir`: the git directory backing the jj workspace at `root`.
pub fn jj_git_dir(root: &Path) -> Option<PathBuf> {
    let jj_dir = root.join(".jj");
    if !jj_dir.is_dir() {
        return None;
    }
    let repo = jj_dir.join("repo");
    let repo_dir = if repo.is_file() {
        read_link(&repo)?
    } else if repo.is_dir() {
        repo
    } else {
        return None;
    };
    let git_dir = read_link(&repo_dir.join("store").join("git_target"))?;
    let git_dir = git_dir.canonicalize().ok()?;
    git_dir.is_dir().then_some(git_dir)
}

/// `is_jj_workspace`: a git-backed jj workspace root with no `.git`.
pub fn is_jj_workspace(root: &Path) -> bool {
    !root.join(".git").exists() && jj_git_dir(root).is_some()
}

/// `WorkingCopy`: commit ids of `@` and its first parent, and the nearest
/// bookmark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingCopy {
    pub commit: String,
    pub parent: String,
    pub bookmark: String,
}

/// `working_copy`: snapshot the workspace (`jj log` does) and read `@` and
/// `@-`; `None` when jj cannot (a stale workspace, jj missing).
pub fn working_copy(root: &Path) -> Option<WorkingCopy> {
    require_working_copy(root).ok()
}

/// `require_working_copy`: [`working_copy`], or the message of the
/// `JjWorkspaceError` Python raises when jj cannot read it.
pub fn require_working_copy(root: &Path) -> Result<WorkingCopy, String> {
    let output = Command::new("jj")
        .args(["--no-pager", "--color=never", "-R"])
        .arg(root)
        .args(["log", "--no-graph", "-r", "@", "-T", WC_TEMPLATE])
        .current_dir(root)
        .output();
    // `_run_capture`: stdout only on success; stderr, or the `OSError`.
    let (out, stderr) = match output {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let out = output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).into_owned());
            (out, stderr)
        }
        Err(err) => (None, crate::pyerr::os_error(&err, "jj")),
    };
    if let Some(wc) = out
        .filter(|out| !out.is_empty())
        .and_then(|out| parse_working_copy(&out))
    {
        return Ok(wc);
    }
    Err(working_copy_error(root, &stderr))
}

/// `JjWorkspaceError`'s message: jj's first line of complaint, and how to
/// recover a stale workspace.
fn working_copy_error(root: &Path, stderr: &str) -> String {
    let python_whitespace = |c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c);
    let reason = crate::svn::splitlines(stderr)
        .into_iter()
        .map(|line| line.trim_matches(python_whitespace))
        .find(|line| !line.is_empty())
        .map(|line| {
            line.strip_prefix("Error: ")
                .unwrap_or(line)
                .trim_end_matches('.')
                .to_string()
        })
        .unwrap_or_else(|| "jj printed no working copy".to_string());
    let hint = if stderr.to_lowercase().contains("stale") {
        " Run `jj workspace update-stale` in the workspace, then retry."
    } else {
        ""
    };
    format!(
        "jj could not read the working copy of {}: {reason}.{hint}",
        root.display()
    )
}

/// `_parse_working_copy`.
fn parse_working_copy(out: &str) -> Option<WorkingCopy> {
    let fields: Vec<&str> = out.trim_matches('\n').split(' ').collect();
    if fields.len() < 2 || fields[0].is_empty() {
        return None;
    }
    let parent = fields[1].split(',').find(|sha| !sha.is_empty())?;
    let names = |index: usize| -> Vec<&str> {
        fields
            .get(index)
            .copied()
            .unwrap_or("")
            .split(',')
            .filter(|name| !name.is_empty())
            .collect()
    };
    let (own, inherited) = (names(2), names(3));
    let bookmark = own.first().or(inherited.first()).copied().unwrap_or("");
    Some(WorkingCopy {
        commit: fields[0].to_string(),
        parent: parent.to_string(),
        bookmark: bookmark.to_string(),
    })
}

/// `git_argv(root, ...)` as a command: git bound to the workspace's git dir
/// and work tree, run from `root`.
pub fn git_command(root: &Path) -> Option<Command> {
    let git_dir = jj_git_dir(root)?;
    let mut command = Command::new("git");
    command
        .arg(format!("--git-dir={}", git_dir.display()))
        .arg(format!("--work-tree={}", root.display()))
        .current_dir(root);
    Some(command)
}

/// `run_git`: stdout (decoded with replacement) of a successful git command
/// against the workspace's repository.
pub fn run_git(root: &Path, args: &[&str]) -> Option<String> {
    let output = git_command(root)?.args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The `~`/`^` suffix of a ref `_HEAD_RELATIVE` accepts (`^HEAD((?:[~^]\d*)*)$`).
fn head_relative_suffix(reference: &str) -> Option<&str> {
    let reference = reference.strip_suffix('\n').unwrap_or(reference);
    let suffix = reference.strip_prefix("HEAD")?;
    let valid = suffix.is_empty()
        || (suffix.starts_with(['~', '^'])
            && suffix
                .chars()
                .all(|c| c == '~' || c == '^' || c.is_ascii_digit()));
    valid.then_some(suffix)
}

/// `resolve_commit`: a git-style `reference` inside the workspace as a full
/// sha; `HEAD`-relative refs are rebased onto `@-`.
pub fn resolve_commit(root: &Path, reference: &str, wc: Option<&WorkingCopy>) -> Option<String> {
    let rebased;
    let reference = match head_relative_suffix(reference) {
        Some(suffix) => {
            let owned;
            let wc = match wc {
                Some(wc) => wc,
                None => {
                    owned = working_copy(root)?;
                    &owned
                }
            };
            rebased = format!("{}{suffix}", wc.parent);
            rebased.as_str()
        }
        None => reference,
    };
    let out = run_git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ],
    )?;
    let sha = out.trim();
    (!sha.is_empty()).then(|| sha.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_matches_python() {
        assert_eq!(
            WC_TEMPLATE,
            "commit_id ++ \" \" ++ parents.map(|c| c.commit_id()).join(\",\") ++ \" \" ++ \
             local_bookmarks.map(|b| b.name()).join(\",\") ++ \" \" ++ \
             parents.map(|c| c.local_bookmarks().map(|b| b.name()).join(\",\")).join(\",\") \
             ++ \"\\n\""
        );
    }

    #[test]
    fn working_copy_fields_parse_as_python_does() {
        let wc = parse_working_copy("abc def,ghi  main\n").unwrap();
        assert_eq!(
            wc,
            WorkingCopy {
                commit: "abc".into(),
                parent: "def".into(),
                bookmark: "main".into(),
            }
        );
        let own = parse_working_copy("abc def feat,x main\n").unwrap();
        assert_eq!(own.bookmark, "feat");
        assert_eq!(parse_working_copy("abc def").unwrap().bookmark, "");
        assert!(parse_working_copy("abc  \n").is_none());
        assert!(parse_working_copy(" def\n").is_none());
        assert!(parse_working_copy("abc\n").is_none());
    }

    #[test]
    fn working_copy_errors_read_as_python_words_them() {
        let root = Path::new("/w");
        assert_eq!(
            working_copy_error(
                root,
                "\n  Error: The working copy is stale (not updated since operation abc).\nHint: x\n"
            ),
            "jj could not read the working copy of /w: The working copy is stale (not updated \
             since operation abc). Run `jj workspace update-stale` in the workspace, then retry."
        );
        assert_eq!(
            working_copy_error(root, ""),
            "jj could not read the working copy of /w: jj printed no working copy."
        );
    }

    #[test]
    fn head_relative_refs_follow_the_python_pattern() {
        assert_eq!(head_relative_suffix("HEAD"), Some(""));
        assert_eq!(head_relative_suffix("HEAD~1"), Some("~1"));
        assert_eq!(head_relative_suffix("HEAD^2~1"), Some("^2~1"));
        assert_eq!(head_relative_suffix("HEAD~"), Some("~"));
        assert_eq!(head_relative_suffix("HEAD\n"), Some(""));
        assert_eq!(head_relative_suffix("HEADS"), None);
        assert_eq!(head_relative_suffix("HEAD1"), None);
        assert_eq!(head_relative_suffix("main"), None);
    }

    #[test]
    fn git_dir_follows_the_repo_pointer() {
        let base = std::env::temp_dir().join(format!("dagayn-jj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let main = base.join("main");
        std::fs::create_dir_all(main.join(".git")).unwrap();
        std::fs::create_dir_all(main.join(".jj/repo/store")).unwrap();
        std::fs::write(main.join(".jj/repo/store/git_target"), "../../../.git").unwrap();
        let workspace = main.join(".worktrees/task");
        std::fs::create_dir_all(workspace.join(".jj")).unwrap();
        std::fs::write(workspace.join(".jj/repo"), "../../../.jj/repo").unwrap();
        let expected = main.join(".git").canonicalize().unwrap();
        assert_eq!(jj_git_dir(&workspace), Some(expected));
        assert!(is_jj_workspace(&workspace));
        // A colocated main checkout stays git.
        assert!(!is_jj_workspace(&main));
        let orphan = base.join("orphan");
        std::fs::create_dir_all(orphan.join(".jj")).unwrap();
        assert!(!is_jj_workspace(&orphan));
        let _ = std::fs::remove_dir_all(&base);
    }
}
