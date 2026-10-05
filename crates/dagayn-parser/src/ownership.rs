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
    Dart,
    Lua,
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

impl RustOwnedPathKind {
    /// Whether this build compiled the grammar the kind's extractor parses
    /// with: its `lang-*` Cargo feature (all on by default). When it is off,
    /// the file stays Rust-owned and gets only a File node, labelled with
    /// [`Self::file_language`].
    pub(super) fn grammar_enabled(self) -> bool {
        match self {
            Self::Markdown => cfg!(feature = "lang-markdown"),
            Self::Terraform => cfg!(feature = "lang-terraform"),
            Self::Rust => cfg!(feature = "lang-rust"),
            Self::Python | Self::Notebook => cfg!(feature = "lang-python"),
            Self::JavaScript => cfg!(feature = "lang-javascript"),
            Self::TypeScript => cfg!(feature = "lang-typescript"),
            Self::Tsx => cfg!(feature = "lang-tsx"),
            Self::Bash => cfg!(feature = "lang-bash"),
            Self::Go => cfg!(feature = "lang-go"),
            Self::Java => cfg!(feature = "lang-java"),
            Self::Ruby => cfg!(feature = "lang-ruby"),
            Self::CSharp => cfg!(feature = "lang-csharp"),
            Self::Php => cfg!(feature = "lang-php"),
            Self::Kotlin => cfg!(feature = "lang-kotlin"),
            Self::Scala => cfg!(feature = "lang-scala"),
            Self::Dart => cfg!(feature = "lang-dart"),
            Self::Lua => cfg!(feature = "lang-lua"),
            Self::C => cfg!(feature = "lang-c"),
            Self::Cpp => cfg!(feature = "lang-cpp"),
            Self::ObjC => cfg!(feature = "lang-objc"),
            Self::Elixir => cfg!(feature = "lang-elixir"),
            Self::Gdscript => cfg!(feature = "lang-gdscript"),
            Self::R => cfg!(feature = "lang-r"),
            Self::Julia => cfg!(feature = "lang-julia"),
            Self::Perl => cfg!(feature = "lang-perl"),
            Self::Vue => cfg!(feature = "lang-vue"),
            Self::Svelte => cfg!(feature = "lang-svelte"),
            Self::Zig => cfg!(feature = "lang-zig"),
            Self::Swift => cfg!(feature = "lang-swift"),
            Self::PowerShell | Self::Unsupported => true,
        }
    }

    /// The `language` of the File node the kind's extractor emits.
    pub(super) fn file_language(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Terraform => "terraform",
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Notebook => "notebook",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Bash => "bash",
            Self::Go => "go",
            Self::Java => "java",
            Self::Ruby => "ruby",
            Self::CSharp => "csharp",
            Self::Php => "php",
            Self::Kotlin => "kotlin",
            Self::Scala => "scala",
            Self::Dart => "dart",
            Self::Lua => "lua",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::ObjC => "objc",
            Self::Elixir => "elixir",
            Self::Gdscript => "gdscript",
            Self::R => "r",
            Self::Julia => "julia",
            Self::Perl => "perl",
            Self::Vue => "vue",
            Self::Svelte => "svelte",
            Self::Zig => "zig",
            Self::PowerShell => "powershell",
            Self::Swift => "swift",
            Self::Unsupported => "",
        }
    }
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
        || ends_with_ascii_ignore_case(file_path, ".tf.json")
        || ends_with_ascii_ignore_case(file_path, ".tfvars.json")
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
        || ends_with_ascii_ignore_case(file_path, ".cjs")
    {
        RustOwnedPathKind::JavaScript
    } else if ends_with_ascii_ignore_case(file_path, ".ts")
        || ends_with_ascii_ignore_case(file_path, ".mts")
        || ends_with_ascii_ignore_case(file_path, ".cts")
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
    } else if ends_with_ascii_ignore_case(file_path, ".dart") {
        RustOwnedPathKind::Dart
    } else if ends_with_ascii_ignore_case(file_path, ".lua") {
        RustOwnedPathKind::Lua
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
    } else if ends_with_ascii_ignore_case(file_path, ".m")
        // Objective-C++: the Objective-C grammar recovers more of a `.mm`
        // file (its messages, interfaces, and C functions) than C++ does.
        || ends_with_ascii_ignore_case(file_path, ".mm")
    {
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
