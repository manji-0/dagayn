//! Compiler command lines (Makefile recipes, justfiles, package.json scripts).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::{Captures, Regex};

use super::super::command::{command_options, split_command};
use super::super::text::{is_file, is_py_space, parent_dir, resolve_rel, splitlines};
use super::{C_SOURCE_SUFFIXES, NativeLibrary, SHARED_OUTPUT_RE, existing_sources, library_stem};

static COMPILER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A(?:[\w.-]+-)?(?:gcc|g\+\+|clang|clang\+\+|cc|c\+\+)(?:-\d+(?:\.\d+)*)?\z")
        .unwrap()
});
static SEGMENT_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&&|\|\||;|\|").unwrap());
static EXPORT_NAME_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z_$][\w$]*").unwrap());
static MAKE_VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$[({]([A-Za-z_][A-Za-z0-9_]*)[)}]").unwrap());
static MAKE_ASSIGN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A([A-Za-z_][A-Za-z0-9_]*)\s*(\?=|:=|::=|\+=|=)\s*(.*)\z").unwrap()
});
const SHARED_FLAGS: &[&str] = &["-shared", "-dynamiclib", "-bundle"];
const VALUED_FLAGS: &[&str] = &[
    "-o",
    "-I",
    "-L",
    "-D",
    "-U",
    "-include",
    "-isystem",
    "-MF",
    "-MT",
    "-MQ",
    "-x",
    "-arch",
    "-framework",
    "-install_name",
    "-target",
];
const EMSCRIPTEN_GLUE_SUFFIXES: &[&str] = &[".js", ".mjs", ".cjs", ".html"];

fn make_default(name: &str) -> Option<&'static str> {
    match name {
        "CC" => Some("cc"),
        "CXX" => Some("c++"),
        _ => None,
    }
}

fn command_library(
    repo_root: &Path,
    command_file: &str,
    cwd: &str,
    segment: &str,
) -> Option<NativeLibrary> {
    let tokens: Vec<String> = split_command(segment)
        .into_iter()
        .filter(|token| !token.is_empty())
        .collect();
    // Skip leading environment assignments.
    let start = tokens
        .iter()
        .position(|token| !token.contains('=') || token.starts_with('-'))?;
    let tokens = &tokens[start..];
    let compiler = tokens[0].trim_start_matches(['@', '-']);
    let program = compiler.rsplit('/').next().unwrap_or(compiler);
    if program == "emcc" || program == "em++" {
        return emscripten_library(repo_root, command_file, cwd, &tokens[1..]);
    }
    if !COMPILER_RE.is_match(program) {
        return None;
    }
    let args = &tokens[1..];
    if !args.iter().any(|arg| SHARED_FLAGS.contains(&arg.as_str())) {
        return None;
    }
    let parsed = command_options(args, &HashSet::from_iter(VALUED_FLAGS.iter().copied()));
    let output = parsed
        .get("-o")
        .filter(|output| !output.is_empty() && SHARED_OUTPUT_RE.is_match(output))?;
    let items: Vec<String> = parsed
        .positional
        .iter()
        .map(|item| object_source(repo_root, cwd, item).unwrap_or_else(|| item.clone()))
        .collect();
    let sources = existing_sources(repo_root, cwd, &items);
    if sources.is_empty() {
        return None;
    }
    Some(NativeLibrary::new(
        command_file,
        "make",
        library_stem(output),
        sources,
    ))
}

fn emscripten_library(
    repo_root: &Path,
    command_file: &str,
    cwd: &str,
    args: &[String],
) -> Option<NativeLibrary> {
    let mut settings: HashMap<String, String> = HashMap::new();
    let mut rest: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let setting = if arg == "-s" && index + 1 < args.len() {
            index += 1;
            &args[index]
        } else if arg.starts_with("-s") && arg.contains('=') {
            &arg[2..]
        } else {
            rest.push(arg.clone());
            index += 1;
            continue;
        };
        let (key, value) = setting.split_once('=').unwrap_or((setting, ""));
        settings.insert(key.to_string(), value.to_string());
        index += 1;
    }
    let parsed = command_options(&rest, &HashSet::from_iter(VALUED_FLAGS.iter().copied()));
    let output = parsed.get("-o").filter(|output| !output.is_empty())?;
    let output_rel = resolve_rel(cwd, output)?;
    let (stem, suffix) = output_rel.rsplit_once('.')?;
    let suffix = format!(".{suffix}");
    if !EMSCRIPTEN_GLUE_SUFFIXES.contains(&suffix.as_str()) && suffix != ".wasm" {
        return None;
    }
    let mut outputs = vec![output_rel.clone()];
    if suffix == ".html" {
        outputs.push(format!("{stem}.js"));
    }
    if suffix != ".wasm" {
        outputs.push(format!("{stem}.wasm"));
    }
    let items: Vec<String> = parsed
        .positional
        .iter()
        .map(|item| object_source(repo_root, cwd, item).unwrap_or_else(|| item.clone()))
        .collect();
    let sources = existing_sources(repo_root, cwd, &items);
    if sources.is_empty() {
        return None;
    }
    let wasm_exports = settings
        .get("EXPORTED_FUNCTIONS")
        .filter(|exported| !exported.starts_with('@'))
        .map(|exported| {
            EXPORT_NAME_RE
                .find_iter(exported)
                .map(|name| {
                    name.as_str()
                        .strip_prefix('_')
                        .unwrap_or(name.as_str())
                        .to_string()
                })
                .collect()
        });
    let name = stem.rsplit('/').next().unwrap_or(stem).to_string();
    let mut library = NativeLibrary::new(command_file, "emscripten", name, sources);
    library.outputs = outputs;
    library.wasm_exports = wasm_exports;
    Some(library)
}

