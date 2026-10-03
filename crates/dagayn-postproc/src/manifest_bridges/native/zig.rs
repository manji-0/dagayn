//! Zig `build.zig` shared libraries and the C sources a Zig build compiles.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::super::text::{is_file, lossy_slice, parent_dir, read_text, resolve_rel};
use super::{NativeLibrary, existing_sources};

static LINE_COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"//[^\n]*").unwrap());
static ZIG_LIBRARY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?P<call>addSharedLibrary|addLibrary)\s*\(").unwrap());
static ZIG_DYNAMIC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.linkage\s*=\s*\.dynamic").unwrap());
static ZIG_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.name\s*=\s*"(?P<name>[^"]+)""#).unwrap());
static ZIG_ROOT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"\.root_source_file\s*=\s*(?:b\.path\(\s*"(?P<path>[^"]+)"\s*\)"#,
        r#"|\.\{\s*\.path\s*=\s*"(?P<legacy>[^"]+)"\s*\})"#,
    ))
    .unwrap()
});
static ZIG_C_FILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"addCSourceFile\s*\(\s*(?:\.\{[^}]*?\.file\s*=\s*b\.path\(\s*"(?P<file>[^"]+)""#,
        r#"|"(?P<legacy>[^"]+)")"#,
    ))
    .unwrap()
});
static ZIG_C_FILES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"addCSourceFiles\s*\(").unwrap());
static ZIG_C_ROOT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.root\s*=\s*b\.path\(\s*"([^"]+)"\s*\)"#).unwrap());
static ZIG_C_FILE_LIST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.files\s*=\s*&\s*\.\{(?P<items>[^}]*)\}").unwrap());
static ZIG_STRING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]+)""#).unwrap());

/// The argument text of the call whose `(` ends at `start`.
fn balanced_parens(text: &str, start: usize) -> String {
    let bytes = text.as_bytes();
    let (mut depth, mut index) = (1_i32, start);
    while index < bytes.len() && depth != 0 {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        index += 1;
    }
    lossy_slice(bytes, start, index.saturating_sub(1))
}

pub(super) fn zig_libraries(repo_root: &Path, build_rel: &str) -> Vec<NativeLibrary> {
    let Some(content) = read_text(&repo_root.join(build_rel)) else {
        return Vec::new();
    };
    let text = LINE_COMMENT_RE.replace_all(&content, "");
    let base = parent_dir(build_rel);
    let mut libraries = Vec::new();
    for found in ZIG_LIBRARY_RE.captures_iter(&text) {
        let args = balanced_parens(&text, found.get(0).unwrap().end());
        if &found["call"] == "addLibrary" && !ZIG_DYNAMIC_RE.is_match(&args) {
            continue;
        }
        let (Some(name), Some(root)) = (ZIG_NAME_RE.captures(&args), ZIG_ROOT_RE.captures(&args))
        else {
            continue;
        };
        let declared = root
            .name("path")
            .or_else(|| root.name("legacy"))
            .map_or("", |found| found.as_str());
        let Some(rel) = resolve_rel(&base, declared) else {
            continue;
        };
        if !is_file(repo_root, &rel) {
            continue;
        }
        libraries.push(NativeLibrary::new(
            build_rel,
            "zig",
            name["name"].replace('-', "_"),
            vec![rel],
        ));
    }
    let mut c_items: Vec<String> = ZIG_C_FILE_RE
        .captures_iter(&text)
        .filter_map(|found| found.name("file").or_else(|| found.name("legacy")))
        .map(|found| found.as_str().to_string())
        .collect();
    for found in ZIG_C_FILES_RE.find_iter(&text) {
        let args = balanced_parens(&text, found.end());
        let Some(files) = ZIG_C_FILE_LIST_RE.captures(&args) else {
            continue;
        };
        let prefix = ZIG_C_ROOT_RE
            .captures(&args)
            .map(|root| format!("{}/", root[1].trim_end_matches('/')))
            .unwrap_or_default();
        c_items.extend(
            ZIG_STRING_RE
                .captures_iter(&files["items"])
                .map(|name| format!("{prefix}{}", &name[1])),
        );
    }
    let sources = existing_sources(repo_root, &base, &c_items);
    if !sources.is_empty() {
        libraries.push(NativeLibrary::new(
            build_rel,
            "zig-c",
            String::new(),
            sources,
        ));
    }
    libraries
}
