//! Meson `shared_library` / `shared_module` / `both_libraries` / `library`.
//!
//! The scanners work on bytes: every delimiter they look for is ASCII, so a
//! multi-byte character is only ever skipped over, never split.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::super::text::{lossy_slice, parent_dir, read_text, splitlines};
use super::{NativeLibrary, existing_sources};

static MESON_ASSIGN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*([A-Za-z_]\w*)\s*(\+?=)\s*").unwrap());
static MESON_LIBRARY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(shared_library|shared_module|both_libraries|library)\s*\(").unwrap()
});
static STATIC_DEFAULT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"default_library\s*=\s*static").unwrap());
static IDENTIFIER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Za-z_]\w*\b").unwrap());
static KEYWORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A\s*[A-Za-z_]\w*\s*\z").unwrap());

/// The end of the `'...'` string literal opening at `start`
/// (`'((?:[^'\\]|\\.)*)'`), if it closes.
fn string_end(text: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    while index < text.len() {
        match text[index] {
            b'\'' => return Some(index + 1),
            b'\\' if text.get(index + 1).is_some_and(|&next| next != b'\n') => index += 2,
            b'\\' => return None,
            _ => index += 1,
        }
    }
    None
}

/// Index just past the bracket expression opening at `start`.
fn balanced(text: &[u8], start: usize) -> usize {
    let mut depth = 0;
    let mut index = start;
    while index < text.len() {
        let byte = text[index];
        if byte == b'\'' {
            index = string_end(text, index).unwrap_or(index + 1);
            continue;
        }
        if b"([{".contains(&byte) {
            depth += 1;
        } else if b")]}".contains(&byte) {
            depth -= 1;
            if depth == 0 {
                return index + 1;
            }
        }
        index += 1;
    }
    index
}

fn split_args(text: &[u8]) -> Vec<String> {
    let mut parts = Vec::new();
    let (mut depth, mut current, mut index) = (0, Vec::new(), 0);
    while index < text.len() {
        let byte = text[index];
        if byte == b'\'' {
            let end = string_end(text, index).unwrap_or(index + 1);
            current.extend_from_slice(&text[index..end]);
            index = end;
            continue;
        }
        if b"([{".contains(&byte) {
            depth += 1;
        } else if b")]}".contains(&byte) {
            depth -= 1;
        }
        if byte == b',' && depth == 0 {
            parts.push(lossy_slice(&current, 0, current.len()).trim().to_string());
            current.clear();
        } else {
            current.push(byte);
        }
        index += 1;
    }
    let last = lossy_slice(&current, 0, current.len());
    if !last.trim().is_empty() {
        parts.push(last.trim().to_string());
    }
    parts
}

/// String literals in `expr`, with bare identifiers replaced by their lists.
fn values(expr: &str, variables: &HashMap<String, Vec<String>>) -> Vec<String> {
    let bytes = expr.as_bytes();
    let mut found = Vec::new();
    let mut stripped = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\''
            && let Some(end) = string_end(bytes, index)
        {
            found.push(lossy_slice(bytes, index + 1, end - 1));
            stripped.push(b' ');
            index = end;
            continue;
        }
        stripped.push(bytes[index]);
        index += 1;
    }
    let stripped = String::from_utf8_lossy(&stripped);
    for name in IDENTIFIER_RE.find_iter(&stripped) {
        if let Some(list) = variables.get(name.as_str()) {
            found.extend(list.iter().cloned());
        }
    }
    found
}

/// End of the right-hand side of an assignment starting at `start`.
fn expression_end(text: &[u8], start: usize) -> usize {
    let mut index = start;
    while index < text.len() {
        match text[index] {
            b'(' | b'[' | b'{' => index = balanced(text, index),
            b'\'' => index = string_end(text, index).unwrap_or(index + 1),
            b'\n' => return index,
            _ => index += 1,
        }
    }
    index
}

enum Event<'a> {
    Assign {
        name: &'a str,
        append: bool,
        end: usize,
    },
    Library {
        function: &'a str,
        end: usize,
    },
}

pub(super) fn meson_libraries(repo_root: &Path, meson_rel: &str) -> Vec<NativeLibrary> {
    let Some(content) = read_text(&repo_root.join(meson_rel)) else {
        return Vec::new();
    };
    let text: String = splitlines(&content)
        .into_iter()
        .map(|line| line.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let bytes = text.as_bytes();
    let base = parent_dir(meson_rel);
    let library_is_shared = !STATIC_DEFAULT_RE.is_match(&text);
    let mut variables: HashMap<String, Vec<String>> = HashMap::new();
    let mut libraries = Vec::new();

    let mut events: Vec<(usize, Event)> = MESON_ASSIGN_RE
        .captures_iter(&text)
        .map(|found| {
            let whole = found.get(0).unwrap();
            let event = Event::Assign {
                name: found.get(1).unwrap().as_str(),
                append: &found[2] == "+=",
                end: whole.end(),
            };
            (whole.start(), event)
        })
        .collect();
    events.extend(MESON_LIBRARY_RE.captures_iter(&text).map(|found| {
        let whole = found.get(0).unwrap();
        let event = Event::Library {
            function: found.get(1).unwrap().as_str(),
            end: whole.end(),
        };
        (whole.start(), event)
    }));
    events.sort_by_key(|(start, _)| *start);

    for (_, event) in events {
        match event {
            Event::Assign { name, append, end } => {
                let stop = expression_end(bytes, end);
                let found = values(&lossy_slice(bytes, end, stop), &variables);
                if append {
                    variables.entry(name.to_string()).or_default().extend(found);
                } else {
                    variables.insert(name.to_string(), found);
                }
            }
            Event::Library { function, end } => {
                if function == "library" && !library_is_shared {
                    continue;
                }
                let close = balanced(bytes, end - 1);
                let args = split_args(lossy_slice(bytes, end, close.saturating_sub(1)).as_bytes());
                let Some(first) = args.first() else {
                    continue;
                };
                let first_bytes = first.as_bytes();
                if first_bytes.first() != Some(&b'\'')
                    || string_end(first_bytes, 0) != Some(first_bytes.len())
                {
                    continue;
                }
                let name = lossy_slice(first_bytes, 1, first_bytes.len() - 1);
                let mut items = Vec::new();
                for arg in &args[1..] {
                    if let Some((key, value)) = arg.split_once(':')
                        && KEYWORD_RE.is_match(key)
                    {
                        if key.trim() == "sources" {
                            items.extend(values(value, &variables));
                        }
                        continue;
                    }
                    items.extend(values(arg, &variables));
                }
                let sources = existing_sources(repo_root, &base, &items);
                if !sources.is_empty() {
                    libraries.push(NativeLibrary::new(
                        meson_rel,
                        "meson",
                        name.replace('-', "_"),
                        sources,
                    ));
                }
            }
        }
    }
    libraries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_reads_strings_then_variables() {
        let variables = HashMap::from([("srcs".to_string(), vec!["a.c".to_string()])]);
        assert_eq!(
            values("['b.c', 'it\\'s'] + srcs", &variables),
            ["b.c", "it\\'s", "a.c"]
        );
    }

    #[test]
    fn split_args_respects_nesting_and_strings() {
        assert_eq!(
            split_args(b"'foo', ['a.c', 'b,c'], install: true"),
            ["'foo'", "['a.c', 'b,c']", "install: true"]
        );
    }
}
