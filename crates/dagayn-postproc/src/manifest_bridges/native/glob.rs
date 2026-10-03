//! CMake `file(GLOB ...)`, matched the way Python's `pathlib` globs:
//! case-sensitive, wildcards match dotfiles, `*` stays within one path
//! segment, and `**` does not descend into symlinked directories.

use std::collections::BTreeSet;
use std::path::Path;

use super::super::text::abs;

pub(super) fn cmake_glob(repo_root: &Path, base: &str, mode: &str, args: &[String]) -> Vec<String> {
    let mut patterns: Vec<&str> = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        match arg.as_str() {
            "LIST_DIRECTORIES" | "RELATIVE" => skip_next = true,
            "CONFIGURE_DEPENDS" => {}
            _ => patterns.push(arg),
        }
    }
    let directory = abs(repo_root, base);
    let mut found = Vec::new();
    for pattern in patterns {
        let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
        if pattern.starts_with('/') || pattern.starts_with("..") || pattern.contains('$') {
            continue;
        }
        let matches = if mode == "GLOB_RECURSE" {
            // `rglob(leaf)` under the parent directory, a literal path even
            // when it holds wildcards.
            let (parent, leaf) = pattern.rsplit_once('/').unwrap_or(("", pattern));
            let prefix = parent
                .split('/')
                .filter(|part| !part.is_empty() && *part != ".")
                .collect::<Vec<_>>()
                .join("/");
            let leaf_pattern = format!("**/{leaf}");
            if prefix.is_empty() {
                glob(&directory, &leaf_pattern)
            } else {
                glob(&directory.join(&prefix), &leaf_pattern)
                    .into_iter()
                    .map(|rel| {
                        if rel.is_empty() {
                            prefix.clone()
                        } else {
                            format!("{prefix}/{rel}")
                        }
                    })
                    .collect()
            }
        } else {
            glob(&directory, pattern)
        };
        let mut matches: Vec<String> = matches
            .into_iter()
            .filter(|rel| directory.join(rel).is_file())
            .collect();
        // Python sorts `Path`s component by component.
        matches.sort_by(|a, b| a.split('/').cmp(b.split('/')));
        found.extend(matches);
    }
    found
}

/// Paths under `root` matching the relative glob `pattern`, relative to
/// `root`, without duplicates.
fn glob(root: &Path, pattern: &str) -> Vec<String> {
    let parts: Vec<&str> = pattern
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    // An empty pattern is an error to pathlib, and a trailing slash selects
    // directories only: neither yields a file.
    if parts.is_empty() || pattern.ends_with('/') {
        return Vec::new();
    }
    let mut found = BTreeSet::new();
    select(root, "", &parts, true, &mut found);
    found.into_iter().collect()
}

fn select(root: &Path, current: &str, parts: &[&str], exists: bool, found: &mut BTreeSet<String>) {
    let Some((&part, rest)) = parts.split_first() else {
        if exists || std::fs::symlink_metadata(abs(root, current)).is_ok() {
            found.insert(current.to_string());
        }
        return;
    };
    let join = |name: &str| {
        if current.is_empty() {
            name.to_string()
        } else {
            format!("{current}/{name}")
        }
    };
    if part == "**" {
        let mut rest = rest;
        while rest.first() == Some(&"**") {
            rest = &rest[1..];
        }
        let dir_only = !rest.is_empty();
        select(root, current, rest, exists, found);
        let mut stack = vec![current.to_string()];
        while let Some(directory) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(abs(root, &directory)) else {
                continue;
            };
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                let child = if directory.is_empty() {
                    name
                } else {
                    format!("{directory}/{name}")
                };
                if dir_only {
                    if is_dir {
                        select(root, &child, rest, true, found);
                    }
                } else {
                    found.insert(child.clone());
                }
                if is_dir {
                    stack.push(child);
                }
            }
        }
    } else if part == ".." || !part.contains(['*', '?', '[']) {
        select(root, &join(part), rest, false, found);
    } else {
        let Ok(entries) = std::fs::read_dir(abs(root, current)) else {
            return;
        };
        let pattern: Vec<char> = part.chars().collect();
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let name_chars: Vec<char> = name.chars().collect();
            if part != "*" && !fnmatch(&pattern, &name_chars) {
                continue;
            }
            let child = join(&name);
            if rest.is_empty() {
                found.insert(child);
            } else if std::fs::metadata(abs(root, &child)).is_ok_and(|meta| meta.is_dir()) {
                select(root, &child, rest, true, found);
            }
        }
    }
}

/// Case-sensitive `fnmatch` of one path segment: `*`, `?`, and `[...]`
/// sets (`!` negates, a leading `]` is literal, an unclosed `[` is literal).
fn fnmatch(pattern: &[char], name: &[char]) -> bool {
    let Some((&first, rest)) = pattern.split_first() else {
        return name.is_empty();
    };
    match first {
        '*' => (0..=name.len()).any(|skip| fnmatch(rest, &name[skip..])),
        '?' => !name.is_empty() && fnmatch(rest, &name[1..]),
        '[' => match bracket_set(rest) {
            Some((matcher, after)) => {
                !name.is_empty() && matcher(name[0]) && fnmatch(&rest[after..], &name[1..])
            }
            None => name.first() == Some(&'[') && fnmatch(rest, &name[1..]),
        },
        literal => name.first() == Some(&literal) && fnmatch(rest, &name[1..]),
    }
}

type SetMatcher = Box<dyn Fn(char) -> bool>;

/// The set opened by a `[` just before `body`, and the index in `body`
/// just past its closing `]`; `None` when it never closes.
fn bracket_set(body: &[char]) -> Option<(SetMatcher, usize)> {
    let mut index = 0;
    let negated = body.first() == Some(&'!');
    if negated {
        index += 1;
    }
    if body.get(index) == Some(&']') {
        index += 1;
    }
    while index < body.len() && body[index] != ']' {
        index += 1;
    }
    if index >= body.len() {
        return None;
    }
    let start = usize::from(negated);
    let items: Vec<char> = body[start..index].to_vec();
    let mut ranges: Vec<(char, char)> = Vec::new();
    let mut at = 0;
    while at < items.len() {
        if at + 2 < items.len() && items[at + 1] == '-' {
            ranges.push((items[at], items[at + 2]));
            at += 3;
        } else {
            ranges.push((items[at], items[at]));
            at += 1;
        }
    }
    let matcher = move |ch: char| {
        let inside = ranges.iter().any(|&(low, high)| low <= ch && ch <= high);
        if ranges.is_empty() {
            // `[!]` matches any character; `[]` never closes, handled above.
            negated
        } else {
            inside != negated
        }
    };
    Some((Box::new(matcher), index + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, name: &str) -> bool {
        let pattern: Vec<char> = pattern.chars().collect();
        let name: Vec<char> = name.chars().collect();
        fnmatch(&pattern, &name)
    }

    #[test]
    fn fnmatch_segments() {
        assert!(matches("*.c", "a.c"));
        assert!(matches("*.c", ".c"));
        assert!(!matches("*.c", "a.C"));
        assert!(matches("[a-c]?.cpp", "bx.cpp"));
        assert!(!matches("[!a-c]x", "ax"));
        assert!(matches("[]]x", "]x"));
        assert!(matches("[x", "[x"));
    }
}
