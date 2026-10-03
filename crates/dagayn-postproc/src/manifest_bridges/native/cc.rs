//! Cargo build scripts compiling C / C++ with `cc` or `cxx_build`.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::super::text::{is_file, join_rel, parent_dir, read_text};
use super::{NativeLibrary, existing_sources};

static LINE_COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"//[^\n]*").unwrap());
static CC_CHAIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?P<kind>(?:cc::)?Build::new\(\)|cxx_build::bridges?\()").unwrap()
});
static CC_COMPILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.compile\(\s*"(?P<name>[^"]+)"\s*\)"#).unwrap());
static CC_FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.file\(\s*"(?P<path>[^"]+)"\s*\)"#).unwrap());
static CC_FILES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.files\(\s*&?\s*(?:vec!)?\s*\[(?P<items>[^\]]*)\]").unwrap());
static RUST_STRING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]*)""#).unwrap());

/// `build.rs` compiling C / C++ into the crate beside it: each
/// `cc::Build::new()` or `cxx_build::bridge(...)` chain up to its
/// `.compile("name")`, with the paths given to `.file` / `.files`.
pub(super) fn cc_build_libraries(repo_root: &Path, build_rel: &str) -> Vec<NativeLibrary> {
    let base = parent_dir(build_rel);
    if !is_file(repo_root, &join_rel(&base, "Cargo.toml")) {
        return Vec::new();
    }
    let Some(content) = read_text(&repo_root.join(build_rel)) else {
        return Vec::new();
    };
    let text = LINE_COMMENT_RE.replace_all(&content, "");
    let mut libraries = Vec::new();
    for start in CC_CHAIN_RE.captures_iter(&text) {
        let whole = start.get(0).unwrap();
        let Some(compile) = CC_COMPILE_RE.captures_at(&text, whole.end()) else {
            continue;
        };
        let chain = &text[whole.start()..compile.get(0).unwrap().end()];
        if CC_CHAIN_RE.find_at(chain, whole.len()).is_some() {
            continue; // another chain starts before this one compiles
        }
        let mut items: Vec<String> = CC_FILE_RE
            .captures_iter(chain)
            .map(|found| found["path"].to_string())
            .collect();
        for files in CC_FILES_RE.captures_iter(chain) {
            items.extend(
                RUST_STRING_RE
                    .captures_iter(&files["items"])
                    .map(|found| found[1].to_string()),
            );
        }
        let sources = existing_sources(repo_root, &base, &items);
        if sources.is_empty() {
            continue;
        }
        let kind = if start["kind"].starts_with("cxx_build") {
            "cxx"
        } else {
            "cc"
        };
        libraries.push(NativeLibrary::new(
            build_rel,
            kind,
            compile["name"].to_string(),
            sources,
        ));
    }
    libraries
}
