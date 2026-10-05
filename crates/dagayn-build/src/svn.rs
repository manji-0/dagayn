//! SVN working copies, as `dagayn.incremental_files` and `dagayn.changes`
//! read them: `svn status`, `svn diff --summarize`, `svn diff`, and
//! `svn info`.
//!
//! The parsers are pure functions over what the commands print (decoded as
//! Python does, UTF-8 with replacement); the runners only add the command and
//! Python's failure handling.

use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Output};

use crate::vcs::ChangeError;

/// `_SAFE_SVN_REV`: `^r?\d+(:r?\d+|:HEAD|:BASE|:COMMITTED)?$`, ignoring case.
/// Like Python's `$`, a single trailing newline is accepted.
pub fn is_safe_svn_rev(rev: &str) -> bool {
    let rev = rev.strip_suffix('\n').unwrap_or(rev);
    let number = |text: &str| -> Option<usize> {
        let text_start = usize::from(text.starts_with(['r', 'R']));
        let digits = text[text_start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        (digits > 0).then_some(text_start + digits)
    };
    let Some(end) = number(rev) else {
        return false;
    };
    let rest = &rev[end..];
    let Some(range) = rest.strip_prefix(':') else {
        return rest.is_empty();
    };
    ["HEAD", "BASE", "COMMITTED"]
        .iter()
        .any(|keyword| range.eq_ignore_ascii_case(keyword))
        || number(range).is_some_and(|end| end == range.len())
}

/// Python's `str.splitlines()`.
pub(crate) fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let breaks = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        lines.push(&text[start..index]);
        let mut next = index + c.len_utf8();
        if c == '\r'
            && let Some(&(_, '\n')) = chars.peek()
        {
            chars.next();
            next += 1;
        }
        start = next;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Python's `str.strip()` without arguments.
fn py_strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// The characters of `line` from `from` on (`line[from:]`).
fn tail(line: &str, from: usize) -> &str {
    line.char_indices()
        .nth(from)
        .map_or("", |(index, _)| &line[index..])
}

/// `svn status` (the `_get_svn_changed_files` branch without a range): one
/// path per modified, added, deleted, replaced, or conflicted entry.
pub fn parse_status(stdout: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in splitlines(stdout) {
        let length = line.chars().count();
        if length < 2 {
            continue;
        }
        if !line.starts_with(['M', 'A', 'D', 'R', 'C']) {
            continue;
        }
        let path = if length > 8 {
            tail(line, 8)
        } else {
            tail(line, 1)
        };
        files.push(py_strip(path).to_string());
    }
    files
}

/// `svn diff --summarize`: one path per modified, added, or deleted entry.
pub fn parse_summarize(stdout: &str) -> Vec<String> {
    splitlines(stdout)
        .into_iter()
        .filter(|line| line.chars().count() >= 2 && line.starts_with(['M', 'A', 'D']))
        .map(|line| py_strip(tail(line, 1)).to_string())
        .collect()
}

/// `_svn_revision_info` over `svn info` output: `(branch path, revision)`.
pub fn parse_info(stdout: &str) -> (String, String) {
    let mut branch = String::new();
    let mut revision = String::new();
    for line in splitlines(stdout) {
        if let Some(url) = line.strip_prefix("URL: ") {
            let url = py_strip(url);
            for marker in ["/branches/", "/tags/", "/trunk"] {
                if let Some(index) = url.find(marker) {
                    branch = url[index..].trim_start_matches('/').to_string();
                    break;
                }
            }
            if branch.is_empty() && !url.is_empty() {
                branch = url
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string();
            }
        } else if let Some(rev) = line.strip_prefix("Revision: ") {
            revision = py_strip(rev).to_string();
        }
    }
    (branch, revision)
}

/// Run `svn` in `root`. `Err(())` where Python's `subprocess.run` would
/// raise past its `FileNotFoundError` handler; `Ok(None)` for a missing
/// binary, which Python reports as no changes.
/// `svn args`; `None` when svn is not installed (which Python catches), or
/// the `OSError` Python lets escape.
fn run(root: &Path, args: &[&str]) -> Result<Option<Output>, ChangeError> {
    match Command::new("svn").args(args).current_dir(root).output() {
        Ok(output) => Ok(Some(output)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ChangeError {
            message: crate::pyerr::os_error(&err, "svn"),
            runtime_error: true,
        }),
    }
}

/// `_get_svn_changed_files(root, rev_range)`, or what it raises.
pub fn changed_files(root: &Path, rev_range: Option<&str>) -> Result<Vec<String>, ChangeError> {
    let Some(rev_range) = rev_range.filter(|rev| !rev.is_empty()) else {
        let Some(output) = run(root, &["status", "--non-interactive"])? else {
            return Ok(Vec::new());
        };
        // Python reads whatever `svn status` printed, whatever its status.
        return Ok(parse_status(&String::from_utf8_lossy(&output.stdout)));
    };
    let Some(output) = run(
        root,
        &["diff", "--summarize", "--non-interactive", "-r", rev_range],
    )?
    else {
        return Ok(Vec::new());
    };
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_summarize(&String::from_utf8_lossy(&output.stdout)))
}

