//! CMake `add_library` targets.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::{Captures, Regex};

use super::super::text::{is_py_space, parent_dir, read_text, splitlines};
use super::{NativeLibrary, existing_sources, glob};

static CMAKE_VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([A-Za-z0-9_.+-]+)\}").unwrap());
static CMAKE_WHOLE_VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A\$\{([A-Za-z0-9_.+-]+)\}\z").unwrap());
const CMAKE_LIBRARY_TYPES: &[&str] = &[
    "STATIC",
    "SHARED",
    "MODULE",
    "OBJECT",
    "INTERFACE",
    "UNKNOWN",
];
const CMAKE_SOURCE_KEYWORDS: &[&str] = &["INTERFACE", "PUBLIC", "PRIVATE", "EXCLUDE_FROM_ALL"];
const CMAKE_TRUE: &[&str] = &["ON", "YES", "TRUE", "Y", "1"];

/// Drop `#[==[ ... ]==]` bracket comments and `#` line comments outside
/// double quotes.
fn strip_comments(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut kept = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '#' && chars.get(index + 1) == Some(&'[') {
            let mut open = index + 2;
            while chars.get(open) == Some(&'=') {
                open += 1;
            }
            if chars.get(open) == Some(&'[') {
                let level = open - (index + 2);
                if let Some(end) = bracket_close(&chars, open + 1, level) {
                    index = end;
                    continue;
                }
            }
        }
        kept.push(chars[index]);
        index += 1;
    }
    let lines: Vec<String> = splitlines(&kept)
        .into_iter()
        .map(|line| {
            let line: Vec<char> = line.chars().collect();
            let mut quoted = false;
            for (index, &ch) in line.iter().enumerate() {
                if ch == '"' && (index == 0 || line[index - 1] != '\\') {
                    quoted = !quoted;
                } else if ch == '#' && !quoted {
                    return line[..index].iter().collect();
                }
            }
            line.iter().collect()
        })
        .collect();
    lines.join("\n")
}

/// The index just past the first `]`, `level` `=`s, `]` at or after `from`.
fn bracket_close(chars: &[char], from: usize, level: usize) -> Option<usize> {
    (from..chars.len()).find_map(|start| {
        let end = start + level + 2;
        let closes = end <= chars.len()
            && chars[start] == ']'
            && chars[start + 1..start + 1 + level]
                .iter()
                .all(|&ch| ch == '=')
            && chars[start + 1 + level] == ']';
        closes.then_some(end)
    })
}

type CMakeArgs = Vec<(String, bool)>;