/// The source file an object file `foo.o` is compiled from, if unique.
fn object_source(repo_root: &Path, cwd: &str, item: &str) -> Option<String> {
    let stem = item.strip_suffix(".o")?;
    let found: Vec<String> = C_SOURCE_SUFFIXES
        .iter()
        .map(|suffix| format!("{stem}{suffix}"))
        .filter(|candidate| resolve_rel(cwd, candidate).is_some_and(|rel| is_file(repo_root, &rel)))
        .collect();
    match found.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// `(targets, prerequisites)` of a rule line,
/// `^([^\s:=#][^:=]*?)\s*::?(?!=)\s*(.*)$`.
fn make_rule(line: &str) -> Option<(&str, &str)> {
    let first = line.chars().next()?;
    if is_py_space(first) || matches!(first, ':' | '=' | '#') {
        return None;
    }
    let colon = line[first.len_utf8()..].find([':', '='])? + first.len_utf8();
    if line.as_bytes()[colon] != b':' {
        return None;
    }
    let bytes = line.as_bytes();
    let colon_end = if bytes.get(colon + 1) == Some(&b':') && bytes.get(colon + 2) != Some(&b'=') {
        colon + 2
    } else if bytes.get(colon + 1) != Some(&b'=') {
        colon + 1
    } else {
        return None;
    };
    let targets = line[..colon].trim_end_matches(is_py_space);
    let targets = if targets.is_empty() {
        &line[..first.len_utf8()]
    } else {
        targets
    };
    Some((targets, line[colon_end..].trim_start_matches(is_py_space)))
}

/// Recipe lines with automatic and simple variables expanded.
pub(super) fn make_recipe_commands(text: &str) -> Vec<String> {
    let mut variables: HashMap<String, String> = HashMap::new();
    let mut commands = Vec::new();
    let mut rule: Option<(Vec<String>, Vec<String>)> = None;

    let expand = |variables: &HashMap<String, String>, value: &str| -> String {
        let mut value = value.to_string();
        for _ in 0..=8 {
            let expanded = MAKE_VAR_RE.replace_all(&value, |found: &Captures| {
                variables
                    .get(&found[1])
                    .map(String::as_str)
                    .or_else(|| make_default(&found[1]))
                    .unwrap_or(&found[0])
                    .to_string()
            });
            if expanded == value {
                break;
            }
            value = expanded.into_owned();
        }
        value
    };

    let joined = text.replace("\\\n", " ");
    for line in splitlines(&joined) {
        if line.starts_with('\t') {
            let Some((targets, prereqs)) = &rule else {
                continue;
            };
            let command = line
                .trim()
                .replace("$@", targets.first().map_or("", String::as_str))
                .replace("$^", &prereqs.join(" "))
                .replace("$+", &prereqs.join(" "))
                .replace("$<", prereqs.first().map_or("", String::as_str));
            commands.push(expand(&variables, &command));
            continue;
        }
        let stripped = line.split('#').next().unwrap_or("").trim_end();
        if stripped.trim().is_empty() {
            continue;
        }
        if let Some(assign) = MAKE_ASSIGN_RE.captures(stripped.trim()) {
            let (name, op, value) = (&assign[1], &assign[2], &assign[3]);
            if op == "+=" {
                let previous = variables.get(name).map_or("", String::as_str);
                let joined = format!("{previous} {value}").trim().to_string();
                variables.insert(name.to_string(), joined);
            } else if op != "?=" || !variables.contains_key(name) {
                variables.insert(name.to_string(), value.to_string());
            }
            rule = None;
            continue;
        }
        rule = make_rule(stripped).map(|(targets, prereqs)| {
            let prereqs = prereqs.split(';').next().unwrap_or("");
            let prereqs = prereqs.split('|').next().unwrap_or("");
            let words = |value: String| -> Vec<String> {
                value.split_whitespace().map(str::to_string).collect()
            };
            (
                words(expand(&variables, targets)),
                words(expand(&variables, prereqs)),
            )
        });
    }
    commands
}

pub(super) fn command_libraries(
    repo_root: &Path,
    command_file: &str,
    commands: &[String],
) -> Vec<NativeLibrary> {
    let cwd = parent_dir(command_file);
    let mut libraries = Vec::new();
    for command in commands {
        for segment in SEGMENT_SPLIT_RE.split(command) {
            if let Some(library) = command_library(repo_root, command_file, &cwd, segment) {
                libraries.push(library);
            }
        }
    }
    libraries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_lines_match_python_regex() {
        assert_eq!(make_rule("lib.so: a.o b.o"), Some(("lib.so", "a.o b.o")));
        assert_eq!(make_rule("a b::=c"), Some(("a b", ":=c")));
        assert_eq!(make_rule("all :: x"), Some(("all", "x")));
        assert_eq!(make_rule("a b:=c"), None);
        assert_eq!(make_rule(" a: b"), None);
        assert_eq!(make_rule("a = b"), None);
    }

    #[test]
    fn recipes_expand_automatic_and_simple_variables() {
        let text = "CC ?= gcc\nSRCS = a.c \\\n  b.c\nlibx.so: $(SRCS)\n\t$(CC) -shared -o $@ $^\n";
        assert_eq!(
            make_recipe_commands(text),
            ["gcc -shared -o libx.so a.c b.c"]
        );
    }
}