/// What `parse_svn_diff_ranges` hands `_parse_unified_diff`: the stdout of
/// `svn diff --non-interactive [-r rev_range]`, or empty text where Python
/// returns no ranges (an unsafe range, a failed or missing `svn`).
pub fn diff_text(root: &Path, rev_range: Option<&str>) -> String {
    let mut args = vec!["diff", "--non-interactive"];
    if let Some(rev_range) = rev_range.filter(|rev| !rev.is_empty()) {
        if !is_safe_svn_rev(rev_range) {
            return String::new();
        }
        args.extend(["-r", rev_range]);
    }
    match Command::new("svn").args(&args).current_dir(root).output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values below were produced by running the Python parsers
    // (`_get_svn_changed_files`, `_svn_revision_info` with `subprocess.run`
    // returning this text, `_SAFE_SVN_REV`, `str.splitlines`) on the same
    // input; tests/test_mcp_vcs_parity.py checks the same recordings end to end.

    const STATUS: &str = "M       app.py\n\
                          ?       scratch.txt\n\
                          A  +    pkg/new.py\n\
                          D       gone.py\n\
                          !       missing.py\n\
                          R       swapped.py\n\
                          C       conflict.py\n \
                          M      props_only.py\n\
                          X\n\
                          M\n\
                          Mshort\n\
                          A       spaced name.py  \n\
                          Performing status on external item at 'ext':\n\
                          M       ü/ñ.py\r\n";

    #[test]
    fn status_follows_python() {
        assert_eq!(
            parse_status(STATUS),
            vec![
                "app.py",
                "pkg/new.py",
                "gone.py",
                "swapped.py",
                "conflict.py",
                "short",
                "spaced name.py",
                "ü/ñ.py",
            ]
        );
    }

    #[test]
    fn summarize_follows_python() {
        let out = "M       app.py\nA       pkg/new.py\nD       gone.py\n M      props.py\nMM      both.py\nR       x.py\nA\n";
        assert_eq!(
            parse_summarize(out),
            vec!["app.py", "pkg/new.py", "gone.py", "M      both.py"]
        );
    }

    #[test]
    fn info_follows_python() {
        let out = "Path: .\nWorking Copy Root Path: /wc\nURL: https://svn.example.com/repo/branches/feature-x\nRelative URL: ^/branches/feature-x\nRevision: 1234\nNode Kind: directory\n";
        assert_eq!(
            parse_info(out),
            ("branches/feature-x".to_string(), "1234".to_string())
        );
        assert_eq!(
            parse_info("URL: https://svn.example.com/repo/trunk\nRevision: 7\n"),
            ("trunk".to_string(), "7".to_string())
        );
        assert_eq!(
            parse_info("URL: file:///srv/svn/project/\n"),
            ("project".to_string(), String::new())
        );
        assert_eq!(
            parse_info("URL: https://x/repo/tags/v1.0/sub\n"),
            ("tags/v1.0/sub".to_string(), String::new())
        );
    }

    #[test]
    fn revision_ranges_follow_python() {
        for ok in ["r100:HEAD", "100", "R5:r9", "1:base", "12:Committed", "7\n"] {
            assert!(is_safe_svn_rev(ok), "{ok}");
        }
        for bad in [
            "",
            "HEAD~1",
            "r",
            "r1:",
            "1:2:3",
            "1:WORKING",
            "-1",
            "1 ",
            "1\n\n",
        ] {
            assert!(!is_safe_svn_rev(bad), "{bad:?}");
        }
    }

    #[test]
    fn splitlines_follows_python() {
        assert_eq!(splitlines("a\r\nb\rc\n\nd"), vec!["a", "b", "c", "", "d"]);
        assert_eq!(splitlines("a\x0bb\u{2028}c\n"), vec!["a", "b", "c"]);
        assert!(splitlines("").is_empty());
    }
}