/// `(command, [(argument, quoted), ...])` in source order.
fn cmake_commands(text: &str) -> Vec<(String, CMakeArgs)> {
    let chars: Vec<char> = strip_comments(text).chars().collect();
    let is_name_start = |ch: char| ch.is_ascii_alphabetic() || ch == '_';
    let is_name_char = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let mut commands = Vec::new();
    let mut pos = 0;
    loop {
        let Some(start) = (pos..chars.len()).find(|&index| is_name_start(chars[index])) else {
            return commands;
        };
        let mut name_end = start + 1;
        while name_end < chars.len() && is_name_char(chars[name_end]) {
            name_end += 1;
        }
        let mut after = name_end;
        while after < chars.len() && matches!(chars[after], ' ' | '\t') {
            after += 1;
        }
        if after >= chars.len() || chars[after] != '(' {
            pos = name_end;
            continue;
        }
        let mut args = Vec::new();
        let (mut depth, mut index, mut current) = (1, after + 1, String::new());
        while index < chars.len() && depth != 0 {
            let ch = chars[index];
            if ch == '"' {
                let mut end = index + 1;
                while end < chars.len() && chars[end] != '"' {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                let value: String = chars[index + 1..end.min(chars.len())].iter().collect();
                args.push((value, true));
                index = end + 1;
                continue;
            }
            if ch == '(' {
                depth += 1;
            } else if ch == ')' {
                depth -= 1;
            }
            if is_py_space(ch) || ch == '(' || ch == ')' {
                if !current.is_empty() {
                    args.push((std::mem::take(&mut current), false));
                }
            } else {
                current.push(ch);
            }
            index += 1;
        }
        let name: String = chars[start..name_end].iter().collect();
        commands.push((name.to_lowercase(), args));
        pos = index;
    }
}

struct CMakeTarget {
    name: String,
    shared: bool,
    output_name: Option<String>,
    items: Vec<String>,
}

pub(super) fn cmake_libraries(repo_root: &Path, cmake_rel: &str) -> Vec<NativeLibrary> {
    let Some(content) = read_text(&repo_root.join(cmake_rel)) else {
        return Vec::new();
    };
    let base = parent_dir(cmake_rel);
    let commands = cmake_commands(&content);
    let mut variables: HashMap<String, Vec<String>> = HashMap::from([
        (
            "CMAKE_CURRENT_SOURCE_DIR".to_string(),
            vec![".".to_string()],
        ),
        ("CMAKE_CURRENT_LIST_DIR".to_string(), vec![".".to_string()]),
    ]);
    if commands.iter().any(|(name, _)| name == "project") {
        variables.insert("PROJECT_SOURCE_DIR".to_string(), vec![".".to_string()]);
    }
    if base.is_empty() {
        variables.insert("CMAKE_SOURCE_DIR".to_string(), vec![".".to_string()]);
    }

    let mut shared_default = false;
    let mut targets: Vec<CMakeTarget> = Vec::new();
    for (command, raw) in &commands {
        if raw.is_empty() {
            continue;
        }
        let args: Vec<String> = raw
            .iter()
            .flat_map(|(arg, quoted)| expand(&variables, arg, *quoted))
            .collect();
        if args.is_empty() {
            continue;
        }
        let target_index = |name: &str, targets: &[CMakeTarget]| {
            targets.iter().position(|target| target.name == name)
        };
        match command.as_str() {
            "set" => {
                let mut rest = args[1..].to_vec();
                for stop in ["CACHE", "PARENT_SCOPE"] {
                    if let Some(at) = rest.iter().position(|arg| arg == stop) {
                        rest.truncate(at);
                    }
                }
                if args[0] == "BUILD_SHARED_LIBS" {
                    shared_default = rest
                        .first()
                        .is_some_and(|value| CMAKE_TRUE.contains(&value.to_uppercase().as_str()));
                }
                variables.insert(args[0].clone(), rest);
            }
            "option" if args[0] == "BUILD_SHARED_LIBS" => {
                shared_default = args
                    .get(2)
                    .is_some_and(|value| CMAKE_TRUE.contains(&value.to_uppercase().as_str()));
            }
            "list" if args.len() >= 2 && args[0] == "APPEND" => {
                variables
                    .entry(args[1].clone())
                    .or_default()
                    .extend(args[2..].iter().cloned());
            }
            "file" if args.len() >= 3 && (args[0] == "GLOB" || args[0] == "GLOB_RECURSE") => {
                let found = glob::cmake_glob(repo_root, &base, &args[0], &args[2..]);
                variables.insert(args[1].clone(), found);
            }
            "add_library" => {
                let name = &args[0];
                let mut rest = &args[1..];
                if rest
                    .first()
                    .is_some_and(|first| first == "ALIAS" || first == "IMPORTED")
                {
                    continue;
                }
                let kind = rest
                    .first()
                    .filter(|first| CMAKE_LIBRARY_TYPES.contains(&first.as_str()))
                    .cloned();
                if kind.is_some() {
                    rest = &rest[1..];
                }
                let shared = match kind.as_deref() {
                    Some(kind) => kind == "SHARED" || kind == "MODULE",
                    None => shared_default,
                };
                let at = target_index(name, &targets).unwrap_or_else(|| {
                    targets.push(CMakeTarget {
                        name: name.clone(),
                        shared,
                        output_name: None,
                        items: Vec::new(),
                    });
                    targets.len() - 1
                });
                let target = &mut targets[at];
                target.shared = shared;
                target.items.extend(
                    rest.iter()
                        .filter(|item| !CMAKE_SOURCE_KEYWORDS.contains(&item.as_str()))
                        .cloned(),
                );
            }
            "target_sources" => {
                if let Some(at) = target_index(&args[0], &targets) {
                    targets[at].items.extend(
                        args[1..]
                            .iter()
                            .filter(|item| !CMAKE_SOURCE_KEYWORDS.contains(&item.as_str()))
                            .cloned(),
                    );
                }
            }
            "set_target_properties" => {
                let Some(split) = args.iter().position(|arg| arg == "PROPERTIES") else {
                    continue;
                };
                for [key, value] in args[split + 1..].as_chunks::<2>().0 {
                    if key != "OUTPUT_NAME" && key != "LIBRARY_OUTPUT_NAME" {
                        continue;
                    }
                    for name in &args[..split] {
                        if let Some(at) = target_index(name, &targets) {
                            targets[at].output_name = Some(value.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let mut libraries = Vec::new();
    for target in targets.iter().filter(|target| target.shared) {
        let sources = existing_sources(repo_root, &base, &target.items);
        if sources.is_empty() {
            continue;
        }
        let name = target
            .output_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&target.name)
            .replace('-', "_");
        libraries.push(NativeLibrary::new(cmake_rel, "cmake", name, sources));
    }
    libraries
}

/// One argument with `${VAR}` references expanded: an unquoted lone
/// reference becomes the variable's list, otherwise lists join with `;`
/// and unquoted values split on `;` again.
fn expand(variables: &HashMap<String, Vec<String>>, arg: &str, quoted: bool) -> Vec<String> {
    if !quoted && let Some(whole) = CMAKE_WHOLE_VAR_RE.captures(arg) {
        return variables
            .get(&whole[1])
            .cloned()
            .unwrap_or_else(|| vec![arg.to_string()]);
    }
    let value = CMAKE_VAR_RE.replace_all(arg, |found: &Captures| match variables.get(&found[1]) {
        Some(values) => values.join(";"),
        None => found[0].to_string(),
    });
    if quoted {
        return vec![value.into_owned()];
    }
    value
        .split(';')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_stripped_outside_quotes() {
        let text = "add_library(x SHARED a.c) # é tail\n#[==[ block ]] still ]==]set(\"#keep\")";
        assert_eq!(
            strip_comments(text),
            "add_library(x SHARED a.c) \nset(\"#keep\")"
        );
    }

    #[test]
    fn commands_split_quoted_and_bare_arguments() {
        let commands = cmake_commands("ADD_LIBRARY(foo SHARED \"a b.c\" c.c)\nfoo bar");
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "add_library");
        assert_eq!(
            commands[0].1,
            vec![
                ("foo".to_string(), false),
                ("SHARED".to_string(), false),
                ("a b.c".to_string(), true),
                ("c.c".to_string(), false),
            ]
        );
    }
}
