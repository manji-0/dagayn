//! Text and path helpers that reproduce the Python behavior the manifest
//! scanners were written against (`str.splitlines`, text-mode reads,
//! `PurePosixPath` joins).

use std::path::{Path, PathBuf};

/// A file read the way Python's `Path.read_text(errors="replace")` reads it:
/// invalid UTF-8 replaced, and `\r\n` / `\r` translated to `\n`.
pub(super) fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(universal_newlines(&String::from_utf8_lossy(&bytes)))
}

/// A strict UTF-8 read with newline translation, as `read_text(encoding="utf-8")`.
pub(super) fn read_text_strict(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(universal_newlines(std::str::from_utf8(&bytes).ok()?))
}

fn universal_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `str.splitlines()`: every Unicode line boundary, no trailing empty line.
pub(super) fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let boundary = matches!(
            ch,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !boundary {
            continue;
        }
        lines.push(&text[start..index]);
        start = index + ch.len_utf8();
        if ch == '\r'
            && let Some((_, '\n')) = chars.peek()
        {
            chars.next();
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// `str.isspace()` for one character.
pub(super) fn is_py_space(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}')
}

/// `bytes[start..end]` as text, without panicking on a cut through a
/// multi-byte character (Python slices code points, so the scanners that
/// slice at an ASCII delimiter never cut one; this only guards the rest).
pub(super) fn lossy_slice(bytes: &[u8], start: usize, end: usize) -> String {
    let end = end.min(bytes.len());
    let start = start.min(end);
    String::from_utf8_lossy(&bytes[start..end]).into_owned()
}

/// The repo-relative parent directory of `rel`, `""` for the repository root
/// (Python's `PurePosixPath(rel).parent` with `.` spelled `""`).
pub(super) fn parent_dir(rel: &str) -> String {
    rel.trim_end_matches('/')
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

/// `dir/name`, or `name` at the repository root.
pub(super) fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// `repo_root / rel`, with `""` naming the root itself.
pub(super) fn abs(repo_root: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        repo_root.to_path_buf()
    } else {
        repo_root.join(rel)
    }
}

pub(super) fn is_file(repo_root: &Path, rel: &str) -> bool {
    abs(repo_root, rel).is_file()
}

/// Resolve `declared` against `base_dir` as a repo-root-relative path.
///
/// Absolute inputs are treated as repo-root-relative by stripping the leading
/// slash. Returns `None` when lexical normalization would escape the
/// repository root via `..` (path traversal), or leaves nothing.
pub fn resolve_rel(base_dir: &str, declared: &str) -> Option<String> {
    let raw = declared.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = if raw.starts_with('/') || raw.starts_with('\\') {
        raw.trim_start_matches(['/', '\\']).to_string()
    } else if base_dir.is_empty() || base_dir == "." {
        raw.to_string()
    } else {
        format!("{base_dir}/{raw}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in candidate.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Join `rel_path` under `repo_root`, rejecting escapes after symlinks are
/// resolved. `None` also when the path does not exist: every caller goes on
/// to require an existing file or directory.
pub(super) fn contained_path(repo_root: &Path, rel_path: &str) -> Option<PathBuf> {
    if rel_path.is_empty() || rel_path.starts_with('/') || rel_path.starts_with('\\') {
        return None;
    }
    // Lexical rejection before touching the filesystem.
    resolve_rel("", rel_path)?;
    let root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let candidate = root.join(rel_path).canonicalize().ok()?;
    candidate.starts_with(&root).then_some(candidate)
}

/// Python's `PurePath.suffix`: the final dot part of the name, empty for
/// dotfiles and names without one.
pub(super) fn py_suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[index..],
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(
            splitlines("a\r\nb\rc\n\nd\u{0c}e\n"),
            ["a", "b", "c", "", "d", "e"]
        );
        assert!(splitlines("").is_empty());
    }

    #[test]
    fn resolve_rel_contains_paths() {
        assert_eq!(resolve_rel("pkg", "../../../etc/passwd"), None);
        assert_eq!(resolve_rel("pkg/sub", "../../.."), None);
        assert_eq!(resolve_rel("", "../outside.toml"), None);
        assert_eq!(resolve_rel("pkg", "/../../etc/passwd"), None);
        assert_eq!(
            resolve_rel("pkg/sub", "../Cargo.toml").as_deref(),
            Some("pkg/Cargo.toml")
        );
        assert_eq!(
            resolve_rel(".", "rust/Cargo.toml").as_deref(),
            Some("rust/Cargo.toml")
        );
        assert_eq!(
            resolve_rel("pkg", "/rust/Cargo.toml").as_deref(),
            Some("rust/Cargo.toml")
        );
        assert_eq!(resolve_rel("", "./"), None);
    }

    #[test]
    fn suffix_ignores_dotfiles() {
        assert_eq!(py_suffix("main.go"), ".go");
        assert_eq!(py_suffix(".go"), "");
        assert_eq!(py_suffix("..go"), ".go");
    }
}
