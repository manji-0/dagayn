use std::path::Path;

use super::discovery::detect_language_from_shebang_bytes;
use super::util::ends_with_ascii_ignore_case;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RustOwnedPathKind {
    Markdown,
    Terraform,
    Rust,
    Python,
    Notebook,
    JavaScript,
    TypeScript,
    Tsx,
    Bash,
    Go,
    Java,
    Ruby,
    CSharp,
    Php,
    Kotlin,
    Scala,
    Solidity,
    Dart,
    Lua,
    Luau,
    C,
    Cpp,
    ObjC,
    Elixir,
    Gdscript,
    R,
    Julia,
    Perl,
    Vue,
    Svelte,
    Zig,
    PowerShell,
    Swift,
    Unsupported,
}

pub(super) fn rust_owned_path_kind(file_path: &str) -> RustOwnedPathKind {
    if ends_with_ascii_ignore_case(file_path, ".md")
        || ends_with_ascii_ignore_case(file_path, ".markdown")
    {
        RustOwnedPathKind::Markdown
    } else if ends_with_ascii_ignore_case(file_path, ".tf")
        || ends_with_ascii_ignore_case(file_path, ".tfvars")
        || ends_with_ascii_ignore_case(file_path, ".tftest.hcl")
        || ends_with_ascii_ignore_case(file_path, ".tfcomponent.hcl")
        || ends_with_ascii_ignore_case(file_path, ".tfdeploy.hcl")
        || ends_with_ascii_ignore_case(file_path, ".tfquery.hcl")
    {
        RustOwnedPathKind::Terraform
    } else if ends_with_ascii_ignore_case(file_path, ".rs") {
        RustOwnedPathKind::Rust
    } else if ends_with_ascii_ignore_case(file_path, ".py") {
        RustOwnedPathKind::Python
    } else if ends_with_ascii_ignore_case(file_path, ".ipynb") {
        RustOwnedPathKind::Notebook
    } else if ends_with_ascii_ignore_case(file_path, ".js")
        || ends_with_ascii_ignore_case(file_path, ".jsx")
        || ends_with_ascii_ignore_case(file_path, ".mjs")
    {
        RustOwnedPathKind::JavaScript
    } else if ends_with_ascii_ignore_case(file_path, ".ts")
        || ends_with_ascii_ignore_case(file_path, ".astro")
    {
        RustOwnedPathKind::TypeScript
    } else if ends_with_ascii_ignore_case(file_path, ".tsx") {
        RustOwnedPathKind::Tsx
    } else if ends_with_ascii_ignore_case(file_path, ".sh")
        || ends_with_ascii_ignore_case(file_path, ".bash")
        || ends_with_ascii_ignore_case(file_path, ".zsh")
        || ends_with_ascii_ignore_case(file_path, ".ksh")
    {
        RustOwnedPathKind::Bash
    } else if ends_with_ascii_ignore_case(file_path, ".go") {
        RustOwnedPathKind::Go
    } else if ends_with_ascii_ignore_case(file_path, ".java") {
        RustOwnedPathKind::Java
    } else if ends_with_ascii_ignore_case(file_path, ".rb") {
        RustOwnedPathKind::Ruby
    } else if ends_with_ascii_ignore_case(file_path, ".cs") {
        RustOwnedPathKind::CSharp
    } else if ends_with_ascii_ignore_case(file_path, ".php") {
        RustOwnedPathKind::Php
    } else if ends_with_ascii_ignore_case(file_path, ".kt")
        || ends_with_ascii_ignore_case(file_path, ".kts")
    {
        RustOwnedPathKind::Kotlin
    } else if ends_with_ascii_ignore_case(file_path, ".scala") {
        RustOwnedPathKind::Scala
    } else if ends_with_ascii_ignore_case(file_path, ".sol") {
        RustOwnedPathKind::Solidity
    } else if ends_with_ascii_ignore_case(file_path, ".dart") {
        RustOwnedPathKind::Dart
    } else if ends_with_ascii_ignore_case(file_path, ".lua") {
        RustOwnedPathKind::Lua
    } else if ends_with_ascii_ignore_case(file_path, ".luau") {
        RustOwnedPathKind::Luau
    } else if ends_with_ascii_ignore_case(file_path, ".c")
        || ends_with_ascii_ignore_case(file_path, ".h")
        || ends_with_ascii_ignore_case(file_path, ".xs")
    {
        RustOwnedPathKind::C
    } else if ends_with_ascii_ignore_case(file_path, ".cpp")
        || ends_with_ascii_ignore_case(file_path, ".cc")
        || ends_with_ascii_ignore_case(file_path, ".cxx")
        || ends_with_ascii_ignore_case(file_path, ".hpp")
    {
        RustOwnedPathKind::Cpp
    } else if ends_with_ascii_ignore_case(file_path, ".m") {
        RustOwnedPathKind::ObjC
    } else if ends_with_ascii_ignore_case(file_path, ".ex")
        || ends_with_ascii_ignore_case(file_path, ".exs")
    {
        RustOwnedPathKind::Elixir
    } else if ends_with_ascii_ignore_case(file_path, ".gd") {
        RustOwnedPathKind::Gdscript
    } else if ends_with_ascii_ignore_case(file_path, ".r") {
        RustOwnedPathKind::R
    } else if ends_with_ascii_ignore_case(file_path, ".jl") {
        RustOwnedPathKind::Julia
    } else if ends_with_ascii_ignore_case(file_path, ".pl")
        || ends_with_ascii_ignore_case(file_path, ".pm")
        || ends_with_ascii_ignore_case(file_path, ".t")
    {
        RustOwnedPathKind::Perl
    } else if ends_with_ascii_ignore_case(file_path, ".vue") {
        RustOwnedPathKind::Vue
    } else if ends_with_ascii_ignore_case(file_path, ".svelte") {
        RustOwnedPathKind::Svelte
    } else if ends_with_ascii_ignore_case(file_path, ".zig") {
        RustOwnedPathKind::Zig
    } else if ends_with_ascii_ignore_case(file_path, ".ps1")
        || ends_with_ascii_ignore_case(file_path, ".psm1")
        || ends_with_ascii_ignore_case(file_path, ".psd1")
    {
        RustOwnedPathKind::PowerShell
    } else if ends_with_ascii_ignore_case(file_path, ".swift") {
        RustOwnedPathKind::Swift
    } else {
        RustOwnedPathKind::Unsupported
    }
}

pub(super) fn rust_owned_path_kind_for_source(file_path: &str, source: &[u8]) -> RustOwnedPathKind {
    let kind = rust_owned_path_kind(file_path);
    if kind != RustOwnedPathKind::Unsupported || Path::new(file_path).extension().is_some() {
        return kind;
    }
    detect_language_from_shebang_bytes(source)
        .and_then(rust_owned_path_kind_for_language)
        .unwrap_or(RustOwnedPathKind::Unsupported)
}

fn rust_owned_path_kind_for_language(language: &str) -> Option<RustOwnedPathKind> {
    match language {
        "bash" => Some(RustOwnedPathKind::Bash),
        "python" => Some(RustOwnedPathKind::Python),
        "javascript" => Some(RustOwnedPathKind::JavaScript),
        "ruby" => Some(RustOwnedPathKind::Ruby),
        "perl" => Some(RustOwnedPathKind::Perl),
        "lua" => Some(RustOwnedPathKind::Lua),
        "r" => Some(RustOwnedPathKind::R),
        "php" => Some(RustOwnedPathKind::Php),
        _ => None,
    }
}
