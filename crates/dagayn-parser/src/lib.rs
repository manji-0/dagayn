//! Rust parser crate.
//!
//! The migration target is for this crate to own file discovery, language
//! detection, parser orchestration, Markdown, Terraform, and notebook
//! extraction. During Phase 1 it starts with parseable-file filtering so Python
//! can shrink back toward CLI/MCP interfaces.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use globset::{Glob, GlobSetBuilder};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

static TERRAFORM_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$"#).unwrap());
static TERRAFORM_CALL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap());
static TERRAFORM_HEADER_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]*)"|'([^']*)'|([A-Za-z_][A-Za-z0-9_-]*)"#).unwrap());
static TERRAFORM_PROVIDER_SOURCE_FALLBACK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"source\s*=\s*(["'][^"']+["'])"#).unwrap());
static TERRAFORM_REFERENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:(data)\.([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)|((?:module|var|local|output|provider|check))\.([A-Za-z_][A-Za-z0-9_]*)|([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*))\b",
    )
    .unwrap()
});
static MARKDOWN_DIRECTIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<!--\s*(constrained-by|blocked-by|supersedes|derived-from)\s+(.+?)\s*-->")
        .unwrap()
});
static MARKDOWN_INLINE_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[^\]]+\]\(([^)]+)\)").unwrap());
static NOTEBOOK_SQL_TABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:FROM|JOIN|INTO|CREATE\s+(?:OR\s+REPLACE\s+)?(?:TABLE|VIEW)|INSERT\s+OVERWRITE)\s+((?:`[^`]+`|\w+)(?:\.(?:`[^`]+`|\w+))*)",
    )
    .unwrap()
});
static NOTEBOOK_R_FUNCTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([A-Za-z_.][A-Za-z0-9_.]*)\s*<-\s*function\s*(\([^)]*\))").unwrap()
});
static NOTEBOOK_R_CALL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Za-z_.][A-Za-z0-9_.]*)\s*\(").unwrap());
static MARKDOWN_REF_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*\[[^\]]+\]:\s*(\S+)").unwrap());
static MARKDOWN_CODE_SPAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`\n]+)`").unwrap());
static MARKDOWN_SYMBOL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*$").unwrap());
static MARKDOWN_TITLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\s+(?:"[^"]*"|'[^']*')\s*$"#).unwrap());
static RESCRIPT_MODULE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*module\s+(?:type\s+)?([A-Z][A-Za-z0-9_']*)\s*[:=]").unwrap()
});
static RESCRIPT_LET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:@[A-Za-z_][A-Za-z0-9_']*(?:\([^)]*\))?\s+)*(?:let\s+(?:rec\s+)?|and\s+)([A-Za-z_][A-Za-z0-9_']*)\b",
    )
    .unwrap()
});
static RESCRIPT_EXTERNAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:@[A-Za-z_][A-Za-z0-9_']*(?:\([^)]*\))?\s+)*external\s+([A-Za-z_][A-Za-z0-9_']*)\s*:",
    )
    .unwrap()
});
static RESCRIPT_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:@[A-Za-z_][A-Za-z0-9_']*(?:\([^)]*\))?\s+)*type\s+(?:rec\s+)?([A-Za-z_][A-Za-z0-9_']*)\b",
    )
    .unwrap()
});
static RESCRIPT_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(open|include)\s+([A-Z][A-Za-z0-9_'.]*)").unwrap());
static RESCRIPT_MODULE_ALIAS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*module\s+([A-Z][A-Za-z0-9_']*)\s*=\s*([A-Z][A-Za-z0-9_']*(?:\.[A-Za-z_][A-Za-z0-9_']*)*)\s*$",
    )
    .unwrap()
});
static RESCRIPT_JSX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)(^|[\s{(,>}])<([A-Z][A-Za-z0-9_']*(?:\.[A-Z][A-Za-z0-9_']*)*)\b").unwrap()
});
static RESCRIPT_MODULE_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"@module\(\s*"([^"]+)"\s*\)"#).unwrap());
static RESCRIPT_CALL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^A-Za-z0-9_'])([A-Za-z_][A-Za-z0-9_']*(?:\.[A-Za-z_][A-Za-z0-9_']*)*)\s*\(")
        .unwrap()
});
static RESCRIPT_DEFINITION_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)(?:let|type|module|external|and)\s").unwrap());
type JavaScriptExportCache = RefCell<HashMap<String, Option<JavaScriptExportIndex>>>;
type JavaScriptModuleCache = RefCell<HashMap<(String, String), Option<String>>>;
type JavaScriptTsconfigCache = RefCell<HashMap<PathBuf, Option<(PathBuf, Value)>>>;

#[derive(Clone, Copy, Default)]
struct JavaScriptCaches<'a> {
    export: Option<&'a JavaScriptExportCache>,
    module: Option<&'a JavaScriptModuleCache>,
    tsconfig: Option<&'a JavaScriptTsconfigCache>,
}

#[derive(Clone)]
struct JavaScriptExportIndex {
    defined_names: HashSet<String>,
    named_exports: HashMap<String, JavaScriptExportTarget>,
    star_exports: Vec<String>,
}

#[derive(Clone)]
enum JavaScriptExportTarget {
    Local(String),
    External {
        module_file: String,
        symbol_name: String,
    },
}

static EXTENSION_TO_LANGUAGE: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        (".py", "python"),
        (".js", "javascript"),
        (".jsx", "javascript"),
        (".ts", "typescript"),
        (".tsx", "tsx"),
        (".go", "go"),
        (".rs", "rust"),
        (".java", "java"),
        (".cs", "csharp"),
        (".rb", "ruby"),
        (".cpp", "cpp"),
        (".cc", "cpp"),
        (".cxx", "cpp"),
        (".c", "c"),
        (".h", "c"),
        (".hpp", "cpp"),
        (".kt", "kotlin"),
        (".kts", "kotlin"),
        (".swift", "swift"),
        (".php", "php"),
        (".scala", "scala"),
        (".sol", "solidity"),
        (".vue", "vue"),
        (".dart", "dart"),
        (".r", "r"),
        (".mjs", "javascript"),
        (".astro", "typescript"),
        (".pl", "perl"),
        (".pm", "perl"),
        (".t", "perl"),
        (".xs", "c"),
        (".lua", "lua"),
        (".luau", "luau"),
        (".m", "objc"),
        (".sh", "bash"),
        (".bash", "bash"),
        (".zsh", "bash"),
        (".ksh", "bash"),
        (".ex", "elixir"),
        (".exs", "elixir"),
        (".ipynb", "notebook"),
        (".zig", "zig"),
        (".ps1", "powershell"),
        (".psm1", "powershell"),
        (".psd1", "powershell"),
        (".svelte", "svelte"),
        (".jl", "julia"),
        (".res", "rescript"),
        (".resi", "rescript"),
        (".gd", "gdscript"),
        (".tf", "terraform"),
        (".tfvars", "terraform"),
        (".md", "markdown"),
        (".markdown", "markdown"),
    ])
});
static SHEBANG_TO_LANGUAGE: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("bash", "bash"),
        ("sh", "bash"),
        ("zsh", "bash"),
        ("ksh", "bash"),
        ("dash", "bash"),
        ("ash", "bash"),
        ("python", "python"),
        ("python2", "python"),
        ("python3", "python"),
        ("pypy", "python"),
        ("pypy3", "python"),
        ("node", "javascript"),
        ("nodejs", "javascript"),
        ("ruby", "ruby"),
        ("perl", "perl"),
        ("lua", "lua"),
        ("Rscript", "r"),
        ("php", "php"),
    ])
});
const LINE_CURSOR_SCAN_THRESHOLD: usize = 4;

pub fn grammar_status() -> dagayn_grammars::GrammarStatus {
    dagayn_grammars::status()
}

pub fn filter_parseable_files(
    repo_root: &Path,
    candidates: &[String],
    ignore_patterns: &[String],
) -> Vec<String> {
    let globset = build_globset(ignore_patterns);
    candidates
        .iter()
        .filter_map(|candidate| {
            let rel_path = candidate.as_str();
            if should_ignore(rel_path, ignore_patterns, globset.as_ref()) {
                return None;
            }
            let full_path = repo_root.join(rel_path);
            if !full_path.is_file() || full_path.is_symlink() {
                return None;
            }
            detect_language(&full_path)?;
            if is_binary(&full_path) {
                return None;
            }
            Some(candidate.clone())
        })
        .collect()
}

pub fn filter_incremental_candidates(
    repo_root: &Path,
    candidates: &[String],
    ignore_patterns: &[String],
) -> (Vec<String>, Vec<String>) {
    let globset = build_globset(ignore_patterns);
    let mut parseable_files = Vec::new();
    let mut removed_files = Vec::new();
    for candidate in candidates {
        let rel_path = candidate.as_str();
        if should_ignore(rel_path, ignore_patterns, globset.as_ref()) {
            continue;
        }
        let full_path = repo_root.join(rel_path);
        if !full_path.is_file() {
            removed_files.push(candidate.clone());
            continue;
        }
        if full_path.is_symlink() {
            continue;
        }
        if detect_language(&full_path).is_none() || is_binary(&full_path) {
            continue;
        }
        parseable_files.push(candidate.clone());
    }
    (parseable_files, removed_files)
}

pub fn collect_parseable_files(repo_root: &Path, recurse_submodules: Option<bool>) -> Vec<String> {
    let ignore_patterns = load_ignore_patterns(repo_root);
    let globset = build_globset(&ignore_patterns);
    let candidates = get_git_tracked_files(repo_root, recurse_submodules)
        .filter(|files| !files.is_empty())
        .unwrap_or_else(|| walk_files(repo_root, &ignore_patterns, globset.as_ref()));
    filter_parseable_files(repo_root, &candidates, &ignore_patterns)
}

pub fn detect_language(path: &Path) -> Option<&'static str> {
    let suffix = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{}", ext.to_ascii_lowercase()));
    if let Some(suffix) = suffix.as_deref() {
        if let Some(language) = EXTENSION_TO_LANGUAGE.get(suffix) {
            return Some(language);
        }
    }
    if path.extension().is_none() {
        return detect_language_from_shebang(path);
    }
    None
}

#[derive(Debug, Serialize)]
pub struct ParsedNode {
    pub kind: String,
    pub name: String,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub language: String,
    pub parent_name: Option<String>,
    pub params: Option<String>,
    pub return_type: Option<String>,
    pub modifiers: Option<String>,
    pub is_test: bool,
    pub extra: Value,
}

#[derive(Debug, Serialize)]
pub struct ParsedEdge {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub file_path: String,
    pub line: i64,
    pub extra: Value,
}

#[derive(Clone, Debug)]
struct Heading {
    text: String,
    slug: String,
    level: i64,
    line: i64,
}

struct MarkdownLineContext<'a> {
    file_path: &'a str,
    headings: &'a [Heading],
}

impl<'a> MarkdownLineContext<'a> {
    fn new(file_path: &'a str, headings: &'a [Heading]) -> Self {
        Self {
            file_path,
            headings,
        }
    }

    fn section_for_line(&self, line: i64) -> Option<String> {
        if self.headings.len() <= 8 {
            let mut section_slug = None;
            for heading in self.headings {
                if heading.line > line {
                    break;
                }
                section_slug = Some(heading.slug.as_str());
            }
            return section_slug.map(|slug| format!("{}::{slug}", self.file_path));
        }
        let idx = self
            .headings
            .partition_point(|heading| heading.line <= line);
        idx.checked_sub(1)
            .map(|idx| format!("{}::{}", self.file_path, self.headings[idx].slug))
    }

    fn source_for_line(&self, line: i64) -> String {
        self.section_for_line(line)
            .unwrap_or_else(|| self.file_path.to_string())
    }
}

struct LineCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
    line: i64,
    lookups: usize,
}

impl<'a> LineCursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            offset: 0,
            line: 1,
            lookups: 0,
        }
    }

    fn line_for_offset(&mut self, offset: usize) -> i64 {
        if self.lookups < LINE_CURSOR_SCAN_THRESHOLD {
            self.lookups += 1;
            let end = offset.min(self.bytes.len());
            self.offset = end;
            self.line = self.bytes[..end]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count() as i64
                + 1;
            return self.line;
        }
        if offset < self.offset {
            self.offset = 0;
            self.line = 1;
        }
        let end = offset.min(self.bytes.len());
        self.line += self.bytes[self.offset..end]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count() as i64;
        self.offset = end;
        self.line
    }
}

pub struct RustOwnedParser {
    markdown_parser: Option<tree_sitter::Parser>,
    terraform_parser: Option<tree_sitter::Parser>,
    rust_parser: Option<tree_sitter::Parser>,
    python_parser: Option<tree_sitter::Parser>,
    javascript_parser: Option<tree_sitter::Parser>,
    typescript_parser: Option<tree_sitter::Parser>,
    tsx_parser: Option<tree_sitter::Parser>,
    bash_parser: Option<tree_sitter::Parser>,
    go_parser: Option<tree_sitter::Parser>,
    java_parser: Option<tree_sitter::Parser>,
    ruby_parser: Option<tree_sitter::Parser>,
    csharp_parser: Option<tree_sitter::Parser>,
    php_parser: Option<tree_sitter::Parser>,
    kotlin_parser: Option<tree_sitter::Parser>,
    scala_parser: Option<tree_sitter::Parser>,
    solidity_parser: Option<tree_sitter::Parser>,
    dart_parser: Option<tree_sitter::Parser>,
    lua_parser: Option<tree_sitter::Parser>,
    luau_parser: Option<tree_sitter::Parser>,
    c_parser: Option<tree_sitter::Parser>,
    cpp_parser: Option<tree_sitter::Parser>,
    objc_parser: Option<tree_sitter::Parser>,
    elixir_parser: Option<tree_sitter::Parser>,
    gdscript_parser: Option<tree_sitter::Parser>,
    r_parser: Option<tree_sitter::Parser>,
    julia_parser: Option<tree_sitter::Parser>,
    perl_parser: Option<tree_sitter::Parser>,
    vue_parser: Option<tree_sitter::Parser>,
    svelte_parser: Option<tree_sitter::Parser>,
    zig_parser: Option<tree_sitter::Parser>,
    powershell_parser: Option<tree_sitter::Parser>,
    swift_parser: Option<tree_sitter::Parser>,
    javascript_export_cache: JavaScriptExportCache,
    javascript_module_cache: JavaScriptModuleCache,
    javascript_tsconfig_cache: JavaScriptTsconfigCache,
}

impl RustOwnedParser {
    pub fn new() -> Self {
        Self {
            markdown_parser: None,
            terraform_parser: None,
            rust_parser: None,
            python_parser: None,
            javascript_parser: None,
            typescript_parser: None,
            tsx_parser: None,
            bash_parser: None,
            go_parser: None,
            java_parser: None,
            ruby_parser: None,
            csharp_parser: None,
            php_parser: None,
            kotlin_parser: None,
            scala_parser: None,
            solidity_parser: None,
            dart_parser: None,
            lua_parser: None,
            luau_parser: None,
            c_parser: None,
            cpp_parser: None,
            objc_parser: None,
            elixir_parser: None,
            gdscript_parser: None,
            r_parser: None,
            julia_parser: None,
            perl_parser: None,
            vue_parser: None,
            svelte_parser: None,
            zig_parser: None,
            powershell_parser: None,
            swift_parser: None,
            javascript_export_cache: RefCell::new(HashMap::new()),
            javascript_module_cache: RefCell::new(HashMap::new()),
            javascript_tsconfig_cache: RefCell::new(HashMap::new()),
        }
    }

    pub fn parse_file(
        &mut self,
        file_path: &str,
        source: &[u8],
    ) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
        self.parse_file_in_repo(None, file_path, source)
    }

    pub fn parse_file_in_repo(
        &mut self,
        repo_root: Option<&Path>,
        file_path: &str,
        source: &[u8],
    ) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
        match rust_owned_path_kind_for_source(file_path, source) {
            RustOwnedPathKind::Markdown => parse_markdown_with_parser(
                file_path,
                source,
                parser_slot(&mut self.markdown_parser, new_markdown_parser),
            ),
            RustOwnedPathKind::Terraform => parse_terraform_with_parser(
                file_path,
                source,
                parser_slot(&mut self.terraform_parser, new_terraform_parser),
            ),
            RustOwnedPathKind::Rust => parse_rust_with_parser(
                file_path,
                source,
                parser_slot(&mut self.rust_parser, new_rust_parser),
            ),
            RustOwnedPathKind::Python => parse_python_with_parser(
                file_path,
                source,
                parser_slot(&mut self.python_parser, new_python_parser),
                repo_root,
            ),
            RustOwnedPathKind::Notebook => parse_notebook_with_parser(
                file_path,
                source,
                parser_slot(&mut self.python_parser, new_python_parser),
                repo_root,
            ),
            RustOwnedPathKind::JavaScript => parse_javascript_like_with_parser(
                file_path,
                source,
                "javascript",
                parser_slot(&mut self.javascript_parser, new_javascript_parser),
                repo_root,
                JavaScriptCaches {
                    export: Some(&self.javascript_export_cache),
                    module: Some(&self.javascript_module_cache),
                    tsconfig: Some(&self.javascript_tsconfig_cache),
                },
            ),
            RustOwnedPathKind::TypeScript => parse_javascript_like_with_parser(
                file_path,
                source,
                "typescript",
                parser_slot(&mut self.typescript_parser, new_typescript_parser),
                repo_root,
                JavaScriptCaches {
                    export: Some(&self.javascript_export_cache),
                    module: Some(&self.javascript_module_cache),
                    tsconfig: Some(&self.javascript_tsconfig_cache),
                },
            ),
            RustOwnedPathKind::Tsx => parse_javascript_like_with_parser(
                file_path,
                source,
                "tsx",
                parser_slot(&mut self.tsx_parser, new_tsx_parser),
                repo_root,
                JavaScriptCaches {
                    export: Some(&self.javascript_export_cache),
                    module: Some(&self.javascript_module_cache),
                    tsconfig: Some(&self.javascript_tsconfig_cache),
                },
            ),
            RustOwnedPathKind::Bash => parse_bash_with_parser(
                file_path,
                source,
                parser_slot(&mut self.bash_parser, new_bash_parser),
                repo_root,
            ),
            RustOwnedPathKind::Go => parse_go_with_parser(
                file_path,
                source,
                parser_slot(&mut self.go_parser, new_go_parser),
            ),
            RustOwnedPathKind::Java => parse_java_with_parser(
                file_path,
                source,
                parser_slot(&mut self.java_parser, new_java_parser),
                repo_root,
            ),
            RustOwnedPathKind::Ruby => parse_ruby_with_parser(
                file_path,
                source,
                parser_slot(&mut self.ruby_parser, new_ruby_parser),
            ),
            RustOwnedPathKind::CSharp => parse_csharp_with_parser(
                file_path,
                source,
                parser_slot(&mut self.csharp_parser, new_csharp_parser),
            ),
            RustOwnedPathKind::Php => parse_php_with_parser(
                file_path,
                source,
                parser_slot(&mut self.php_parser, new_php_parser),
            ),
            RustOwnedPathKind::Kotlin => parse_kotlin_with_parser(
                file_path,
                source,
                parser_slot(&mut self.kotlin_parser, new_kotlin_parser),
            ),
            RustOwnedPathKind::Scala => parse_scala_with_parser(
                file_path,
                source,
                parser_slot(&mut self.scala_parser, new_scala_parser),
            ),
            RustOwnedPathKind::Solidity => parse_solidity_with_parser(
                file_path,
                source,
                parser_slot(&mut self.solidity_parser, new_solidity_parser),
            ),
            RustOwnedPathKind::Dart => parse_dart_with_parser(
                file_path,
                source,
                parser_slot(&mut self.dart_parser, new_dart_parser),
            ),
            RustOwnedPathKind::Lua => parse_lua_with_parser(
                file_path,
                source,
                parser_slot(&mut self.lua_parser, new_lua_parser),
            ),
            RustOwnedPathKind::Luau => parse_luau_with_parser(
                file_path,
                source,
                parser_slot(&mut self.luau_parser, new_luau_parser),
            ),
            RustOwnedPathKind::C => parse_c_with_parser(
                file_path,
                source,
                parser_slot(&mut self.c_parser, new_c_parser),
            ),
            RustOwnedPathKind::Cpp => parse_cpp_with_parser(
                file_path,
                source,
                parser_slot(&mut self.cpp_parser, new_cpp_parser),
            ),
            RustOwnedPathKind::ObjC => parse_objc_with_parser(
                file_path,
                source,
                parser_slot(&mut self.objc_parser, new_objc_parser),
            ),
            RustOwnedPathKind::Elixir => parse_elixir_with_parser(
                file_path,
                source,
                parser_slot(&mut self.elixir_parser, new_elixir_parser),
            ),
            RustOwnedPathKind::Gdscript => parse_gdscript_with_parser(
                file_path,
                source,
                parser_slot(&mut self.gdscript_parser, new_gdscript_parser),
            ),
            RustOwnedPathKind::R => parse_r_with_parser(
                file_path,
                source,
                parser_slot(&mut self.r_parser, new_r_parser),
            ),
            RustOwnedPathKind::Julia => parse_julia_with_parser(
                file_path,
                source,
                parser_slot(&mut self.julia_parser, new_julia_parser),
            ),
            RustOwnedPathKind::Perl => parse_perl_with_parser(
                file_path,
                source,
                parser_slot(&mut self.perl_parser, new_perl_parser),
            ),
            RustOwnedPathKind::Vue => {
                ensure_parser(&mut self.vue_parser, new_vue_parser);
                ensure_parser(&mut self.javascript_parser, new_javascript_parser);
                ensure_parser(&mut self.typescript_parser, new_typescript_parser);
                parse_vue_with_parsers(
                    file_path,
                    source,
                    self.vue_parser.as_mut(),
                    self.javascript_parser.as_mut(),
                    self.typescript_parser.as_mut(),
                    repo_root,
                    JavaScriptCaches {
                        export: Some(&self.javascript_export_cache),
                        module: Some(&self.javascript_module_cache),
                        tsconfig: Some(&self.javascript_tsconfig_cache),
                    },
                )
            }
            RustOwnedPathKind::Svelte => {
                ensure_parser(&mut self.svelte_parser, new_svelte_parser);
                ensure_parser(&mut self.javascript_parser, new_javascript_parser);
                ensure_parser(&mut self.typescript_parser, new_typescript_parser);
                parse_svelte_with_parsers(
                    file_path,
                    source,
                    self.svelte_parser.as_mut(),
                    self.javascript_parser.as_mut(),
                    self.typescript_parser.as_mut(),
                    repo_root,
                    JavaScriptCaches {
                        export: Some(&self.javascript_export_cache),
                        module: Some(&self.javascript_module_cache),
                        tsconfig: Some(&self.javascript_tsconfig_cache),
                    },
                )
            }
            RustOwnedPathKind::Zig => {
                ensure_parser(&mut self.zig_parser, new_zig_parser);
                parse_zig_with_parser(file_path, source, self.zig_parser.as_mut())
            }
            RustOwnedPathKind::PowerShell => {
                ensure_parser(&mut self.powershell_parser, new_powershell_parser);
                parse_powershell_with_parser(file_path, source, self.powershell_parser.as_mut())
            }
            RustOwnedPathKind::ReScript => parse_rescript(file_path, source),
            RustOwnedPathKind::Swift => {
                ensure_parser(&mut self.swift_parser, new_swift_parser);
                parse_swift_with_parser(file_path, source, self.swift_parser.as_mut())
            }
            RustOwnedPathKind::Unsupported => (Vec::new(), Vec::new()),
        }
    }
}

impl Default for RustOwnedParser {
    fn default() -> Self {
        Self::new()
    }
}

fn ensure_parser(
    slot: &mut Option<tree_sitter::Parser>,
    init: fn() -> Option<tree_sitter::Parser>,
) {
    if slot.is_none() {
        *slot = init();
    }
}

fn parser_slot(
    slot: &mut Option<tree_sitter::Parser>,
    init: fn() -> Option<tree_sitter::Parser>,
) -> Option<&mut tree_sitter::Parser> {
    ensure_parser(slot, init);
    slot.as_mut()
}

pub fn parse_markdown(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_markdown_parser();
    parse_markdown_with_parser(file_path, source, parser.as_mut())
}

fn parse_markdown_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let line_end = line_count(source);
    let headings = collect_markdown_headings(source, &text, parser);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "markdown".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    let mut stack: Vec<(i64, String)> = Vec::new();
    for heading in &headings {
        while stack
            .last()
            .is_some_and(|(level, _)| *level >= heading.level)
        {
            stack.pop();
        }
        let section_qname = format!("{}::{}", file_path, heading.slug);
        let container = stack
            .last()
            .map(|(_, qname)| qname.clone())
            .unwrap_or_else(|| file_path.to_string());
        nodes.push(ParsedNode {
            kind: "Class".to_string(),
            name: heading.slug.clone(),
            file_path: file_path.to_string(),
            line_start: heading.line,
            line_end: heading.line,
            language: "markdown".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({
                "markdown_kind": "section",
                "display_name": heading.text,
                "heading_level": heading.level,
            }),
        });
        edges.push(ParsedEdge {
            kind: "CONTAINS".to_string(),
            source: container,
            target: section_qname.clone(),
            file_path: file_path.to_string(),
            line: heading.line,
            extra: json!({}),
        });
        stack.push((heading.level, section_qname));
    }

    let line_context = MarkdownLineContext::new(file_path, &headings);
    extract_markdown_directives(&line_context, &text, &mut edges);
    extract_markdown_links(&line_context, &text, &mut edges);
    extract_markdown_code_spans(&line_context, &text, &mut edges);
    (nodes, dedupe_edges(edges))
}

pub fn parse_markdown_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_markdown(file_path, source);
    parsed_compact_json(nodes, edges)
}

pub fn parse_rust_owned_files_compact_json(repo_root: &Path, file_paths: &[String]) -> String {
    let mut batch = Vec::new();
    let mut errors = Vec::new();
    let mut parser = RustOwnedParser::new();
    for file_path in file_paths {
        let full_path = repo_root.join(file_path);
        let source = match std::fs::read(&full_path) {
            Ok(source) => source,
            Err(err) => {
                errors.push(json!([file_path, err.to_string()]));
                continue;
            }
        };
        if !rust_parser_owns_source(file_path, &source) {
            errors.push(json!([file_path, "unsupported Rust parser path"]));
            continue;
        }
        let (nodes, edges) = parser.parse_file_in_repo(Some(repo_root), file_path, &source);
        let (nodes, edges) = parsed_compact_values(nodes, edges);
        batch.push(json!([file_path, nodes, edges, sha256_hex(&source)]));
    }
    json!({"batch": batch, "errors": errors}).to_string()
}

pub fn parse_terraform(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_terraform_parser();
    parse_terraform_with_parser(file_path, source, parser.as_mut())
}

fn parse_terraform_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let line_end = line_count(source);
    let blocks = collect_terraform_blocks(source, &text, parser);
    let mut defined_names = HashSet::new();
    for block in &blocks {
        if block.kind == "locals" {
            for attr in terraform_attrs(block).iter() {
                defined_names.insert(format!("local.{}", attr.name));
            }
        } else if let Some(name) = terraform_defined_name(block) {
            defined_names.insert(name);
        }
    }

    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "terraform".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    for block in &blocks {
        if block.kind == "locals" {
            for attr in terraform_attrs(block).iter() {
                let node_name = format!("local.{}", attr.name);
                push_terraform_node(
                    file_path,
                    &mut nodes,
                    &mut edges,
                    TerraformNodeSpec {
                        kind: "Function",
                        name: &node_name,
                        line_start: attr.line_start,
                        line_end: attr.line_end,
                        is_test: false,
                        terraform_kind: "local",
                    },
                );
                scan_terraform_attr(
                    attr,
                    &node_name,
                    file_path,
                    attr.line_start,
                    &defined_names,
                    &mut edges,
                );
            }
            continue;
        }

        if matches!(block.kind.as_str(), "import" | "moved" | "removed") {
            handle_terraform_meta_block(file_path, block, &defined_names, &mut edges);
            continue;
        }

        let Some(node_name) = terraform_defined_name(block) else {
            continue;
        };
        let terraform_kind = terraform_kind_for_block(block);
        let (kind, is_test) = match block.kind.as_str() {
            "variable" | "output" => ("Function", false),
            "check" => ("Test", true),
            _ => ("Class", false),
        };
        push_terraform_node(
            file_path,
            &mut nodes,
            &mut edges,
            TerraformNodeSpec {
                kind,
                name: &node_name,
                line_start: block.line_start,
                line_end: block.line_end,
                is_test,
                terraform_kind,
            },
        );
        scan_terraform_block(
            block,
            &node_name,
            file_path,
            block.line_start,
            &defined_names,
            &mut edges,
        );

        if block.kind == "module" {
            if let Some(source_attr) = terraform_attrs(block)
                .iter()
                .find(|attr| attr.name == "source")
            {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: terraform_qualified(file_path, &node_name),
                    target: strip_tf_string(&source_attr.value),
                    file_path: file_path.to_string(),
                    line: source_attr.line_start,
                    extra: json!({}),
                });
            }
        }

        if block.kind == "terraform" {
            for provider_source in terraform_provider_sources(block).iter() {
                edges.push(ParsedEdge {
                    kind: "DEPENDS_ON".to_string(),
                    source: terraform_qualified(file_path, &node_name),
                    target: provider_source.clone(),
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({}),
                });
            }
        }
    }

    (nodes, dedupe_edges(edges))
}

pub fn parse_terraform_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_terraform(file_path, source);
    parsed_compact_json(nodes, edges)
}

pub fn parse_python(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_python_parser();
    parse_python_with_parser(file_path, source, parser.as_mut(), None)
}

fn parse_python_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    if is_databricks_py_source(source) {
        return parse_databricks_py_with_parser(file_path, source, parser, repo_root);
    }

    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            let (import_map, top_level_defined_names) = collect_python_file_scope(root, source);
            let context = PythonParseContext {
                source,
                file_path,
                repo_root,
                import_map: &import_map,
                top_level_defined_names: &top_level_defined_names,
            };
            python_walk_children(root, &context, None, None, &mut nodes, &mut edges);
            let edges = resolve_python_call_targets(&nodes, edges, file_path);
            let edges = add_python_tested_by_edges(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

pub fn parse_python_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_python(file_path, source);
    parsed_compact_json(nodes, edges)
}

pub fn parse_notebook(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_python_parser();
    parse_notebook_with_parser(file_path, source, parser.as_mut(), None)
}

fn parse_notebook_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let Ok(notebook) = serde_json::from_slice::<Value>(source) else {
        return (Vec::new(), Vec::new());
    };
    let Some(default_language) = notebook_kernel_language(&notebook) else {
        return (Vec::new(), Vec::new());
    };
    let cells = collect_notebook_cells(&notebook, default_language);
    if cells.is_empty() {
        return (
            vec![notebook_file_node(
                file_path,
                1,
                default_language,
                is_test_file(file_path),
                None,
            )],
            Vec::new(),
        );
    }
    parse_notebook_cells_with_parser(file_path, &cells, default_language, None, parser, repo_root)
}

struct PythonParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    repo_root: Option<&'a Path>,
    import_map: &'a HashMap<String, String>,
    top_level_defined_names: &'a HashSet<String>,
}

#[derive(Clone)]
struct NotebookCell {
    cell_index: i64,
    language: &'static str,
    source: String,
}

fn is_databricks_py_source(source: &[u8]) -> bool {
    let first_line = source
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    first_line.trim_ascii() == b"# Databricks notebook source"
}

fn parse_databricks_py_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let cells = collect_databricks_py_cells(&text);
    if cells.is_empty() {
        return (
            vec![databricks_file_node(file_path, 1, is_test_file(file_path))],
            Vec::new(),
        );
    }
    parse_notebook_cells_with_parser(
        file_path,
        &cells,
        "python",
        Some("databricks_py"),
        parser,
        repo_root,
    )
}

fn parse_notebook_cells_with_parser(
    file_path: &str,
    cells: &[NotebookCell],
    default_language: &'static str,
    notebook_format: Option<&'static str>,
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut cell_offsets = Vec::new();
    let mut max_line = 1_i64;
    let mut parser = parser;
    let mut languages = Vec::<&'static str>::new();
    for cell in cells {
        if !languages.contains(&cell.language) {
            languages.push(cell.language);
        }
    }

    for language in languages {
        let lang_cells = cells
            .iter()
            .filter(|cell| cell.language == language)
            .cloned()
            .collect::<Vec<_>>();
        match language {
            "python" => {
                let (mut parsed_nodes, parsed_edges, offsets, current_line) =
                    parse_databricks_python_cells(
                        file_path,
                        &lang_cells,
                        parser.as_deref_mut(),
                        repo_root,
                    );
                nodes.append(&mut parsed_nodes);
                edges.extend(parsed_edges);
                cell_offsets.extend(offsets);
                max_line = max_line.max(current_line);
            }
            "sql" => {
                for cell in &lang_cells {
                    extract_databricks_sql_imports(file_path, cell, &mut edges);
                }
            }
            "r" => {
                let (mut parsed_nodes, parsed_edges, offsets, current_line) =
                    parse_databricks_r_cells(file_path, &lang_cells);
                nodes.append(&mut parsed_nodes);
                edges.extend(parsed_edges);
                cell_offsets.extend(offsets);
                max_line = max_line.max(current_line);
            }
            _ => {}
        }
    }

    let file_node = notebook_file_node(
        file_path,
        max_line,
        default_language,
        is_test_file(file_path),
        notebook_format,
    );
    nodes.insert(0, file_node);
    tag_notebook_cell_indices(&mut nodes, &cell_offsets);
    let edges = resolve_python_call_targets(&nodes, edges, file_path);
    let edges = add_python_tested_by_edges(&nodes, edges, file_path);
    (nodes, edges)
}

fn databricks_file_node(file_path: &str, line_end: i64, is_test: bool) -> ParsedNode {
    notebook_file_node(
        file_path,
        line_end,
        "python",
        is_test,
        Some("databricks_py"),
    )
}

fn notebook_kernel_language(notebook: &Value) -> Option<&'static str> {
    let language = notebook
        .pointer("/metadata/kernelspec/language")
        .and_then(Value::as_str)
        .or_else(|| {
            notebook
                .pointer("/metadata/language_info/name")
                .and_then(Value::as_str)
        })
        .unwrap_or("python")
        .to_ascii_lowercase();
    match language.as_str() {
        "python" => Some("python"),
        "r" => Some("r"),
        _ => None,
    }
}

fn collect_notebook_cells(notebook: &Value, default_language: &'static str) -> Vec<NotebookCell> {
    let Some(cells) = notebook.get("cells").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (cell_index, cell) in cells.iter().enumerate() {
        if cell.get("cell_type").and_then(Value::as_str) != Some("code") {
            continue;
        }
        let lines = notebook_source_lines(cell.get("source"));
        if lines.is_empty() {
            continue;
        }
        let first_line = lines[0].trim();
        let mut cell_language = default_language;
        let mut cell_lines = lines.as_slice();
        if first_line == "%python" || first_line.starts_with("%python ") {
            cell_language = "python";
            cell_lines = &lines[1..];
        } else if first_line == "%sql" || first_line.starts_with("%sql ") {
            cell_language = "sql";
            cell_lines = &lines[1..];
        } else if first_line == "%r" || first_line.starts_with("%r ") {
            cell_language = "r";
            cell_lines = &lines[1..];
        } else if first_line == "%scala"
            || first_line.starts_with("%scala ")
            || first_line == "%md"
            || first_line.starts_with("%md ")
            || first_line == "%sh"
            || first_line.starts_with("%sh ")
        {
            continue;
        }

        let filtered = if matches!(cell_language, "python" | "r") {
            cell_lines
                .iter()
                .filter(|line| {
                    let trimmed = line.trim_start();
                    !trimmed.starts_with('%') && !trimmed.starts_with('!')
                })
                .cloned()
                .collect::<Vec<_>>()
        } else {
            cell_lines.to_vec()
        };
        if filtered.is_empty() {
            continue;
        }
        out.push(NotebookCell {
            cell_index: cell_index as i64,
            language: cell_language,
            source: filtered.join(""),
        });
    }
    out
}

fn notebook_source_lines(source: Option<&Value>) -> Vec<String> {
    match source {
        Some(Value::Array(lines)) => lines
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(text)) => split_lines_keepends(text),
        _ => Vec::new(),
    }
}

fn split_lines_keepends(text: &str) -> Vec<String> {
    let mut lines = text
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() && !text.is_empty() {
        lines.push(text.to_string());
    }
    lines
}

fn notebook_file_node(
    file_path: &str,
    line_end: i64,
    language: &str,
    is_test: bool,
    notebook_format: Option<&str>,
) -> ParsedNode {
    ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: notebook_format
            .map(|format| json!({"notebook_format": format}))
            .unwrap_or_else(|| json!({})),
    }
}

fn collect_databricks_py_cells(text: &str) -> Vec<NotebookCell> {
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if lines
        .first()
        .is_some_and(|line| line.trim() == "# Databricks notebook source")
    {
        lines.remove(0);
    }

    let mut chunks = vec![Vec::<&str>::new()];
    for line in lines {
        if is_databricks_command_line(line) {
            chunks.push(Vec::new());
        } else if let Some(chunk) = chunks.last_mut() {
            chunk.push(line);
        }
    }

    let mut cells = Vec::new();
    for (cell_index, chunk) in chunks.into_iter().enumerate() {
        let non_empty = chunk
            .iter()
            .copied()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        if non_empty.is_empty() {
            continue;
        }
        let first_line = non_empty[0];
        let all_magic = non_empty.iter().all(|line| line.starts_with("# MAGIC "));
        let magic_language = if all_magic && first_line.starts_with("# MAGIC %sql") {
            Some("sql")
        } else if all_magic && first_line.starts_with("# MAGIC %r") {
            Some("r")
        } else {
            None
        };
        if let Some(language) = magic_language {
            let mut stripped = chunk
                .iter()
                .map(|line| line.strip_prefix("# MAGIC ").unwrap_or(line).to_string())
                .collect::<Vec<_>>();
            if let Some(first_directive) = stripped
                .iter()
                .find(|line| !line.trim().is_empty())
                .filter(|line| line.trim().starts_with('%'))
                .cloned()
            {
                stripped.retain(|line| line != &first_directive);
            }
            cells.push(NotebookCell {
                cell_index: cell_index as i64,
                language,
                source: stripped.join("\n"),
            });
            continue;
        }
        if all_magic
            && (first_line.starts_with("# MAGIC %md") || first_line.starts_with("# MAGIC %sh"))
        {
            continue;
        }
        let source = chunk
            .iter()
            .copied()
            .filter(|line| !line.starts_with("# MAGIC "))
            .collect::<Vec<_>>()
            .join("\n");
        cells.push(NotebookCell {
            cell_index: cell_index as i64,
            language: "python",
            source,
        });
    }
    cells
}

fn is_databricks_command_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("# COMMAND") && trimmed[9..].trim().bytes().all(|byte| byte == b'-')
}

type NotebookOffsets = Vec<(i64, i64, i64)>;

fn parse_databricks_python_cells(
    file_path: &str,
    cells: &[NotebookCell],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>, NotebookOffsets, i64) {
    let (source, offsets, current_line) = concatenate_notebook_cells(cells);
    let (nodes, edges) = parse_python_with_parser(file_path, source.as_bytes(), parser, repo_root);
    (
        nodes
            .into_iter()
            .filter(|node| node.kind != "File")
            .collect(),
        edges,
        offsets,
        current_line,
    )
}

fn parse_databricks_r_cells(
    file_path: &str,
    cells: &[NotebookCell],
) -> (Vec<ParsedNode>, Vec<ParsedEdge>, NotebookOffsets, i64) {
    let (source, offsets, current_line) = concatenate_notebook_cells(cells);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let lines = source.lines().collect::<Vec<_>>();
    let mut current_function: Option<String> = None;
    for (index, line) in lines.iter().enumerate() {
        let line_no = index as i64 + 1;
        if let Some(captures) = NOTEBOOK_R_FUNCTION_RE.captures(line) {
            let Some(name) = captures.get(1).map(|capture| capture.as_str()) else {
                continue;
            };
            let params = captures.get(2).map(|capture| capture.as_str().to_string());
            let line_end = find_r_function_end(&lines, index);
            let qualified = qualify(file_path, name, None);
            nodes.push(ParsedNode {
                kind: "Function".to_string(),
                name: name.to_string(),
                file_path: file_path.to_string(),
                line_start: line_no,
                line_end,
                language: "r".to_string(),
                parent_name: None,
                params,
                return_type: None,
                modifiers: None,
                is_test: false,
                extra: json!({}),
            });
            edges.push(ParsedEdge {
                kind: "CONTAINS".to_string(),
                source: file_path.to_string(),
                target: qualified.clone(),
                file_path: file_path.to_string(),
                line: line_no,
                extra: json!({}),
            });
            current_function = Some(qualified);
            continue;
        }
        let Some(caller) = current_function.as_ref() else {
            continue;
        };
        for captures in NOTEBOOK_R_CALL_RE.captures_iter(line) {
            let Some(name) = captures.get(1).map(|capture| capture.as_str()) else {
                continue;
            };
            if name == "function" {
                continue;
            }
            edges.push(ParsedEdge {
                kind: "CALLS".to_string(),
                source: caller.clone(),
                target: name.to_string(),
                file_path: file_path.to_string(),
                line: line_no,
                extra: json!({}),
            });
        }
    }
    (nodes, edges, offsets, current_line)
}

fn concatenate_notebook_cells(cells: &[NotebookCell]) -> (String, NotebookOffsets, i64) {
    let mut chunks = Vec::new();
    let mut offsets = Vec::new();
    let mut current_line = 1_i64;
    for cell in cells {
        let line_count = cell.source.matches('\n').count() as i64
            + if cell.source.ends_with('\n') { 0 } else { 1 };
        offsets.push((cell.cell_index, current_line, current_line + line_count - 1));
        chunks.push(cell.source.clone());
        current_line += line_count + 1;
    }
    (chunks.join("\n"), offsets, current_line)
}

fn find_r_function_end(lines: &[&str], start: usize) -> i64 {
    lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, line)| line.trim() == "}")
        .map(|(index, _)| index as i64 + 1)
        .unwrap_or(start as i64 + 1)
}

fn extract_databricks_sql_imports(
    file_path: &str,
    cell: &NotebookCell,
    edges: &mut Vec<ParsedEdge>,
) {
    for captures in NOTEBOOK_SQL_TABLE_RE.captures_iter(&cell.source) {
        let Some(target) = captures.get(1).map(|capture| capture.as_str()) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: "IMPORTS_FROM".to_string(),
            source: file_path.to_string(),
            target: target.replace('`', ""),
            file_path: file_path.to_string(),
            line: 1,
            extra: json!({}),
        });
    }
}

fn tag_notebook_cell_indices(nodes: &mut [ParsedNode], offsets: &[(i64, i64, i64)]) {
    for node in nodes {
        if node.kind == "File" {
            continue;
        }
        let mut best = None;
        let mut best_overlap = -1_i64;
        for (cell_index, start, end) in offsets {
            let overlap = node.line_end.min(*end) - node.line_start.max(*start) + 1;
            if overlap > best_overlap && overlap > 0 {
                best_overlap = overlap;
                best = Some(*cell_index);
            }
        }
        if let Some(cell_index) = best {
            set_node_extra_i64(node, "cell_index", cell_index);
        }
    }
}

fn set_node_extra_i64(node: &mut ParsedNode, key: &str, value: i64) {
    if !node.extra.is_object() {
        node.extra = json!({});
    }
    if let Some(map) = node.extra.as_object_mut() {
        map.insert(key.to_string(), json!(value));
    }
}

fn python_walk_children(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_definition" => {
                if let Some(name) = python_identifier_child(child, context.source) {
                    let qualified = qualify(context.file_path, &name, enclosing_class);
                    nodes.push(ParsedNode {
                        kind: "Class".to_string(),
                        name: name.clone(),
                        file_path: context.file_path.to_string(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "python".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: json!({"type_role": "class"}),
                    });
                    edges.push(ParsedEdge {
                        kind: "CONTAINS".to_string(),
                        source: context.file_path.to_string(),
                        target: qualified.clone(),
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    python_emit_bases(child, context.source, context.file_path, &qualified, edges);
                    python_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = python_identifier_child(child, context.source) {
                    let qualified = qualify(context.file_path, &name, enclosing_class);
                    let params = python_child_text(child, context.source, "parameters");
                    let return_type = python_return_type(child, context.source);
                    let is_test =
                        python_is_test_function(&name, context.file_path, child, context.source);
                    nodes.push(ParsedNode {
                        kind: if is_test { "Test" } else { "Function" }.to_string(),
                        name: name.clone(),
                        file_path: context.file_path.to_string(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "python".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params,
                        return_type,
                        modifiers: None,
                        is_test,
                        extra: json!({}),
                    });
                    let container = enclosing_class
                        .map(|name| qualify(context.file_path, name, None))
                        .unwrap_or_else(|| context.file_path.to_string());
                    edges.push(ParsedEdge {
                        kind: "CONTAINS".to_string(),
                        source: container,
                        target: qualified,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    python_walk_children(
                        child,
                        context,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "import_statement" | "import_from_statement" => {
                for target in python_import_targets(
                    child,
                    context.source,
                    context.file_path,
                    context.repo_root,
                ) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
            }
            "call" => {
                if let Some(call_name) = python_call_name(child, context.source) {
                    let caller = enclosing_func
                        .map(|name| qualify(context.file_path, name, enclosing_class))
                        .unwrap_or_else(|| context.file_path.to_string());
                    let target = python_resolve_imported_call_target(&call_name, context)
                        .unwrap_or_else(|| call_name.clone());
                    edges.push(ParsedEdge {
                        kind: "CALLS".to_string(),
                        source: caller.clone(),
                        target,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    if let Some(edge) =
                        python_bridge_edge(child, context.source, context.file_path, &caller)
                    {
                        edges.push(edge);
                    }
                }
            }
            "pair" | "assignment" | "list" => {
                python_emit_value_references(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        python_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn python_emit_value_references(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|name| qualify(context.file_path, name, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    match node.kind() {
        "pair" => {
            if let Some(value_node) = python_last_value_child(node) {
                if value_node.kind() == "identifier" {
                    python_emit_reference_if_known(value_node, context, &caller, edges);
                }
            }
        }
        "assignment" => {
            let Some(lhs) = python_first_child(node) else {
                return;
            };
            if !matches!(lhs.kind(), "attribute" | "subscript") {
                return;
            }
            if let Some(rhs) = python_last_value_child(node) {
                if rhs.kind() == "identifier" {
                    python_emit_reference_if_known(rhs, context, &caller, edges);
                }
            }
        }
        "list" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    python_emit_reference_if_known(child, context, &caller, edges);
                }
            }
        }
        _ => {}
    }
}

fn python_last_value_child(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut last = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !matches!(
            child.kind(),
            ":" | "," | "=" | "comment" | "type_annotation"
        ) {
            last = Some(child);
        }
    }
    last
}

fn python_first_child(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).next();
    first
}

fn python_emit_reference_if_known(
    node: tree_sitter::Node<'_>,
    context: &PythonParseContext<'_>,
    caller: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let name = node_text(node, context.source);
    let Some(target) = python_resolve_reference_target(&name, context) else {
        return;
    };
    edges.push(ParsedEdge {
        kind: "REFERENCES".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn python_resolve_reference_target(name: &str, context: &PythonParseContext<'_>) -> Option<String> {
    if python_skip_value_reference_name(name) {
        return None;
    }
    if context.top_level_defined_names.contains(name) {
        return Some(qualify(context.file_path, name, None));
    }
    let module = context.import_map.get(name)?;
    Some(
        python_resolve_module_to_file(module, context.file_path, context.repo_root)
            .map(|resolved| qualify(&resolved, name, None))
            .unwrap_or_else(|| name.to_string()),
    )
}

fn python_skip_value_reference_name(name: &str) -> bool {
    name.is_empty()
        || name.len() <= 1
        || name.bytes().all(|byte| !byte.is_ascii_lowercase())
        || matches!(
            name,
            "true"
                | "false"
                | "null"
                | "undefined"
                | "None"
                | "True"
                | "False"
                | "self"
                | "this"
                | "cls"
                | "super"
        )
}

fn collect_python_file_scope(
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> (HashMap<String, String>, HashSet<String>) {
    let mut import_map = HashMap::new();
    let mut defined_names = HashSet::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        let target = if child.kind() == "decorated_definition" {
            python_decorated_target(child)
        } else {
            Some(child)
        };
        if let Some(target) = target {
            match target.kind() {
                "class_definition" | "function_definition" => {
                    if let Some(name) = python_identifier_child(target, source) {
                        defined_names.insert(name);
                    }
                }
                "import_statement" | "import_from_statement" => {
                    collect_python_import_names(target, source, &mut import_map);
                }
                _ => {}
            }
        }
    }
    (import_map, defined_names)
}

fn python_decorated_target(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_definition"
                | "function_definition"
                | "import_statement"
                | "import_from_statement"
        ) {
            return Some(child);
        }
    }
    None
}

fn collect_python_import_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    import_map: &mut HashMap<String, String>,
) {
    if node.kind() != "import_from_statement" {
        return;
    }

    let mut module = None;
    let mut seen_import = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" if !seen_import => {
                module = Some(node_text(child, source));
            }
            "import" => {
                seen_import = true;
            }
            "identifier" | "dotted_name" if seen_import => {
                if let Some(module) = &module {
                    import_map.insert(node_text(child, source), module.clone());
                }
            }
            "aliased_import" if seen_import => {
                if let Some(module) = &module {
                    if let Some(name) = python_aliased_import_name(child, source) {
                        import_map.insert(name, module.clone());
                    }
                }
            }
            _ => {}
        }
    }
}

fn python_aliased_import_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| matches!(child.kind(), "identifier" | "dotted_name"))
        .map(|child| node_text(child, source))
        .last()
}

fn python_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn python_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

fn python_return_type(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for (index, child) in children.iter().enumerate() {
        if child.kind() == "->" {
            return children
                .get(index + 1)
                .map(|return_node| node_text(*return_node, source));
        }
    }
    None
}

fn python_is_test_function(
    name: &str,
    file_path: &str,
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> bool {
    python_name_matches_test_pattern(name)
        || (is_test_file(file_path) && python_is_test_runner_name(name))
        || python_has_test_annotation(node, source)
}

fn python_name_matches_test_pattern(name: &str) -> bool {
    name.starts_with("test_")
        || name.starts_with("Test")
        || name.ends_with("_test")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("_spec")
}

fn python_is_test_runner_name(name: &str) -> bool {
    matches!(
        name,
        "describe" | "it" | "test" | "beforeEach" | "afterEach" | "beforeAll" | "afterAll"
    )
}

fn python_has_test_annotation(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "decorated_definition" {
        return false;
    }
    let mut cursor = parent.walk();
    let has_decorator = parent.children(&mut cursor).any(|child| {
        if child.kind() != "decorator" {
            return false;
        }
        let text = node_text(child, source);
        matches!(
            text.trim_start_matches('@').trim(),
            "Test"
                | "ParameterizedTest"
                | "RepeatedTest"
                | "TestFactory"
                | "org.junit.Test"
                | "org.junit.jupiter.api.Test"
        )
    });
    has_decorator
}

fn python_emit_bases(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    qualified: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "argument_list" {
            continue;
        }
        let mut arg_cursor = child.walk();
        for arg in child.children(&mut arg_cursor) {
            if matches!(arg.kind(), "identifier" | "attribute") {
                edges.push(ParsedEdge {
                    kind: "INHERITS".to_string(),
                    source: qualified.to_string(),
                    target: node_text(arg, source),
                    file_path: file_path.to_string(),
                    line: node.start_position().row as i64 + 1,
                    extra: json!({
                        "relationship_role": "extends",
                        "syntax_source": node.kind(),
                    }),
                });
            }
        }
    }
}

fn python_import_targets(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    repo_root: Option<&Path>,
) -> Vec<String> {
    if node.kind() == "import_statement" {
        let mut imports = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "dotted_name" {
                let target = node_text(child, source);
                imports.push(
                    python_resolve_module_to_file(&target, file_path, repo_root).unwrap_or(target),
                );
            }
        }
        return imports;
    }

    let mut module = None;
    let mut seen_import = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "relative_import" => {
                let text = node_text(child, source);
                module = Some(text);
                break;
            }
            "dotted_name" if !seen_import => {
                module = Some(node_text(child, source));
            }
            "import" => {
                seen_import = true;
            }
            _ => {}
        }
    }
    module
        .into_iter()
        .map(|target| {
            python_resolve_module_to_file(&target, file_path, repo_root).unwrap_or(target)
        })
        .collect()
}

fn python_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).next()?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "attribute" => rust_rightmost_identifier(first, source),
        _ => None,
    }
}

fn resolve_python_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    resolve_rust_call_targets(nodes, edges, file_path)
}

fn python_resolve_imported_call_target(
    call_name: &str,
    context: &PythonParseContext<'_>,
) -> Option<String> {
    if context.top_level_defined_names.contains(call_name) {
        return None;
    }
    let module = context.import_map.get(call_name)?;
    let resolved = python_resolve_module_to_file(module, context.file_path, context.repo_root)?;
    Some(qualify(&resolved, call_name, None))
}

fn add_python_tested_by_edges(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    if !is_test_file(file_path) {
        return edges;
    }
    let test_qnames = nodes
        .iter()
        .filter(|node| node.is_test)
        .map(|node| qualify(file_path, &node.name, node.parent_name.as_deref()))
        .collect::<HashSet<_>>();
    if test_qnames.is_empty() {
        return edges;
    }
    let mut out = edges;
    let tested_by_edges = out
        .iter()
        .filter(|edge| edge.kind == "CALLS" && test_qnames.contains(&edge.source))
        .map(|edge| ParsedEdge {
            kind: "TESTED_BY".to_string(),
            source: edge.target.clone(),
            target: edge.source.clone(),
            file_path: edge.file_path.clone(),
            line: edge.line,
            extra: json!({}),
        })
        .collect::<Vec<_>>();
    out.extend(tested_by_edges);
    out
}

fn python_resolve_module_to_file(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
) -> Option<String> {
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let candidates_for = |base: PathBuf, rel: &str| {
        [
            base.join(format!("{rel}.py")),
            base.join(rel).join("__init__.py"),
        ]
    };

    if module.starts_with('.') {
        let leading_dots = module.bytes().take_while(|byte| *byte == b'.').count();
        let remainder = &module[leading_dots..];
        let mut base = caller_dir.to_path_buf();
        for _ in 0..leading_dots.saturating_sub(1) {
            base = base.parent().unwrap_or(Path::new("")).to_path_buf();
        }
        let candidates = if remainder.is_empty() {
            vec![base.join("__init__.py")]
        } else {
            let rel = remainder.replace('.', "/");
            candidates_for(base, &rel).into_iter().collect()
        };
        return candidates
            .into_iter()
            .find(|candidate| python_module_candidate_is_file(candidate, repo_root))
            .and_then(|candidate| python_module_candidate_path(candidate, repo_root));
    }

    let rel = module.replace('.', "/");
    let mut current = caller_dir.to_path_buf();
    loop {
        for candidate in candidates_for(current.clone(), &rel) {
            if python_module_candidate_is_file(&candidate, repo_root) {
                return python_module_candidate_path(candidate, repo_root);
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    None
}

fn python_module_candidate_is_file(candidate: &Path, repo_root: Option<&Path>) -> bool {
    repo_root
        .map(|root| root.join(candidate).is_file())
        .unwrap_or_else(|| candidate.is_file())
}

fn python_module_candidate_path(candidate: PathBuf, repo_root: Option<&Path>) -> Option<String> {
    if repo_root.is_some() {
        return Some(candidate.to_string_lossy().to_string());
    }
    candidate
        .canonicalize()
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

fn python_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
) -> Option<ParsedEdge> {
    let signature = python_call_signature(node, source)?;
    let (relationship_role, bridge_kind) = python_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match python_first_string_arg(node, source) {
        Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
        _ => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "python",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn python_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let signature = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list")
        .map(|child| node_text(child, source).trim().to_string())
        .filter(|value| !value.is_empty());
    signature
}

fn python_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "subprocess.run"
        | "subprocess.Popen"
        | "subprocess.call"
        | "subprocess.check_call"
        | "subprocess.check_output"
        | "os.system"
        | "os.popen"
        | "os.execv"
        | "os.execvp"
        | "os.execvpe"
        | "os.execve"
        | "os.execl"
        | "os.execlp"
        | "os.execlpe"
        | "os.execle"
        | "os.spawnv"
        | "os.spawnvp" => Some(("invokes_binary", "subprocess")),
        "ctypes.CDLL"
        | "ctypes.cdll.LoadLibrary"
        | "ctypes.WinDLL"
        | "ctypes.PyDLL"
        | "cffi.FFI().dlopen" => Some(("loads_shared_library", "ffi")),
        "open" | "io.open" => Some(("opens_file", "file_io")),
        _ => None,
    }
}

fn python_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if child.kind() == "string" {
            return Some(decode_python_string_literal(child, source));
        }
        if matches!(child.kind(), "list" | "tuple") {
            return python_first_string_in_sequence(child, source);
        }
        return None;
    }
    None
}

fn python_first_string_in_sequence(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if child.kind() == "string" {
            return Some(decode_python_string_literal(child, source));
        }
    }
    None
}

fn decode_python_string_literal(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "string_content" | "string_fragment") {
            return node_text(child, source);
        }
    }
    node_text(node, source)
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`')
        .to_string()
}

pub fn parse_rust(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_rust_parser();
    parse_rust_with_parser(file_path, source, parser.as_mut())
}

fn parse_rust_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "rust".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            let mut defined_names = HashSet::new();
            collect_rust_defined_names(root, source, &mut defined_names);
            let context = RustParseContext {
                source,
                file_path,
                defined_names: &defined_names,
            };
            rust_walk_children(root, &context, None, None, &mut nodes, &mut edges);
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

pub fn parse_rust_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_rust(file_path, source);
    parsed_compact_json(nodes, edges)
}

fn parse_javascript_like(
    file_path: &str,
    source: &[u8],
    language: &'static str,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = match language {
        "javascript" => new_javascript_parser(),
        "typescript" => new_typescript_parser(),
        "tsx" => new_tsx_parser(),
        _ => None,
    };
    parse_javascript_like_with_parser(
        file_path,
        source,
        language,
        parser.as_mut(),
        None,
        JavaScriptCaches::default(),
    )
}

fn parse_javascript_like_with_parser(
    file_path: &str,
    source: &[u8],
    language: &'static str,
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let test_file = is_javascript_test_file(file_path);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: test_file,
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            let mut defined_names = HashSet::new();
            collect_javascript_defined_names(root, source, &mut defined_names);
            let mut import_map = HashMap::new();
            collect_javascript_import_map(root, source, &mut import_map);
            let context = JavaScriptParseContext {
                source,
                file_path,
                language,
                test_file,
                defined_names: &defined_names,
                import_map: &import_map,
                repo_root,
                caches,
            };
            javascript_walk_children(root, &context, None, None, &mut nodes, &mut edges);
            let mut edges = resolve_rust_call_targets(&nodes, edges, file_path);
            if test_file {
                add_tested_by_edges(&nodes, &mut edges);
            }
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

pub fn parse_vue(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut vue_parser = new_vue_parser();
    let mut javascript_parser = new_javascript_parser();
    let mut typescript_parser = new_typescript_parser();
    parse_vue_with_parsers(
        file_path,
        source,
        vue_parser.as_mut(),
        javascript_parser.as_mut(),
        typescript_parser.as_mut(),
        None,
        JavaScriptCaches::default(),
    )
}

pub fn parse_svelte(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut svelte_parser = new_svelte_parser();
    let mut javascript_parser = new_javascript_parser();
    let mut typescript_parser = new_typescript_parser();
    parse_svelte_with_parsers(
        file_path,
        source,
        svelte_parser.as_mut(),
        javascript_parser.as_mut(),
        typescript_parser.as_mut(),
        None,
        JavaScriptCaches::default(),
    )
}

pub fn parse_zig(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_zig_parser();
    parse_zig_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_powershell(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_powershell_parser();
    parse_powershell_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_swift(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_swift_parser();
    parse_swift_with_parser(file_path, source, parser.as_mut())
}

fn parse_vue_with_parsers(
    file_path: &str,
    source: &[u8],
    vue_parser: Option<&mut tree_sitter::Parser>,
    javascript_parser: Option<&mut tree_sitter::Parser>,
    typescript_parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_sfc_with_parsers(
        file_path,
        source,
        SfcParserInputs {
            language: "vue",
            sfc_parser: vue_parser,
            javascript_parser,
            typescript_parser,
            repo_root,
            caches,
        },
    )
}

fn parse_svelte_with_parsers(
    file_path: &str,
    source: &[u8],
    svelte_parser: Option<&mut tree_sitter::Parser>,
    javascript_parser: Option<&mut tree_sitter::Parser>,
    typescript_parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_sfc_with_parsers(
        file_path,
        source,
        SfcParserInputs {
            language: "svelte",
            sfc_parser: svelte_parser,
            javascript_parser,
            typescript_parser,
            repo_root,
            caches,
        },
    )
}

fn parse_zig_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_tree_sitter_file_only_with_parser(file_path, source, "zig", parser)
}

fn parse_powershell_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_tree_sitter_file_only_with_parser(file_path, source, "powershell", parser)
}

fn parse_tree_sitter_file_only_with_parser(
    file_path: &str,
    source: &[u8],
    language: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    if let Some(parser) = parser {
        let _ = parser.parse(source, None);
    }
    let line_end = line_count(source);
    (
        vec![ParsedNode {
            kind: "File".to_string(),
            name: file_path.to_string(),
            file_path: file_path.to_string(),
            line_start: 1,
            line_end,
            language: language.to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: is_test_file(file_path),
            extra: json!({}),
        }],
        Vec::new(),
    )
}

fn parse_swift_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "swift".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let context = SwiftParseContext { source, file_path };
            swift_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }
    (nodes, edges)
}

struct SwiftParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
}

fn swift_walk_children(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_declaration" if enclosing_class.is_none() && enclosing_func.is_none() => {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: context.file_path.to_string(),
                    target: node_text(child, context.source).trim().to_string(),
                    file_path: context.file_path.to_string(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
                continue;
            }
            "class_declaration" | "protocol_declaration" => {
                if let Some(name) = swift_type_name(child, context.source) {
                    swift_emit_class(child, context, &name, nodes, edges);
                    swift_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_declaration" => {
                if let Some(name) = swift_function_name(child, context.source) {
                    swift_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    swift_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "call_expression" => {
                swift_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        swift_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn swift_emit_class(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let swift_kind = swift_type_kind(node, context.source);
    let (type_role, extra_flags) = match swift_kind.as_str() {
        "protocol" => (
            "protocol",
            json!({"is_abstract": true, "is_contract": true}),
        ),
        "struct" => ("struct", json!({})),
        "enum" => ("enum", json!({})),
        _ => ("class", json!({})),
    };
    let mut extra = json!({"type_role": type_role, "swift_kind": swift_kind});
    if let (Some(extra_obj), Some(flags)) = (extra.as_object_mut(), extra_flags.as_object()) {
        for (key, value) in flags {
            extra_obj.insert(key.clone(), value.clone());
        }
    }
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "swift".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.file_path.to_string(),
        target: qualify(context.file_path, name, None),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for base in swift_inheritance_targets(node, context.source) {
        edges.push(ParsedEdge {
            kind: "INHERITS".to_string(),
            source: qualify(context.file_path, name, None),
            target: base,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": "extends",
                "syntax_source": "class_declaration",
            }),
        });
    }
}

fn swift_emit_function(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "swift".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualify(context.file_path, name, enclosing_class),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn swift_emit_call(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    if let Some(call_name) = swift_call_name(node, context.source) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = swift_call_signature(node, context.source) {
        if let Some(edge) = swift_bridge_edge(node, context, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn swift_type_kind(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    if node.kind() == "protocol_declaration" {
        return "protocol".to_string();
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = node_text(child, source);
        if matches!(
            text.as_str(),
            "class" | "struct" | "enum" | "actor" | "extension"
        ) {
            return text;
        }
    }
    "class".to_string()
}

fn swift_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    swift_direct_child(node, &["type_identifier"])
        .map(|child| node_text(child, source))
        .or_else(|| swift_first_descendant_text(node, source, &["type_identifier"]))
}

fn swift_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    swift_direct_child(node, &["simple_identifier"]).map(|child| node_text(child, source))
}

fn swift_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(specifier) = swift_direct_child(node, &["inheritance_specifier"]) else {
        return Vec::new();
    };
    swift_descendant_texts(specifier, source, &["type_identifier"])
}

fn swift_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = swift_call_callee(node)?;
    match callee.kind() {
        "simple_identifier" => Some(node_text(callee, source)),
        "navigation_expression" => {
            swift_last_descendant_text(callee, source, &["simple_identifier"])
        }
        _ => None,
    }
}

fn swift_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = swift_call_callee(node)?;
    match callee.kind() {
        "simple_identifier" => Some(node_text(callee, source)),
        "navigation_expression" => {
            let parts = swift_descendant_texts(callee, source, &["simple_identifier"]);
            (!parts.is_empty()).then(|| parts.join("."))
        }
        _ => None,
    }
}

fn swift_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "call_suffix");
    found
}

fn swift_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &SwiftParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Process.run" => ("invokes_binary", "subprocess"),
        "String.contentsOf" | "Data.contentsOf" | "FileManager.contentsOfFile" => {
            ("reads_file", "file_io")
        }
        "FileManager.createFile" => ("writes_file", "file_io"),
        "dlopen" | "Bundle.load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match swift_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "swift",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn swift_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let args = swift_first_descendant(node, &["value_arguments"])?;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() != "value_argument" {
            continue;
        }
        let mut value_cursor = child.walk();
        for value in child.children(&mut value_cursor) {
            if value.kind() == "line_string_literal" {
                return Some(swift_string_text(value, source));
            }
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn swift_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let text = node_text(node, source);
    strip_matching_quotes(text.trim()).to_string()
}

fn swift_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn swift_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = swift_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn swift_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    swift_first_descendant(node, kinds).map(|child| node_text(child, source))
}

fn swift_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    swift_collect_descendant_texts(node, source, kinds, &mut out);
    out
}

fn swift_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut out = Vec::new();
    swift_collect_descendant_texts(node, source, kinds, &mut out);
    out.pop()
}

fn swift_collect_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    out: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
        swift_collect_descendant_texts(child, source, kinds, out);
    }
}

pub fn parse_rescript(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let text = String::from_utf8_lossy(source);
    let cleaned = strip_rescript_noise(&text);
    let line_starts = line_starts(&cleaned);
    let is_interface = ends_with_ascii_ignore_case(file_path, ".resi");
    let test_file = rescript_is_test_file(file_path);

    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end: text
            .as_bytes()
            .iter()
            .filter(|byte| **byte == b'\n')
            .count() as i64
            + 1,
        language: "rescript".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: test_file,
        extra: if is_interface {
            json!({"rescript_interface": true})
        } else {
            json!({})
        },
    }];
    let mut edges = Vec::new();
    let mut modules = scan_rescript_modules(&cleaned, &line_starts);
    assign_rescript_module_parents(&mut modules);
    let depth = rescript_brace_depth_array(&cleaned);

    for module in &modules {
        nodes.push(ParsedNode {
            kind: "Class".to_string(),
            name: module.name.clone(),
            file_path: file_path.to_string(),
            line_start: module.start_line,
            line_end: module.end_line,
            language: "rescript".to_string(),
            parent_name: module.parent.clone(),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"rescript_kind": "module"}),
        });
    }

    let mut lets = collect_rescript_lets(file_path, &cleaned, &line_starts, &modules, &depth);
    fill_rescript_let_ends(&mut lets, &cleaned, &line_starts, &modules);
    for entry in &lets {
        nodes.push(ParsedNode {
            kind: if entry.is_test { "Test" } else { "Function" }.to_string(),
            name: entry.name.clone(),
            file_path: file_path.to_string(),
            line_start: entry.line_start,
            line_end: entry.line_end,
            language: "rescript".to_string(),
            parent_name: entry.parent.clone(),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: entry.is_test,
            extra: json!({}),
        });
    }

    for capture in RESCRIPT_EXTERNAL_RE.captures_iter(&cleaned) {
        let Some(name_match) = capture.get(1) else {
            continue;
        };
        let name = name_match.as_str();
        if rescript_is_keyword(name) {
            continue;
        }
        let off = name_match.start();
        let parent = rescript_enclosing_module(&modules, off);
        if !rescript_is_top_level(off, parent.as_deref(), &modules, &depth) {
            continue;
        }
        let line_start = offset_to_line(&line_starts, off);
        nodes.push(ParsedNode {
            kind: "Function".to_string(),
            name: name.to_string(),
            file_path: file_path.to_string(),
            line_start,
            line_end: line_start,
            language: "rescript".to_string(),
            parent_name: parent,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"rescript_external": true}),
        });
        let look_start = off.saturating_sub(200);
        if let Some(snippet) = safe_str_slice(&text, look_start, off) {
            for attr in RESCRIPT_MODULE_ATTR_RE.captures_iter(snippet) {
                if let Some(target) = attr.get(1) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: file_path.to_string(),
                        target: target.as_str().to_string(),
                        file_path: file_path.to_string(),
                        line: line_start,
                        extra: json!({"rescript_import_kind": "external_module"}),
                    });
                }
            }
        }
    }

    for capture in RESCRIPT_TYPE_RE.captures_iter(&cleaned) {
        let Some(name_match) = capture.get(1) else {
            continue;
        };
        let name = name_match.as_str();
        if rescript_is_keyword(name) {
            continue;
        }
        let off = name_match.start();
        let parent = rescript_enclosing_module(&modules, off);
        if !rescript_is_top_level(off, parent.as_deref(), &modules, &depth) {
            continue;
        }
        let line_start = offset_to_line(&line_starts, off);
        nodes.push(ParsedNode {
            kind: "Type".to_string(),
            name: name.to_string(),
            file_path: file_path.to_string(),
            line_start,
            line_end: line_start,
            language: "rescript".to_string(),
            parent_name: parent,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({}),
        });
    }

    for capture in RESCRIPT_OPEN_RE.captures_iter(&cleaned) {
        let Some(kind) = capture.get(1) else {
            continue;
        };
        let Some(target) = capture.get(2) else {
            continue;
        };
        let Some(full) = capture.get(0) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: "IMPORTS_FROM".to_string(),
            source: file_path.to_string(),
            target: target.as_str().to_string(),
            file_path: file_path.to_string(),
            line: offset_to_line(&line_starts, full.start()),
            extra: json!({"rescript_import_kind": kind.as_str()}),
        });
    }

    for capture in RESCRIPT_MODULE_ALIAS_RE.captures_iter(&cleaned) {
        let Some(full) = capture.get(0) else {
            continue;
        };
        if modules
            .iter()
            .any(|module| module.start_off == full.start())
        {
            continue;
        }
        let Some(alias) = capture.get(1) else {
            continue;
        };
        let Some(target) = capture.get(2) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: "IMPORTS_FROM".to_string(),
            source: file_path.to_string(),
            target: target.as_str().to_string(),
            file_path: file_path.to_string(),
            line: offset_to_line(&line_starts, full.start()),
            extra: json!({
                "rescript_import_kind": "module_alias",
                "alias_name": alias.as_str(),
            }),
        });
    }

    if !is_interface {
        for capture in RESCRIPT_JSX_RE.captures_iter(&cleaned) {
            let Some(target_match) = capture.get(2) else {
                continue;
            };
            let target = target_match.as_str();
            let root = target.split('.').next().unwrap_or(target);
            let off = target_match.start();
            let line = offset_to_line(&line_starts, off);
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: file_path.to_string(),
                target: root.to_string(),
                file_path: file_path.to_string(),
                line,
                extra: json!({"rescript_import_kind": "jsx"}),
            });
            if let Some(entry) = rescript_enclosing_let(&lets, off) {
                edges.push(ParsedEdge {
                    kind: "CALLS".to_string(),
                    source: qualify(file_path, &entry.name, entry.parent.as_deref()),
                    target: target.to_string(),
                    file_path: file_path.to_string(),
                    line,
                    extra: json!({"rescript_call_kind": "jsx"}),
                });
            }
        }
    }

    if !is_interface && !lets.is_empty() {
        for capture in RESCRIPT_CALL_RE.captures_iter(&cleaned) {
            let Some(target_match) = capture.get(2) else {
                continue;
            };
            let target = target_match.as_str();
            let top = target.split('.').next().unwrap_or(target);
            if rescript_is_keyword(top) || rescript_is_keyword(target) {
                continue;
            }
            let (target, off) = expand_rescript_call_target(&cleaned, target, target_match.start());
            let top = target.split('.').next().unwrap_or(&target);
            if rescript_is_keyword(top) || rescript_is_keyword(&target) {
                continue;
            }
            let Some(entry) = rescript_enclosing_let(&lets, off) else {
                continue;
            };
            if entry.name == target && off == entry.start_off {
                continue;
            }
            edges.push(ParsedEdge {
                kind: "CALLS".to_string(),
                source: qualify(file_path, &entry.name, entry.parent.as_deref()),
                target,
                file_path: file_path.to_string(),
                line: offset_to_line(&line_starts, off),
                extra: json!({}),
            });
        }
    }

    for node in &nodes {
        if matches!(node.kind.as_str(), "Function" | "Type" | "Test") {
            if let Some(parent) = node.parent_name.as_deref() {
                edges.push(ParsedEdge {
                    kind: "CONTAINS".to_string(),
                    source: qualify(file_path, parent, None),
                    target: qualify(file_path, &node.name, Some(parent)),
                    file_path: file_path.to_string(),
                    line: node.line_start,
                    extra: json!({}),
                });
            }
        }
    }

    tag_rescript_js_binding_modules(&mut nodes);
    edges = dedupe_rescript_imports(edges);
    edges = resolve_rust_call_targets(&nodes, edges, file_path);
    if test_file {
        add_tested_by_edges(&nodes, &mut edges);
    }
    (nodes, edges)
}

#[derive(Clone, Debug)]
struct RescriptModule {
    name: String,
    start_off: usize,
    end_off: usize,
    body_start_off: usize,
    start_line: i64,
    end_line: i64,
    parent: Option<String>,
}

#[derive(Clone, Debug)]
struct RescriptLet {
    name: String,
    start_off: usize,
    line_start: i64,
    parent: Option<String>,
    is_test: bool,
    end_off: usize,
    line_end: i64,
}

fn strip_rescript_noise(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied().unwrap_or('\0');
        if c == '/' && next == '/' {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        if c == '/' && next == '*' {
            let mut depth = 1usize;
            out.push(' ');
            out.push(' ');
            i += 2;
            while i < chars.len() && depth > 0 {
                let c = chars[i];
                let next = chars.get(i + 1).copied().unwrap_or('\0');
                if c == '/' && next == '*' {
                    depth += 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else if c == '*' && next == '/' {
                    depth -= 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else {
                    out.push(if c == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            continue;
        }
        if c == '"' {
            out.push('"');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else {
                    out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            if i < chars.len() {
                out.push('"');
                i += 1;
            }
            continue;
        }
        if c == '`' {
            out.push('`');
            i += 1;
            while i < chars.len() && chars[i] != '`' {
                out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                i += 1;
            }
            if i < chars.len() {
                out.push('`');
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn rescript_brace_depth_array(cleaned: &str) -> Vec<usize> {
    let mut depth = vec![0; cleaned.len() + 1];
    let mut current = 0usize;
    for (idx, ch) in cleaned.char_indices() {
        depth[idx] = current;
        if ch == '{' {
            current += 1;
        } else if ch == '}' {
            current = current.saturating_sub(1);
        }
    }
    depth[cleaned.len()] = current;
    depth
}

fn scan_rescript_modules(cleaned: &str, line_starts: &[usize]) -> Vec<RescriptModule> {
    let alias_starts = RESCRIPT_MODULE_ALIAS_RE
        .captures_iter(cleaned)
        .filter_map(|capture| capture.get(0).map(|matched| matched.start()))
        .collect::<HashSet<_>>();
    let mut modules = Vec::new();
    for capture in RESCRIPT_MODULE_RE.captures_iter(cleaned) {
        let Some(full) = capture.get(0) else {
            continue;
        };
        if alias_starts.contains(&full.start()) {
            continue;
        }
        let Some(name) = capture.get(1) else {
            continue;
        };
        let Some(brace_rel) = cleaned[full.end()..].find('{') else {
            continue;
        };
        let brace_open = full.end() + brace_rel;
        if RESCRIPT_DEFINITION_START_RE.is_match(&cleaned[full.end()..brace_open]) {
            continue;
        }
        let mut brace_depth = 1usize;
        let mut brace_close = cleaned.len().saturating_sub(1);
        for (idx, ch) in cleaned[brace_open + 1..].char_indices() {
            let absolute = brace_open + 1 + idx;
            if ch == '{' {
                brace_depth += 1;
            } else if ch == '}' {
                brace_depth = brace_depth.saturating_sub(1);
                if brace_depth == 0 {
                    brace_close = absolute;
                    break;
                }
            }
        }
        modules.push(RescriptModule {
            name: name.as_str().to_string(),
            start_off: full.start(),
            end_off: brace_close,
            body_start_off: brace_open + 1,
            start_line: offset_to_line(line_starts, full.start()),
            end_line: offset_to_line(line_starts, brace_close),
            parent: None,
        });
    }
    modules
}

fn assign_rescript_module_parents(modules: &mut [RescriptModule]) {
    let snapshot = modules.to_vec();
    for module in modules {
        let mut parent_name = None;
        let mut parent_start = 0usize;
        let mut found_parent = false;
        for other in &snapshot {
            if other.start_off < module.start_off
                && other.end_off > module.end_off
                && (!found_parent || other.start_off > parent_start)
            {
                parent_name = Some(other.name.clone());
                parent_start = other.start_off;
                found_parent = true;
            }
        }
        module.parent = parent_name;
    }
}

fn collect_rescript_lets(
    file_path: &str,
    cleaned: &str,
    line_starts: &[usize],
    modules: &[RescriptModule],
    depth: &[usize],
) -> Vec<RescriptLet> {
    let mut entries = Vec::new();
    for capture in RESCRIPT_LET_RE.captures_iter(cleaned) {
        let Some(name_match) = capture.get(1) else {
            continue;
        };
        let name = name_match.as_str();
        if rescript_is_keyword(name) {
            continue;
        }
        let off = name_match.start();
        let parent = rescript_enclosing_module(modules, off);
        if !rescript_is_top_level(off, parent.as_deref(), modules, depth) {
            continue;
        }
        entries.push(RescriptLet {
            name: name.to_string(),
            start_off: off,
            line_start: offset_to_line(line_starts, off),
            parent,
            is_test: rescript_is_test_function(name, file_path),
            end_off: off + 1,
            line_end: offset_to_line(line_starts, off),
        });
    }
    entries.sort_by_key(|entry| entry.start_off);
    entries
}

fn fill_rescript_let_ends(
    entries: &mut [RescriptLet],
    cleaned: &str,
    line_starts: &[usize],
    modules: &[RescriptModule],
) {
    for idx in 0..entries.len() {
        let mut next = entries
            .get(idx + 1)
            .map(|entry| entry.start_off)
            .unwrap_or(cleaned.len());
        if let Some(parent) = entries[idx].parent.as_deref() {
            if let Some(module) = modules.iter().find(|module| {
                module.name == parent
                    && module.start_off <= entries[idx].start_off
                    && entries[idx].start_off <= module.end_off
            }) {
                next = next.min(module.end_off);
            }
        }
        entries[idx].end_off = next.max(entries[idx].start_off + 1);
        entries[idx].line_end = offset_to_line(line_starts, entries[idx].end_off - 1);
    }
}

fn rescript_enclosing_module(modules: &[RescriptModule], off: usize) -> Option<String> {
    let mut innermost_name = None;
    let mut innermost_start = 0usize;
    let mut found = false;
    for module in modules {
        if module.start_off <= off
            && off <= module.end_off
            && (!found || module.start_off > innermost_start)
        {
            innermost_name = Some(module.name.clone());
            innermost_start = module.start_off;
            found = true;
        }
    }
    innermost_name
}

fn rescript_is_top_level(
    off: usize,
    parent: Option<&str>,
    modules: &[RescriptModule],
    depth: &[usize],
) -> bool {
    let current_depth = depth.get(off).copied().unwrap_or(0);
    let Some(parent) = parent else {
        return current_depth == 0;
    };
    modules
        .iter()
        .find(|module| module.name == parent && module.start_off <= off && off <= module.end_off)
        .is_some_and(|module| {
            current_depth == depth.get(module.body_start_off).copied().unwrap_or(0)
        })
}

fn rescript_enclosing_let(entries: &[RescriptLet], off: usize) -> Option<&RescriptLet> {
    let mut found = None;
    for entry in entries {
        if entry.start_off <= off && off < entry.end_off {
            found = Some(entry);
        } else if entry.start_off > off {
            break;
        }
    }
    found
}

fn expand_rescript_call_target(cleaned: &str, target: &str, off: usize) -> (String, usize) {
    let bytes = cleaned.as_bytes();
    if off == 0 || bytes.get(off.wrapping_sub(1)) != Some(&b'.') {
        return (target.to_string(), off);
    }
    let mut start = off - 1;
    while start > 0 {
        let byte = bytes[start - 1];
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'\'' | b'.') {
            start -= 1;
        } else {
            break;
        }
    }
    let expanded = cleaned
        .get(start..off + target.len())
        .filter(|candidate| {
            candidate
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        })
        .unwrap_or(target);
    (expanded.to_string(), start)
}

fn tag_rescript_js_binding_modules(nodes: &mut [ParsedNode]) {
    let mut member_funcs: HashMap<String, Vec<bool>> = HashMap::new();
    for node in nodes.iter() {
        if node.kind == "Function" {
            if let Some(parent) = node.parent_name.as_deref() {
                member_funcs.entry(parent.to_string()).or_default().push(
                    node.extra.get("rescript_external").and_then(Value::as_bool) == Some(true),
                );
            }
        }
    }
    for node in nodes {
        if node.kind != "Class" {
            continue;
        }
        if member_funcs
            .get(&node.name)
            .is_some_and(|members| !members.is_empty() && members.iter().all(|value| *value))
        {
            node.extra = json!({"rescript_kind": "js_binding"});
        }
    }
}

fn dedupe_rescript_imports(edges: Vec<ParsedEdge>) -> Vec<ParsedEdge> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(edges.len());
    for edge in edges {
        if edge.kind == "IMPORTS_FROM" {
            let key = (edge.source.clone(), edge.target.clone());
            if !seen.insert(key) {
                continue;
            }
        }
        deduped.push(edge);
    }
    deduped
}

fn rescript_is_keyword(name: &str) -> bool {
    matches!(
        name,
        "let"
            | "rec"
            | "and"
            | "type"
            | "module"
            | "open"
            | "include"
            | "external"
            | "if"
            | "else"
            | "switch"
            | "when"
            | "match"
            | "fun"
            | "true"
            | "false"
            | "for"
            | "while"
            | "mutable"
            | "try"
            | "catch"
            | "throw"
            | "assert"
            | "lazy"
            | "do"
            | "in"
            | "of"
            | "as"
            | "exception"
            | "private"
            | "constraint"
            | "with"
            | "downto"
            | "to"
            | "unpack"
            | "async"
            | "await"
    )
}

fn rescript_is_test_file(file_path: &str) -> bool {
    is_test_file(file_path)
        || ends_with_ascii_ignore_case(file_path, "_test.res")
        || ends_with_ascii_ignore_case(file_path, "_test.resi")
        || contains_ascii_ignore_case(file_path, ".test.res")
        || contains_ascii_ignore_case(file_path, ".test.resi")
}

fn rescript_is_test_function(name: &str, file_path: &str) -> bool {
    starts_with_ascii_ignore_case(name, "test_")
        || name.starts_with("Test")
        || name.ends_with("_test")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("_spec")
        || (rescript_is_test_file(file_path)
            && matches!(
                name,
                "describe" | "it" | "test" | "beforeEach" | "afterEach" | "beforeAll" | "afterAll"
            ))
}

fn safe_str_slice(value: &str, start: usize, end: usize) -> Option<&str> {
    let start = previous_char_boundary(value, start.min(value.len()));
    let end = previous_char_boundary(value, end.min(value.len()));
    (start <= end).then(|| &value[start..end])
}

fn previous_char_boundary(value: &str, mut index: usize) -> usize {
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn offset_to_line(line_starts: &[usize], off: usize) -> i64 {
    match line_starts.binary_search(&off) {
        Ok(idx) => idx as i64 + 1,
        Err(idx) => idx as i64,
    }
}

struct SfcParserInputs<'a> {
    language: &'static str,
    sfc_parser: Option<&'a mut tree_sitter::Parser>,
    javascript_parser: Option<&'a mut tree_sitter::Parser>,
    typescript_parser: Option<&'a mut tree_sitter::Parser>,
    repo_root: Option<&'a Path>,
    caches: JavaScriptCaches<'a>,
}

fn parse_sfc_with_parsers(
    file_path: &str,
    source: &[u8],
    mut inputs: SfcParserInputs<'_>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: inputs.language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = inputs.sfc_parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            let mut cursor = root.walk();
            for child in root.children(&mut cursor) {
                if child.kind() != "script_element" {
                    continue;
                }
                let Some(raw_text_node) = sfc_direct_child(child, "raw_text") else {
                    continue;
                };
                let script_language = sfc_script_language(child, source);
                let script_parser = if script_language == "typescript" {
                    inputs.typescript_parser.as_deref_mut()
                } else {
                    inputs.javascript_parser.as_deref_mut()
                };
                let script_source = &source[raw_text_node.start_byte()..raw_text_node.end_byte()];
                let line_offset = raw_text_node.start_position().row as i64;
                let (script_nodes, script_edges) = parse_javascript_like_with_parser(
                    file_path,
                    script_source,
                    script_language,
                    script_parser,
                    inputs.repo_root,
                    inputs.caches,
                );

                nodes.extend(script_nodes.into_iter().skip(1).map(|mut node| {
                    node.line_start += line_offset;
                    node.line_end += line_offset;
                    node.language = inputs.language.to_string();
                    node
                }));
                edges.extend(script_edges.into_iter().map(|mut edge| {
                    edge.line += line_offset;
                    edge
                }));
            }
        }
    }

    (nodes, edges)
}

fn sfc_direct_child<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() == kind);
    found
}

fn sfc_script_language(node: tree_sitter::Node<'_>, source: &[u8]) -> &'static str {
    let Some(start_tag) = sfc_direct_child(node, "start_tag") else {
        return "javascript";
    };
    let mut cursor = start_tag.walk();
    for attr in start_tag.children(&mut cursor) {
        if attr.kind() != "attribute" {
            continue;
        }
        let Some(name) = javascript_child_text(attr, source, "attribute_name") else {
            continue;
        };
        if name != "lang" {
            continue;
        }
        if matches!(
            sfc_first_descendant_text(attr, source, &["attribute_value"]).as_deref(),
            Some("ts" | "typescript")
        ) {
            return "typescript";
        }
    }
    "javascript"
}

fn sfc_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = sfc_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

struct JavaScriptParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    language: &'static str,
    test_file: bool,
    defined_names: &'a HashSet<String>,
    import_map: &'a HashMap<String, String>,
    repo_root: Option<&'a Path>,
    caches: JavaScriptCaches<'a>,
}

fn javascript_walk_children(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "class" | "interface_declaration" => {
                if let Some(name) = javascript_named_child(
                    child,
                    context.source,
                    &["identifier", "type_identifier"],
                ) {
                    let qualified = qualify(context.file_path, &name, enclosing_class);
                    nodes.push(ParsedNode {
                        kind: "Class".to_string(),
                        name: name.clone(),
                        file_path: context.file_path.to_string(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: context.language.to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: javascript_class_extra(child.kind()),
                    });
                    edges.push(ParsedEdge {
                        kind: "CONTAINS".to_string(),
                        source: context.file_path.to_string(),
                        target: qualified.clone(),
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    emit_javascript_inheritance_edges(child, context, &qualified, edges);
                    javascript_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_declaration" | "method_definition" | "arrow_function" => {
                if javascript_emit_function_node(child, context, enclosing_class, nodes, edges) {
                    if let Some(name) = javascript_function_name(child, context.source) {
                        javascript_walk_children(
                            child,
                            context,
                            enclosing_class,
                            Some(&name),
                            nodes,
                            edges,
                        );
                    }
                    continue;
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                if javascript_emit_variable_functions(child, context, enclosing_class, nodes, edges)
                {
                    continue;
                }
            }
            "public_field_definition" => {
                if javascript_emit_field_function(child, context, enclosing_class, nodes, edges) {
                    continue;
                }
            }
            "import_statement" => {
                for target in javascript_import_targets(child, context.source) {
                    let resolved = resolve_javascript_module(
                        &target,
                        context.file_path,
                        context.repo_root,
                        context.caches,
                    )
                    .unwrap_or(target);
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target: resolved,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
                continue;
            }
            "call_expression" => {
                if javascript_emit_call(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) {
                    continue;
                }
            }
            "jsx_opening_element" | "jsx_self_closing_element" => {
                javascript_emit_jsx_component_call(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            "pair"
            | "assignment_expression"
            | "array"
            | "arguments"
            | "shorthand_property_identifier" => {
                javascript_emit_value_references(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        javascript_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn javascript_emit_function_node(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(name) = javascript_function_name(node, context.source) else {
        return false;
    };
    let is_test = is_javascript_test_function(&name, context.file_path);
    let qualified = qualify(context.file_path, &name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.clone(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: if node.kind() == "arrow_function" {
            None
        } else {
            javascript_child_text(node, context.source, "formal_parameters")
        },
        return_type: javascript_child_text(node, context.source, "type_annotation"),
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    let container = enclosing_class
        .map(|name| qualify(context.file_path, name, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: container,
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    true
}

fn javascript_emit_variable_functions(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let mut handled = false;
    let mut cursor = node.walk();
    for declarator in node.children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let mut name = None;
        let mut function_node = None;
        let mut declarator_cursor = declarator.walk();
        for child in declarator.children(&mut declarator_cursor) {
            if child.kind() == "identifier" && name.is_none() {
                name = Some(node_text(child, context.source));
            } else if is_javascript_function_value(child.kind()) {
                function_node = Some(child);
            }
        }
        let (Some(name), Some(function_node)) = (name, function_node) else {
            continue;
        };
        let is_test = is_javascript_test_function(&name, context.file_path);
        let qualified = qualify(context.file_path, &name, enclosing_class);
        nodes.push(ParsedNode {
            kind: if is_test { "Test" } else { "Function" }.to_string(),
            name: name.clone(),
            file_path: context.file_path.to_string(),
            line_start: node.start_position().row as i64 + 1,
            line_end: node.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: enclosing_class.map(str::to_string),
            params: javascript_child_text(function_node, context.source, "formal_parameters"),
            return_type: javascript_child_text(function_node, context.source, "type_annotation"),
            modifiers: None,
            is_test,
            extra: json!({}),
        });
        let container = enclosing_class
            .map(|class_name| qualify(context.file_path, class_name, None))
            .unwrap_or_else(|| context.file_path.to_string());
        edges.push(ParsedEdge {
            kind: "CONTAINS".to_string(),
            source: container,
            target: qualified,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
        javascript_walk_children(
            function_node,
            context,
            enclosing_class,
            Some(&name),
            nodes,
            edges,
        );
        handled = true;
    }
    handled
}

fn javascript_emit_field_function(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let mut name = None;
    let mut function_node = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "property_identifier" && name.is_none() {
            name = Some(node_text(child, context.source));
        } else if is_javascript_function_value(child.kind()) {
            function_node = Some(child);
        }
    }
    let (Some(name), Some(function_node)) = (name, function_node) else {
        return false;
    };
    let is_test = is_javascript_test_function(&name, context.file_path);
    let qualified = qualify(context.file_path, &name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.clone(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: javascript_child_text(function_node, context.source, "formal_parameters"),
        return_type: javascript_child_text(function_node, context.source, "type_annotation"),
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    let container = enclosing_class
        .map(|class_name| qualify(context.file_path, class_name, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: container,
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    javascript_walk_children(
        function_node,
        context,
        enclosing_class,
        Some(&name),
        nodes,
        edges,
    );
    true
}

fn javascript_emit_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(call_name) = javascript_call_name(node, context.source) else {
        return false;
    };
    let effective_call_name = if context.test_file && !is_test_runner_name(&call_name) {
        javascript_base_test_runner_name(node, context.source).unwrap_or_else(|| call_name.clone())
    } else {
        call_name.clone()
    };
    if context.test_file && is_test_runner_name(&effective_call_name) {
        let line = node.start_position().row as i64 + 1;
        let synthetic_name = match javascript_first_string_arg(node, context.source) {
            Some(description) if !description.is_empty() => {
                format!("{effective_call_name}:{description}@L{line}")
            }
            _ => format!("{effective_call_name}@L{line}"),
        };
        let qualified = qualify(context.file_path, &synthetic_name, enclosing_class);
        nodes.push(ParsedNode {
            kind: "Test".to_string(),
            name: synthetic_name.clone(),
            file_path: context.file_path.to_string(),
            line_start: line,
            line_end: node.end_position().row as i64 + 1,
            language: context.language.to_string(),
            parent_name: enclosing_class.map(str::to_string),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: true,
            extra: json!({}),
        });
        let container = enclosing_func
            .map(|func| qualify(context.file_path, func, enclosing_class))
            .unwrap_or_else(|| context.file_path.to_string());
        edges.push(ParsedEdge {
            kind: "CONTAINS".to_string(),
            source: container,
            target: qualified,
            file_path: context.file_path.to_string(),
            line,
            extra: json!({}),
        });
        javascript_walk_children(
            node,
            context,
            enclosing_class,
            Some(&synthetic_name),
            nodes,
            edges,
        );
        return true;
    }

    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    let target = resolve_javascript_call_target(&call_name, context);
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller.clone(),
        target,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = javascript_bridge_edge(node, context, &caller) {
        edges.push(edge);
    }
    false
}

fn javascript_emit_value_references(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    match node.kind() {
        "pair" => {
            if let Some(value) = javascript_pair_value_identifier(node, context.source) {
                javascript_emit_reference_if_known(node, context, &caller, &value, edges);
            }
        }
        "shorthand_property_identifier" => {
            let value = node_text(node, context.source);
            javascript_emit_reference_if_known(node, context, &caller, &value, edges);
        }
        "assignment_expression" => {
            if let Some(value) = javascript_last_identifier_child(node, context.source) {
                javascript_emit_reference_if_known(node, context, &caller, &value, edges);
            }
        }
        "array" | "arguments" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    let value = node_text(child, context.source);
                    javascript_emit_reference_if_known(child, context, &caller, &value, edges);
                }
            }
        }
        _ => {}
    }
}

fn javascript_emit_jsx_component_call(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = javascript_jsx_component_target(node, context) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller,
        target,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn javascript_jsx_component_target(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let (base_name, component_name) = javascript_jsx_component_reference(node, context.source)?;
    if let Some(base_name) = base_name {
        if let Some(module) = context.import_map.get(&base_name) {
            return resolve_javascript_imported_symbol(&component_name, module, context)
                .or(Some(component_name));
        }
        return Some(component_name);
    }
    Some(resolve_javascript_call_target(&component_name, context))
}

fn javascript_jsx_component_reference(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(Option<String>, String)> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let name = node_text(child, source);
                return looks_like_jsx_component_name(&name).then_some((None, name));
            }
            "member_expression" => {
                let component_name = javascript_rightmost_identifier(child, source)?;
                if !looks_like_jsx_component_name(&component_name) {
                    return None;
                }
                let base_name = javascript_leftmost_identifier(child, source);
                return Some((base_name, component_name));
            }
            _ => {}
        }
    }
    None
}

fn looks_like_jsx_component_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_uppercase())
}

fn javascript_emit_reference_if_known(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    caller: &str,
    name: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    if javascript_should_skip_value_reference(name)
        || (!context.defined_names.contains(name) && !context.import_map.contains_key(name))
    {
        return;
    }
    let target = resolve_javascript_call_target(name, context);
    edges.push(ParsedEdge {
        kind: "REFERENCES".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn collect_javascript_defined_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "class_declaration" | "class" | "interface_declaration" => {
            if let Some(name) =
                javascript_named_child(node, source, &["identifier", "type_identifier"])
            {
                names.insert(name);
            }
        }
        "function_declaration" | "method_definition" => {
            if let Some(name) = javascript_function_name(node, source) {
                names.insert(name);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = node.walk();
            for declarator in node.children(&mut cursor) {
                if declarator.kind() != "variable_declarator" {
                    continue;
                }
                if let Some(name) = javascript_variable_declarator_function_name(declarator, source)
                {
                    names.insert(name);
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_defined_names(child, source, names);
    }
}

fn resolve_javascript_call_target(name: &str, context: &JavaScriptParseContext<'_>) -> String {
    if context.defined_names.contains(name) {
        return qualify(context.file_path, name, None);
    }
    let Some(module) = context.import_map.get(name) else {
        return name.to_string();
    };
    resolve_javascript_imported_symbol(name, module, context).unwrap_or_else(|| name.to_string())
}

fn resolve_javascript_imported_symbol(
    symbol_name: &str,
    module: &str,
    context: &JavaScriptParseContext<'_>,
) -> Option<String> {
    let module_file =
        resolve_javascript_module(module, context.file_path, context.repo_root, context.caches)?;
    resolve_javascript_exported_symbol(
        &module_file,
        symbol_name,
        context.repo_root,
        context.caches,
        &mut HashSet::new(),
    )
    .or_else(|| Some(qualify(&module_file, symbol_name, None)))
}

fn resolve_javascript_exported_symbol(
    module_file: &str,
    symbol_name: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
    seen: &mut HashSet<(String, String)>,
) -> Option<String> {
    let key = (module_file.to_string(), symbol_name.to_string());
    if !seen.insert(key) {
        return None;
    }
    let index = javascript_export_index(module_file, repo_root, caches)?;
    if index.defined_names.contains(symbol_name) {
        return Some(qualify(module_file, symbol_name, None));
    }
    if let Some(target) = index.named_exports.get(symbol_name) {
        return match target {
            JavaScriptExportTarget::Local(original_name) => {
                Some(qualify(module_file, original_name, None))
            }
            JavaScriptExportTarget::External {
                module_file,
                symbol_name,
            } => resolve_javascript_exported_symbol(
                module_file,
                symbol_name,
                repo_root,
                caches,
                seen,
            )
            .or_else(|| Some(qualify(module_file, symbol_name, None))),
        };
    }
    for exported_module in &index.star_exports {
        if let Some(result) = resolve_javascript_exported_symbol(
            exported_module,
            symbol_name,
            repo_root,
            caches,
            seen,
        ) {
            return Some(result);
        }
    }
    None
}

fn javascript_export_index(
    module_file: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<JavaScriptExportIndex> {
    if let Some(cache) = caches.export {
        if let Some(cached) = cache.borrow().get(module_file).cloned() {
            return cached;
        }
    }
    let result = javascript_export_index_uncached(module_file, repo_root, caches);
    if let Some(cache) = caches.export {
        cache
            .borrow_mut()
            .insert(module_file.to_string(), result.clone());
    }
    result
}

fn javascript_export_index_uncached(
    module_file: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<JavaScriptExportIndex> {
    let source_path = repo_root
        .map(|root| root.join(module_file))
        .unwrap_or_else(|| PathBuf::from(module_file));
    let source = std::fs::read(&source_path).ok()?;
    let mut parser = new_javascript_module_parser(module_file)?;
    let tree = parser.parse(&source, None)?;
    let root = tree.root_node();

    let mut defined_names = HashSet::new();
    collect_javascript_defined_names(root, &source, &mut defined_names);
    let mut named_exports = HashMap::new();
    let mut star_exports = Vec::new();

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() != "export_statement" {
            continue;
        }
        let (export_clause, target_module, has_star_export) =
            javascript_export_statement_parts(child, &source);
        if let Some(export_clause) = export_clause {
            let mut clause_cursor = export_clause.walk();
            for spec in export_clause.children(&mut clause_cursor) {
                if spec.kind() != "export_specifier" {
                    continue;
                }
                let names = javascript_named_descendants(
                    spec,
                    &source,
                    &["identifier", "property_identifier"],
                );
                let Some(exported_name) = names.last() else {
                    continue;
                };
                let original_name = names.first().unwrap_or(exported_name);
                if let Some(target_module) = target_module.as_deref() {
                    if let Some(resolved_module) =
                        resolve_javascript_module(target_module, module_file, repo_root, caches)
                    {
                        named_exports.insert(
                            exported_name.clone(),
                            JavaScriptExportTarget::External {
                                module_file: resolved_module,
                                symbol_name: original_name.clone(),
                            },
                        );
                    }
                } else {
                    named_exports.insert(
                        exported_name.clone(),
                        JavaScriptExportTarget::Local(original_name.clone()),
                    );
                }
            }
        }

        if has_star_export {
            let Some(target_module) = target_module.as_deref() else {
                continue;
            };
            let Some(resolved_module) =
                resolve_javascript_module(target_module, module_file, repo_root, caches)
            else {
                continue;
            };
            star_exports.push(resolved_module);
        }
    }
    Some(JavaScriptExportIndex {
        defined_names,
        named_exports,
        star_exports,
    })
}

fn new_javascript_module_parser(module_file: &str) -> Option<tree_sitter::Parser> {
    if ends_with_ascii_ignore_case(module_file, ".tsx") {
        return new_tsx_parser();
    }
    if ends_with_ascii_ignore_case(module_file, ".ts") {
        return new_typescript_parser();
    }
    if ends_with_ascii_ignore_case(module_file, ".js")
        || ends_with_ascii_ignore_case(module_file, ".jsx")
        || ends_with_ascii_ignore_case(module_file, ".mjs")
    {
        return new_javascript_parser();
    }
    None
}

fn javascript_export_statement_parts<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
) -> (Option<tree_sitter::Node<'a>>, Option<String>, bool) {
    let mut export_clause = None;
    let mut target_module = None;
    let mut has_star_export = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "export_clause" => export_clause = Some(child),
            "string" => target_module = Some(decode_javascript_string_literal(child, source)),
            "*" => has_star_export = true,
            _ => {}
        }
    }
    (export_clause, target_module, has_star_export)
}

fn collect_javascript_import_map(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    import_map: &mut HashMap<String, String>,
) {
    if node.kind() == "import_statement" {
        if let Some(module) = javascript_import_targets(node, source).into_iter().next() {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "import_clause" {
                    collect_javascript_import_clause_names(child, source, &module, import_map);
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_import_map(child, source, import_map);
    }
}

fn collect_javascript_import_clause_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    module: &str,
    import_map: &mut HashMap<String, String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                import_map.insert(node_text(child, source), module.to_string());
            }
            "namespace_import" => {
                if let Some(name) = javascript_last_named_descendant(
                    child,
                    source,
                    &["identifier", "property_identifier"],
                ) {
                    import_map.insert(name, module.to_string());
                }
            }
            "named_imports" => collect_javascript_named_imports(child, source, module, import_map),
            _ => {}
        }
    }
}

fn collect_javascript_named_imports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    module: &str,
    import_map: &mut HashMap<String, String>,
) {
    let mut cursor = node.walk();
    for spec in node.children(&mut cursor) {
        if spec.kind() != "import_specifier" {
            continue;
        }
        if let Some(name) =
            javascript_last_named_descendant(spec, source, &["identifier", "property_identifier"])
        {
            import_map.insert(name, module.to_string());
        }
    }
}

fn javascript_last_named_descendant(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_last_named_descendant(child, source, kinds) {
            return Some(name);
        }
    }
    None
}

fn javascript_named_descendants(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut names = Vec::new();
    collect_javascript_named_descendants(node, source, kinds, &mut names);
    names
}

fn collect_javascript_named_descendants(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    names: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            names.push(node_text(child, source));
        }
        collect_javascript_named_descendants(child, source, kinds, names);
    }
}

fn javascript_variable_declarator_function_name(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    let mut name = None;
    let mut has_function = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" && name.is_none() {
            name = Some(node_text(child, source));
        } else if is_javascript_function_value(child.kind()) {
            has_function = true;
        }
    }
    has_function.then_some(name).flatten()
}

fn javascript_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    javascript_named_child(
        node,
        source,
        &["identifier", "property_identifier", "type_identifier"],
    )
}

fn javascript_named_child(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
    }
    None
}

fn javascript_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

fn javascript_class_extra(kind: &str) -> Value {
    if kind == "interface_declaration" {
        json!({"type_role": "interface", "is_abstract": true, "is_contract": true})
    } else {
        json!({"type_role": "class"})
    }
}

fn emit_javascript_inheritance_edges(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    qualified: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut bases = Vec::new();
    collect_javascript_bases(node, context.source, &mut bases);
    for (base, role) in bases {
        edges.push(ParsedEdge {
            kind: if role == "implements" {
                "IMPLEMENTS".to_string()
            } else {
                "INHERITS".to_string()
            },
            source: qualified.to_string(),
            target: base,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({"relationship_role": role, "syntax_source": node.kind()}),
        });
    }
}

fn collect_javascript_bases(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    bases: &mut Vec<(String, &'static str)>,
) {
    let role = match node.kind() {
        "extends_clause" => Some("extends"),
        "implements_clause" => Some("implements"),
        _ => None,
    };
    if let Some(role) = role {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if matches!(
                child.kind(),
                "identifier" | "type_identifier" | "nested_identifier"
            ) {
                bases.push((node_text(child, source), role));
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_bases(child, source, bases);
    }
}

fn javascript_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string" {
            let target = decode_javascript_string_literal(child, source);
            if !target.is_empty() {
                targets.push(target);
            }
        }
    }
    targets
}

fn resolve_javascript_module(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<String> {
    let key = (file_path.to_string(), module.to_string());
    if let Some(cache) = caches.module {
        if let Some(cached) = cache.borrow().get(&key).cloned() {
            return cached;
        }
    }
    let result = resolve_javascript_module_uncached(module, file_path, repo_root, caches);
    if let Some(cache) = caches.module {
        cache.borrow_mut().insert(key, result.clone());
    }
    result
}

fn resolve_javascript_module_uncached(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<String> {
    if !module.starts_with('.') {
        return resolve_javascript_alias(module, file_path, repo_root, caches);
    }
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let base = caller_dir.join(module);
    if javascript_module_candidate_is_file(&base, repo_root) {
        return javascript_module_candidate_path(base, repo_root);
    }
    for ext in [".ts", ".tsx", ".js", ".jsx", ".vue"] {
        let target = base.with_extension(ext.trim_start_matches('.'));
        if javascript_module_candidate_is_file(&target, repo_root) {
            return javascript_module_candidate_path(target, repo_root);
        }
    }
    if javascript_module_candidate_is_dir(&base, repo_root) {
        for ext in [".ts", ".tsx", ".js", ".jsx", ".vue"] {
            let target = base.join(format!("index{ext}"));
            if javascript_module_candidate_is_file(&target, repo_root) {
                return javascript_module_candidate_path(target, repo_root);
            }
        }
    }
    None
}

fn javascript_module_candidate_is_file(candidate: &Path, repo_root: Option<&Path>) -> bool {
    repo_root
        .map(|root| root.join(candidate).is_file())
        .unwrap_or_else(|| candidate.is_file())
}

fn javascript_module_candidate_is_dir(candidate: &Path, repo_root: Option<&Path>) -> bool {
    repo_root
        .map(|root| root.join(candidate).is_dir())
        .unwrap_or_else(|| candidate.is_dir())
}

fn javascript_module_candidate_path(
    candidate: PathBuf,
    repo_root: Option<&Path>,
) -> Option<String> {
    if let Some(repo_root) = repo_root {
        let relative = candidate
            .strip_prefix(repo_root)
            .ok()
            .unwrap_or(candidate.as_path());
        return Some(normalize_relative_path(relative));
    }
    candidate
        .canonicalize()
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

fn resolve_javascript_alias(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
    caches: JavaScriptCaches<'_>,
) -> Option<String> {
    let (tsconfig_path, config) = find_javascript_tsconfig(file_path, repo_root, caches.tsconfig)?;
    let compiler_options = config.get("compilerOptions")?;
    let paths = compiler_options.get("paths")?.as_object()?;
    let base_url = compiler_options
        .get("baseUrl")
        .and_then(Value::as_str)
        .unwrap_or("");
    let base_dir = tsconfig_path.parent().unwrap_or_else(|| Path::new(""));
    let base_dir = base_dir.join(base_url);

    let mut patterns = paths.iter().collect::<Vec<_>>();
    patterns.sort_by_key(|(pattern, _)| std::cmp::Reverse(javascript_alias_specificity(pattern)));
    for (pattern, replacements) in patterns {
        let Some(suffix) = javascript_alias_match(pattern, module) else {
            continue;
        };
        let Some(replacements) = replacements.as_array() else {
            continue;
        };
        for replacement in replacements {
            let Some(replacement) = replacement.as_str() else {
                continue;
            };
            let mapped = if replacement.contains('*') {
                replacement.replacen('*', &suffix, 1)
            } else {
                replacement.to_string()
            };
            let candidate = base_dir.join(mapped);
            if let Some(path) = probe_javascript_module_candidate(&candidate, repo_root) {
                return Some(path);
            }
        }
    }
    None
}

fn find_javascript_tsconfig(
    file_path: &str,
    repo_root: Option<&Path>,
    tsconfig_cache: Option<&JavaScriptTsconfigCache>,
) -> Option<(PathBuf, Value)> {
    let mut current = if let Some(repo_root) = repo_root {
        repo_root.join(file_path)
    } else {
        PathBuf::from(file_path)
    };
    current = current.parent()?.to_path_buf();
    if let Some(cache) = tsconfig_cache {
        if let Some(cached) = cache.borrow().get(&current).cloned() {
            return cached;
        }
    }
    let start_dir = current.clone();
    let result = find_javascript_tsconfig_uncached(current);
    if let Some(cache) = tsconfig_cache {
        cache.borrow_mut().insert(start_dir, result.clone());
    }
    result
}

fn find_javascript_tsconfig_uncached(mut current: PathBuf) -> Option<(PathBuf, Value)> {
    loop {
        for name in ["tsconfig.json", "tsconfig.app.json"] {
            let candidate = current.join(name);
            if candidate.is_file() {
                let value = read_javascript_tsconfig(&candidate)?;
                return Some((candidate, value));
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    None
}

fn read_javascript_tsconfig(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    let stripped = strip_jsonc_comments(&raw);
    serde_json::from_str(&stripped).ok()
}

fn strip_jsonc_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            out.push('"');
            i += 1;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                out.push(ch);
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    out.push(bytes[i] as char);
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    strip_json_trailing_commas(&out)
}

fn strip_json_trailing_commas(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b',' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && matches!(bytes[j], b'}' | b']') {
                i += 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn javascript_alias_specificity(pattern: &str) -> usize {
    pattern
        .split_once('*')
        .map(|(prefix, _)| prefix.len())
        .unwrap_or(pattern.len())
}

fn javascript_alias_match(pattern: &str, module: &str) -> Option<String> {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return (pattern == module).then(String::new);
    };
    if !module.starts_with(prefix) || !module.ends_with(suffix) {
        return None;
    }
    let end = module.len().saturating_sub(suffix.len());
    Some(module[prefix.len()..end].to_string())
}

fn probe_javascript_module_candidate(candidate: &Path, repo_root: Option<&Path>) -> Option<String> {
    if javascript_module_candidate_is_file(candidate, repo_root) {
        return javascript_module_candidate_path(candidate.to_path_buf(), repo_root);
    }
    for ext in [".ts", ".tsx", ".js", ".jsx", ".vue"] {
        let target = if candidate.extension().is_none() {
            candidate.with_extension(ext.trim_start_matches('.'))
        } else {
            PathBuf::from(format!("{}{}", candidate.to_string_lossy(), ext))
        };
        if javascript_module_candidate_is_file(&target, repo_root) {
            return javascript_module_candidate_path(target, repo_root);
        }
    }
    if javascript_module_candidate_is_dir(candidate, repo_root) {
        for ext in [".ts", ".tsx", ".js", ".jsx", ".vue"] {
            let target = candidate.join(format!("index{ext}"));
            if javascript_module_candidate_is_file(&target, repo_root) {
                return javascript_module_candidate_path(target, repo_root);
            }
        }
    }
    None
}

fn javascript_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = javascript_callee_node(node)?;
    match callee.kind() {
        "identifier" | "property_identifier" => Some(node_text(callee, source)),
        "member_expression" => javascript_rightmost_identifier(callee, source),
        _ => None,
    }
}

fn javascript_callee_node(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    let callee = node
        .children(&mut cursor)
        .find(|child| child.kind() != "arguments");
    callee
}

fn javascript_rightmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if matches!(
            child.kind(),
            "identifier" | "property_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_rightmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn javascript_leftmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "property_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = javascript_leftmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn javascript_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    javascript_callee_node(node)
        .map(|callee| node_text(callee, source).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn javascript_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &JavaScriptParseContext<'_>,
    caller: &str,
) -> Option<ParsedEdge> {
    let signature = javascript_call_signature(node, context.source)?;
    let (relationship_role, bridge_kind) = javascript_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) =
        match javascript_first_string_arg(node, context.source) {
            Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
            _ => (
                format!("<dynamic:{signature}@{}:{line}>", context.file_path),
                0.2,
                "LOW",
            ),
        };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": context.language,
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn javascript_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "child_process.exec"
        | "child_process.execFile"
        | "child_process.execSync"
        | "child_process.execFileSync"
        | "child_process.spawn"
        | "child_process.spawnSync"
        | "child_process.fork" => Some(("invokes_binary", "subprocess")),
        "fs.readFile" | "fs.readFileSync" | "fs.promises.readFile" => {
            Some(("reads_file", "file_io"))
        }
        "fs.writeFile" | "fs.writeFileSync" | "fs.promises.writeFile" => {
            Some(("writes_file", "file_io"))
        }
        _ => None,
    }
}

fn javascript_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if matches!(child.kind(), "string" | "template_string") {
            return Some(decode_javascript_string_literal(child, source));
        }
        return None;
    }
    None
}

fn decode_javascript_string_literal(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "string_fragment" | "template_chars") {
            return node_text(child, source);
        }
    }
    node_text(node, source)
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`')
        .to_string()
}

fn javascript_pair_value_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut seen_colon = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == ":" {
            seen_colon = true;
            continue;
        }
        if seen_colon && child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn javascript_last_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    children
        .into_iter()
        .rev()
        .find(|child| child.kind() == "identifier")
        .map(|child| node_text(child, source))
}

fn javascript_base_test_runner_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = javascript_callee_node(node)?;
    if callee.kind() != "member_expression" {
        return None;
    }
    let rightmost = javascript_rightmost_identifier(callee, source)?;
    if !matches!(
        rightmost.as_str(),
        "only" | "skip" | "each" | "todo" | "concurrent"
    ) {
        return None;
    }
    let mut cursor = callee.walk();
    for child in callee.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
        if child.kind() == "member_expression" {
            let mut inner = child.walk();
            for sub in child.children(&mut inner) {
                if sub.kind() == "identifier" {
                    return Some(node_text(sub, source));
                }
            }
        }
    }
    None
}

fn is_test_runner_name(name: &str) -> bool {
    matches!(name, "describe" | "it" | "test")
}

fn is_javascript_function_value(kind: &str) -> bool {
    matches!(kind, "arrow_function" | "function_expression" | "function")
}

fn is_javascript_test_function(name: &str, file_path: &str) -> bool {
    starts_with_ascii_ignore_case(name, "test_")
        || name.starts_with("Test")
        || name.ends_with("_test")
        || name.ends_with("_spec")
        || (is_javascript_test_file(file_path) && is_test_runner_name(name))
}

fn is_javascript_test_file(file_path: &str) -> bool {
    is_test_file(file_path)
        || ends_with_ascii_ignore_case(file_path, ".test.ts")
        || ends_with_ascii_ignore_case(file_path, ".spec.ts")
        || ends_with_ascii_ignore_case(file_path, ".test.js")
        || ends_with_ascii_ignore_case(file_path, ".spec.js")
}

fn javascript_should_skip_value_reference(name: &str) -> bool {
    matches!(
        name,
        "true"
            | "false"
            | "null"
            | "undefined"
            | "None"
            | "True"
            | "False"
            | "self"
            | "this"
            | "cls"
            | "super"
    ) || name.len() <= 1
        || name.bytes().all(|byte| !byte.is_ascii_lowercase())
}

fn add_tested_by_edges(nodes: &[ParsedNode], edges: &mut Vec<ParsedEdge>) {
    let test_qnames = nodes
        .iter()
        .filter(|node| node.is_test)
        .map(|node| qualify(&node.file_path, &node.name, node.parent_name.as_deref()))
        .collect::<HashSet<_>>();
    let tested_by = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && test_qnames.contains(&edge.source))
        .map(|edge| ParsedEdge {
            kind: "TESTED_BY".to_string(),
            source: edge.target.clone(),
            target: edge.source.clone(),
            file_path: edge.file_path.clone(),
            line: edge.line,
            extra: json!({}),
        })
        .collect::<Vec<_>>();
    edges.extend(tested_by);
}

fn rust_walk_children(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "struct_item" | "enum_item" | "impl_item" => {
                if let Some(name) = rust_type_name(child, context.source) {
                    let qualified = qualify(context.file_path, &name, enclosing_class);
                    nodes.push(ParsedNode {
                        kind: "Class".to_string(),
                        name: name.clone(),
                        file_path: context.file_path.to_string(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "rust".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: json!({"type_role": rust_type_role(child.kind())}),
                    });
                    edges.push(ParsedEdge {
                        kind: "CONTAINS".to_string(),
                        source: context.file_path.to_string(),
                        target: qualified,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    rust_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_item" => {
                if let Some(name) = rust_identifier_child(child, context.source) {
                    let qualified = qualify(context.file_path, &name, enclosing_class);
                    let params = rust_child_text(child, context.source, "parameters");
                    let is_test = is_test_function(&name, context.file_path, child, context.source);
                    nodes.push(ParsedNode {
                        kind: if is_test { "Test" } else { "Function" }.to_string(),
                        name: name.clone(),
                        file_path: context.file_path.to_string(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "rust".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params,
                        return_type: None,
                        modifiers: None,
                        is_test,
                        extra: json!({}),
                    });
                    let container = enclosing_class
                        .map(|name| qualify(context.file_path, name, None))
                        .unwrap_or_else(|| context.file_path.to_string());
                    edges.push(ParsedEdge {
                        kind: "CONTAINS".to_string(),
                        source: container,
                        target: qualified,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    rust_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "use_declaration" => {
                if let Some(target) = rust_use_target(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
            }
            "call_expression" | "macro_invocation" => {
                if let Some(call_name) = rust_call_name(child, context.source) {
                    let caller = enclosing_func
                        .map(|name| qualify(context.file_path, name, enclosing_class))
                        .unwrap_or_else(|| context.file_path.to_string());
                    edges.push(ParsedEdge {
                        kind: "CALLS".to_string(),
                        source: caller.clone(),
                        target: call_name.clone(),
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    if let Some(edge) = rust_bridge_edge(
                        child,
                        context.source,
                        context.file_path,
                        &caller,
                        &call_name,
                    ) {
                        edges.push(edge);
                    }
                }
            }
            "arguments" => {
                rust_emit_argument_references(
                    child,
                    context.source,
                    context.file_path,
                    enclosing_class,
                    enclosing_func,
                    context.defined_names,
                    edges,
                );
            }
            _ => {}
        }
        rust_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

struct RustParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    defined_names: &'a HashSet<String>,
}

fn collect_rust_defined_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "struct_item" | "enum_item" | "impl_item" => {
            if let Some(name) = rust_type_name(node, source) {
                names.insert(name);
            }
        }
        "function_item" => {
            if let Some(name) = rust_identifier_child(node, source) {
                names.insert(name);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_defined_names(child, source, names);
    }
}

fn rust_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "impl_item" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "type_identifier" {
                return Some(node_text(child, source));
            }
        }
        return None;
    }
    rust_identifier_child(node, source)
}

fn rust_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "type_identifier" | "field_identifier"
        ) {
            return Some(node_text(child, source));
        }
    }
    None
}

fn rust_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

fn rust_type_role(kind: &str) -> &'static str {
    match kind {
        "enum_item" => "enum",
        "impl_item" => "implementation",
        _ => "class",
    }
}

fn rust_use_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    Some(
        text.replace("use ", "")
            .trim_end_matches(';')
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn rust_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "scoped_identifier" => return Some(node_text(child, source)),
            "field_expression" => return rust_rightmost_identifier(child, source),
            _ => {}
        }
    }
    None
}

fn rust_rightmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if matches!(
            child.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = rust_rightmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn rust_emit_argument_references(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|name| qualify(file_path, name, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "identifier" {
            continue;
        }
        let name = node_text(child, source);
        if rust_should_skip_value_reference(&name) || !defined_names.contains(&name) {
            continue;
        }
        edges.push(ParsedEdge {
            kind: "REFERENCES".to_string(),
            source: caller.clone(),
            target: qualify(file_path, &name, None),
            file_path: file_path.to_string(),
            line: child.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn rust_should_skip_value_reference(name: &str) -> bool {
    matches!(
        name,
        "true"
            | "false"
            | "null"
            | "undefined"
            | "None"
            | "True"
            | "False"
            | "self"
            | "this"
            | "cls"
            | "super"
    ) || name.len() <= 1
        || name.bytes().all(|byte| !byte.is_ascii_lowercase())
}

fn rust_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let signature = rust_call_signature(node, source).unwrap_or_else(|| call_name.to_string());
    let (relationship_role, bridge_kind) = rust_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match rust_first_string_arg(node, source) {
        Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
        _ => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "rust",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn rust_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let signature = node
        .children(&mut cursor)
        .find(|child| child.kind() != "arguments")
        .map(|child| node_text(child, source).trim().to_string())
        .filter(|value| !value.is_empty());
    signature
}

fn rust_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "std::process::Command::new" | "Command::new" => Some(("invokes_binary", "subprocess")),
        "std::fs::read"
        | "std::fs::read_to_string"
        | "std::fs::File::open"
        | "fs::read"
        | "fs::read_to_string"
        | "File::open" => Some(("reads_file", "file_io")),
        "std::fs::write" | "std::fs::File::create" | "fs::write" | "File::create" => {
            Some(("writes_file", "file_io"))
        }
        "libloading::Library::new" | "Library::new" => Some(("loads_shared_library", "ffi")),
        _ => None,
    }
}

fn rust_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if matches!(child.kind(), "string_literal" | "raw_string_literal") {
            return Some(decode_rust_string_literal(child, source));
        }
        return None;
    }
    None
}

fn decode_rust_string_literal(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "string_content" | "string_fragment") {
            return node_text(child, source);
        }
    }
    node_text(node, source)
        .trim_matches('"')
        .trim_matches('`')
        .to_string()
}

pub fn parse_bash(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_bash_parser();
    parse_bash_with_parser(file_path, source, parser.as_mut(), None)
}

fn parse_bash_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "bash".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            bash_walk_children(
                root, source, file_path, repo_root, None, &mut nodes, &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn bash_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    repo_root: Option<&Path>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                if let Some(name) = bash_function_name(child, source) {
                    let qualified = qualify(file_path, &name, None);
                    nodes.push(ParsedNode {
                        kind: "Function".to_string(),
                        name: name.clone(),
                        file_path: file_path.to_string(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "bash".to_string(),
                        parent_name: None,
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: json!({}),
                    });
                    edges.push(ParsedEdge {
                        kind: "CONTAINS".to_string(),
                        source: file_path.to_string(),
                        target: qualified,
                        file_path: file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    bash_walk_children(
                        child,
                        source,
                        file_path,
                        repo_root,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "command" => {
                bash_emit_command(child, source, file_path, repo_root, enclosing_func, edges);
            }
            _ => {}
        }
        bash_walk_children(
            child,
            source,
            file_path,
            repo_root,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn bash_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let name = node
        .children(&mut cursor)
        .find(|child| child.kind() == "word")
        .map(|child| node_text(child, source));
    name
}

fn bash_emit_command(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    repo_root: Option<&Path>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(command_name) = bash_command_name(node, source) else {
        return;
    };
    if matches!(command_name.as_str(), "source" | ".") {
        if let Some(target) = bash_first_command_arg(node, source) {
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: file_path.to_string(),
                target: resolve_bash_source_target(&target, file_path, repo_root).unwrap_or(target),
                file_path: file_path.to_string(),
                line: node.start_position().row as i64 + 1,
                extra: json!({}),
            });
        }
        return;
    }

    let caller = enclosing_func
        .map(|func| qualify(file_path, func, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller,
        target: command_name,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn bash_command_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "command_name" {
            return Some(node_text(child, source).trim().to_string())
                .filter(|name| !name.is_empty());
        }
    }
    None
}

fn bash_first_command_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut seen_command = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "command_name" {
            seen_command = true;
            continue;
        }
        if seen_command && matches!(child.kind(), "word" | "string" | "raw_string") {
            let text = node_text(child, source);
            return Some(strip_matching_quotes(text.trim()).to_string())
                .filter(|arg| !arg.is_empty());
        }
    }
    None
}

fn resolve_bash_source_target(
    target: &str,
    file_path: &str,
    repo_root: Option<&Path>,
) -> Option<String> {
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    if let Some(repo_root) = repo_root {
        let candidate = repo_root.join(caller_dir).join(target);
        if candidate.is_file() {
            return candidate
                .strip_prefix(repo_root)
                .ok()
                .map(normalize_relative_path);
        }
        return None;
    }
    let candidate = caller_dir.join(target);
    candidate
        .is_file()
        .then(|| normalize_relative_path(&candidate))
}

fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && matches!(bytes[0], b'\'' | b'"')
        && bytes.last().is_some_and(|last| *last == bytes[0])
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

pub fn parse_go(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_go_parser();
    parse_go_with_parser(file_path, source, parser.as_mut())
}

fn parse_go_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "go".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let root = tree.root_node();
            go_walk_children(root, source, file_path, None, &mut nodes, &mut edges);
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn go_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_declaration" => {
                go_emit_imports(child, source, file_path, edges);
            }
            "type_declaration" => {
                go_emit_types(child, source, file_path, nodes, edges);
            }
            "function_declaration" | "method_declaration" => {
                if let Some((name, receiver)) = go_function_name_and_receiver(child, source) {
                    go_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        receiver.as_deref(),
                        nodes,
                        edges,
                    );
                    go_walk_children(child, source, file_path, Some(&name), nodes, edges);
                    continue;
                }
            }
            "call_expression" => {
                go_emit_call(child, source, file_path, enclosing_func, edges);
            }
            _ => {}
        }
        go_walk_children(child, source, file_path, enclosing_func, nodes, edges);
    }
}

fn go_emit_imports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        go_emit_imports(child, source, file_path, edges);
        if child.kind() == "interpreted_string_literal" {
            let target = strip_matching_quotes(node_text(child, source).trim()).to_string();
            if !target.is_empty() {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.to_string(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
            }
        }
    }
}

fn go_emit_types(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "type_spec" {
            continue;
        }
        let Some(name) = go_direct_child_text(child, source, "type_identifier") else {
            continue;
        };
        let qualified = qualify(file_path, &name, None);
        nodes.push(ParsedNode {
            kind: "Class".to_string(),
            name,
            file_path: file_path.to_string(),
            line_start: child.start_position().row as i64 + 1,
            line_end: child.end_position().row as i64 + 1,
            language: "go".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"type_role": "class"}),
        });
        edges.push(ParsedEdge {
            kind: "CONTAINS".to_string(),
            source: file_path.to_string(),
            target: qualified,
            file_path: file_path.to_string(),
            line: child.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn go_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    receiver: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, receiver);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "go".to_string(),
        parent_name: receiver.map(str::to_string),
        params: go_first_parameter_list(node, source),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    let container = receiver
        .map(|receiver| qualify(file_path, receiver, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: container,
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn go_function_name_and_receiver(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, Option<String>)> {
    if node.kind() == "function_declaration" {
        return go_direct_child_text(node, source, "identifier").map(|name| (name, None));
    }
    let name = go_direct_child_text(node, source, "field_identifier")?;
    let receiver = go_receiver_name(node, source);
    Some((name, receiver))
}

fn go_receiver_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let receiver_list = node
        .children(&mut cursor)
        .find(|child| child.kind() == "parameter_list")?;
    go_last_named_descendant(receiver_list, source, &["type_identifier"])
}

fn go_first_parameter_list(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let params = node
        .children(&mut cursor)
        .find(|child| child.kind() == "parameter_list")
        .map(|child| node_text(child, source));
    params
}

fn go_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some((call_name, signature)) = go_call_name_and_signature(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller.clone(),
        target: call_name,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = go_bridge_edge(node, source, file_path, &caller, &signature) {
        edges.push(edge);
    }
}

fn go_call_name_and_signature(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, String)> {
    let mut cursor = node.walk();
    let callee = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list")?;
    if callee.kind() == "identifier" {
        let name = node_text(callee, source);
        return Some((name.clone(), name));
    }
    if callee.kind() == "selector_expression" {
        let signature = node_text(callee, source);
        let name = go_last_named_descendant(callee, source, &["field_identifier", "identifier"])?;
        return Some((name, signature));
    }
    None
}

fn go_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "exec.Command" => ("invokes_binary", "subprocess"),
        "os.ReadFile" | "os.Open" => ("reads_file", "file_io"),
        "os.WriteFile" => ("writes_file", "file_io"),
        "plugin.Open" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match go_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "go",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn go_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if matches!(
            child.kind(),
            "interpreted_string_literal" | "raw_string_literal"
        ) {
            return Some(strip_matching_quotes(node_text(child, source).trim()).to_string());
        }
        return None;
    }
    None
}

fn go_direct_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

fn go_last_named_descendant(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            found = Some(node_text(child, source));
        }
        if let Some(name) = go_last_named_descendant(child, source, kinds) {
            found = Some(name);
        }
    }
    found
}

pub fn parse_java(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_java_parser();
    parse_java_with_parser(file_path, source, parser.as_mut(), None)
}

struct JavaParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    repo_root: Option<&'a Path>,
}

fn parse_java_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
    repo_root: Option<&Path>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "java".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let context = JavaParseContext {
                source,
                file_path,
                repo_root,
            };
            java_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn java_walk_children(
    node: tree_sitter::Node<'_>,
    context: &JavaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_declaration" => {
                java_emit_import(
                    child,
                    context.source,
                    context.file_path,
                    context.repo_root,
                    edges,
                );
            }
            "class_declaration" | "interface_declaration" | "enum_declaration" => {
                if let Some(name) = java_type_name(child, context.source) {
                    java_emit_type(
                        child,
                        context.source,
                        context.file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    java_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "method_declaration" | "constructor_declaration" => {
                if let Some(name) = java_function_name(child, context.source) {
                    java_emit_function(
                        child,
                        context.source,
                        context.file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    java_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "method_invocation" => {
                java_emit_call(
                    child,
                    context.source,
                    context.file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        java_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn java_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    repo_root: Option<&Path>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(import_target) = java_import_target(node, source) else {
        return;
    };
    let target =
        resolve_java_import_target(&import_target, file_path, repo_root).unwrap_or(import_target);
    edges.push(ParsedEdge {
        kind: "IMPORTS_FROM".to_string(),
        source: file_path.to_string(),
        target,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn java_import_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    let target = text
        .trim()
        .trim_start_matches("import")
        .trim()
        .trim_start_matches("static")
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string();
    (!target.is_empty()).then_some(target)
}

fn resolve_java_import_target(
    target: &str,
    file_path: &str,
    repo_root: Option<&Path>,
) -> Option<String> {
    if target.ends_with(".*") {
        return None;
    }
    java_resolve_module_to_file(target, file_path, repo_root).or_else(|| {
        target
            .rfind('.')
            .and_then(|dot| java_resolve_module_to_file(&target[..dot], file_path, repo_root))
    })
}

fn java_resolve_module_to_file(
    module: &str,
    file_path: &str,
    repo_root: Option<&Path>,
) -> Option<String> {
    let relative = module.replace('.', "/") + ".java";
    let caller_dir = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    if let Some(repo_root) = repo_root {
        let mut current = repo_root.join(caller_dir);
        loop {
            let candidate = current.join(&relative);
            if candidate.is_file() {
                return candidate
                    .strip_prefix(repo_root)
                    .ok()
                    .map(normalize_relative_path);
            }
            if !current.pop() {
                break;
            }
        }
        return None;
    }

    let mut current = caller_dir.to_path_buf();
    loop {
        let candidate = current.join(&relative);
        if candidate.is_file() {
            return Some(normalize_relative_path(&candidate));
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn java_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = java_type_role(node, source);
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "java".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|parent| qualify(file_path, parent, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for (base, role) in java_bases(node, source) {
        edges.push(ParsedEdge {
            kind: if role == "implements" {
                "IMPLEMENTS".to_string()
            } else {
                "INHERITS".to_string()
            },
            source: qualified.clone(),
            target: base,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": role,
                "syntax_source": node.kind(),
            }),
        });
    }
}

fn java_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    java_direct_child_text(node, source, &["identifier", "type_identifier"])
}

fn java_type_role(node: tree_sitter::Node<'_>, source: &[u8]) -> (&'static str, bool, bool) {
    if node.kind() == "interface_declaration" {
        return ("interface", true, true);
    }
    if node.kind() == "enum_declaration" {
        return ("enum", false, false);
    }
    let is_abstract = java_direct_child_text(node, source, &["modifiers"])
        .is_some_and(|mods| mods.split_whitespace().any(|part| part == "abstract"));
    if is_abstract {
        ("abstract_class", true, false)
    } else {
        ("class", false, false)
    }
}

fn java_bases(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<(String, &'static str)> {
    let mut bases = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "superclass" => java_collect_type_names(child, source, "extends", &mut bases),
            "super_interfaces" => {
                java_collect_type_names(child, source, "implements", &mut bases);
            }
            "extends_interfaces" => java_collect_type_names(child, source, "extends", &mut bases),
            _ => {}
        }
    }
    bases
}

fn java_collect_type_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    role: &'static str,
    bases: &mut Vec<(String, &'static str)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "type_identifier" | "generic_type") {
            bases.push((node_text(child, source), role));
        } else {
            java_collect_type_names(child, source, role, bases);
        }
    }
}

fn java_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "java".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: java_direct_child_text(node, source, &["formal_parameters"]),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn java_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    java_direct_child_text(node, source, &["identifier"])
}

fn java_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());

    if let Some(call_name) = java_call_name(node, source) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }

    if let Some(signature) = java_call_signature(node, source) {
        if let Some(edge) = java_bridge_edge(node, source, file_path, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn java_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let first = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list")?;
    matches!(first.kind(), "identifier").then(|| node_text(first, source))
}

fn java_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "argument_list" {
            break;
        }
        parts.push(node_text(child, source));
    }
    let signature = parts.join("").trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn java_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Runtime.getRuntime().exec" | "Runtime.exec" => ("invokes_binary", "subprocess"),
        "System.loadLibrary"
        | "System.load"
        | "Runtime.getRuntime().loadLibrary"
        | "Runtime.getRuntime().load" => ("loads_shared_library", "ffi"),
        "Files.readString" | "Files.readAllBytes" => ("reads_file", "file_io"),
        "Files.writeString" | "Files.write" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match java_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "java",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn java_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string_literal" {
            return Some(java_string_text(child, source));
        }
        return None;
    }
    None
}

fn java_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_fragment" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn java_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
    }
    None
}

pub fn parse_ruby(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_ruby_parser();
    parse_ruby_with_parser(file_path, source, parser.as_mut())
}

fn parse_ruby_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "ruby".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            ruby_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn ruby_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "module" | "class" => {
                if let Some(name) = ruby_class_name(child, source) {
                    ruby_emit_class(child, file_path, &name, enclosing_class, nodes, edges);
                    ruby_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "method" | "singleton_method" => {
                if let Some(name) = ruby_method_name(child, source) {
                    ruby_emit_function(child, file_path, &name, enclosing_class, nodes, edges);
                    ruby_walk_children(
                        child,
                        source,
                        file_path,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "call" | "method_call" => {
                ruby_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        ruby_walk_children(
            child,
            source,
            file_path,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn ruby_emit_class(
    node: tree_sitter::Node<'_>,
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "ruby".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualify(file_path, name, enclosing_class),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn ruby_emit_function(
    node: tree_sitter::Node<'_>,
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "ruby".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn ruby_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let call_name = ruby_call_name(node, source);
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    if let Some(call_name) = call_name {
        if call_name == "require" || call_name == "require_relative" {
            if let Some(target) = ruby_first_string_arg(node, source) {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.to_string(),
                    line: node.start_position().row as i64 + 1,
                    extra: json!({}),
                });
            }
        }
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = ruby_call_signature(node, source) {
        if let Some(edge) = ruby_bridge_edge(node, source, file_path, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn ruby_class_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    ruby_direct_child_text(node, source, &["constant"])
}

fn ruby_method_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    ruby_direct_child_text(node, source, &["identifier"])
}

fn ruby_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).find(|child| {
        !matches!(
            child.kind(),
            "argument_list" | "do_block" | "block" | "." | "::" | "&."
        )
    })?;
    matches!(first.kind(), "identifier").then(|| node_text(first, source))
}

fn ruby_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "argument_list" | "do_block" | "block") {
            break;
        }
        parts.push(node_text(child, source));
    }
    let signature = parts.join("").trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn ruby_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "system" | "exec" | "spawn" | "Kernel.system" | "Process.spawn" | "IO.popen"
        | "Open3.capture3" | "Open3.popen3" => ("invokes_binary", "subprocess"),
        "File.read" | "File.readlines" | "IO.read" => ("reads_file", "file_io"),
        "File.write" | "IO.write" => ("writes_file", "file_io"),
        "File.open" => ("opens_file", "file_io"),
        "Fiddle.dlopen" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match ruby_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "ruby",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn ruby_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string" {
            return Some(ruby_string_text(child, source));
        }
        return None;
    }
    None
}

fn ruby_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn ruby_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
    }
    None
}

pub fn parse_csharp(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_csharp_parser();
    parse_csharp_with_parser(file_path, source, parser.as_mut())
}

fn parse_csharp_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "csharp".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            csharp_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn csharp_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "using_directive" => {
                csharp_emit_import(child, source, file_path, edges);
            }
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "struct_declaration" => {
                if let Some(name) = csharp_type_name(child, source) {
                    csharp_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    csharp_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "method_declaration" | "constructor_declaration" => {
                if let Some(name) = csharp_function_name(child, source) {
                    csharp_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    csharp_walk_children(
                        child,
                        source,
                        file_path,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "invocation_expression" | "object_creation_expression" => {
                csharp_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        csharp_walk_children(
            child,
            source,
            file_path,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn csharp_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let text = node_text(node, source);
    let target = text
        .trim()
        .trim_start_matches("using")
        .trim()
        .trim_end_matches(';')
        .trim();
    if target.is_empty() {
        return;
    }
    edges.push(ParsedEdge {
        kind: "IMPORTS_FROM".to_string(),
        source: file_path.to_string(),
        target: target.to_string(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn csharp_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = csharp_type_role(node, source);
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
    }
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "csharp".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualify(file_path, name, enclosing_class),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn csharp_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    csharp_direct_child_text(node, source, &["identifier"])
}

fn csharp_type_role(node: tree_sitter::Node<'_>, source: &[u8]) -> (&'static str, bool, bool) {
    match node.kind() {
        "interface_declaration" => ("interface", true, true),
        "enum_declaration" => ("enum", false, false),
        "struct_declaration" => ("struct", false, false),
        _ => {
            let is_abstract = csharp_direct_child_text(node, source, &["modifier"])
                .is_some_and(|modifier| modifier == "abstract");
            if is_abstract {
                ("abstract_class", true, false)
            } else {
                ("class", false, false)
            }
        }
    }
}

fn csharp_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "csharp".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: csharp_direct_child_text(node, source, &["parameter_list"]),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn csharp_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    csharp_direct_child_text(node, source, &["identifier"])
}

fn csharp_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    if let Some(call_name) = csharp_call_name(node, source) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = csharp_call_signature(node, source) {
        if let Some(edge) = csharp_bridge_edge(node, source, file_path, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn csharp_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let first = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list")?;
    matches!(first.kind(), "identifier").then(|| node_text(first, source))
}

fn csharp_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let callee = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list")?;
    let signature = node_text(callee, source).trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn csharp_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Process.Start" | "System.Diagnostics.Process.Start" => ("invokes_binary", "subprocess"),
        "File.ReadAllText" | "File.ReadAllBytes" | "File.ReadAllLines" | "File.OpenRead" => {
            ("reads_file", "file_io")
        }
        "File.WriteAllText" | "File.WriteAllBytes" | "File.OpenWrite" | "File.Create" => {
            ("writes_file", "file_io")
        }
        "Assembly.LoadFile" | "NativeLibrary.Load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match csharp_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "csharp",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn csharp_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        let arg = if child.kind() == "argument" {
            csharp_first_non_punctuation_child(child).unwrap_or(child)
        } else {
            child
        };
        if arg.kind() == "string_literal" {
            return Some(csharp_string_text(arg, source));
        }
        return None;
    }
    None
}

fn csharp_first_non_punctuation_child(
    node: tree_sitter::Node<'_>,
) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    let child = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")"));
    child
}

fn csharp_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_literal_content" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn csharp_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
    }
    None
}

pub fn parse_php(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_php_parser();
    parse_php_with_parser(file_path, source, parser.as_mut())
}

fn parse_php_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "php".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            php_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn php_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "namespace_use_declaration" => {
                php_emit_import(child, source, file_path, edges);
            }
            "class_declaration" | "interface_declaration" => {
                if let Some(name) = php_direct_child_text(child, source, &["name"]) {
                    php_emit_type(child, file_path, &name, enclosing_class, nodes, edges);
                    php_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_definition" | "method_declaration" => {
                if let Some(name) = php_direct_child_text(child, source, &["name"]) {
                    php_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    php_walk_children(
                        child,
                        source,
                        file_path,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "function_call_expression"
            | "member_call_expression"
            | "nullsafe_member_call_expression"
            | "scoped_call_expression" => {
                php_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        php_walk_children(
            child,
            source,
            file_path,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn php_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    edges.push(ParsedEdge {
        kind: "IMPORTS_FROM".to_string(),
        source: file_path.to_string(),
        target: node_text(node, source).trim().to_string(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn php_emit_type(
    node: tree_sitter::Node<'_>,
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = if node.kind() == "interface_declaration" {
        ("interface", true, true)
    } else {
        ("class", false, false)
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
    }
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "php".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualify(file_path, name, enclosing_class),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn php_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "php".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: php_direct_child_text(node, source, &["formal_parameters"]),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn php_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(call_name) = php_call_name(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller.clone(),
        target: call_name.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = php_bridge_edge(node, source, file_path, &caller, &call_name) {
        edges.push(edge);
    }
}

fn php_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_call_expression" => {
            php_direct_child_text(node, source, &["name", "qualified_name"])
                .map(|name| name.trim_start_matches('\\').to_string())
        }
        "member_call_expression" | "nullsafe_member_call_expression" => {
            php_last_direct_child_text(node, source, "name")
        }
        "scoped_call_expression" => {
            let names = php_direct_child_texts(node, source, &["name"]);
            if names.len() >= 2 {
                return Some(format!("{}::{}", names[0], names[1]));
            }
            if let Some(scope) = php_direct_child_text(node, source, &["relative_scope"]) {
                if matches!(scope.as_str(), "parent" | "self") {
                    return names.last().cloned();
                }
            }
            names.last().cloned()
        }
        _ => None,
    }
}

fn php_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "exec" | "shell_exec" | "system" | "passthru" | "proc_open" | "popen" => {
            ("invokes_binary", "subprocess")
        }
        "file_get_contents" | "fread" | "readfile" => ("reads_file", "file_io"),
        "file_put_contents" | "fwrite" => ("writes_file", "file_io"),
        "fopen" => ("opens_file", "file_io"),
        "FFI::cdef" | "FFI::load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match php_first_string_arg(node, source) {
        Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
        _ => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "php",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn php_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        let arg = if child.kind() == "argument" {
            php_first_non_punctuation_child(child).unwrap_or(child)
        } else {
            child
        };
        if matches!(arg.kind(), "encapsed_string" | "string") {
            return Some(php_string_text(arg, source));
        }
        return None;
    }
    None
}

fn php_first_non_punctuation_child(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    let child = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")"));
    child
}

fn php_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn php_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
    }
    None
}

fn php_last_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kind: &str,
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            found = Some(node_text(child, source));
        }
    }
    found
}

fn php_direct_child_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
    }
    out
}

pub fn parse_kotlin(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_kotlin_parser();
    parse_kotlin_with_parser(file_path, source, parser.as_mut())
}

fn parse_kotlin_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "kotlin".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            kotlin_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn kotlin_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_header" => {
                kotlin_emit_import(child, source, file_path, edges);
            }
            "class_declaration" => {
                if let Some(name) = kotlin_direct_child_text(child, source, &["type_identifier"]) {
                    kotlin_emit_type(child, file_path, &name, enclosing_class, nodes, edges);
                    kotlin_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_declaration" => {
                if let Some(name) = kotlin_direct_child_text(child, source, &["simple_identifier"])
                {
                    kotlin_emit_function(child, file_path, &name, enclosing_class, nodes, edges);
                    kotlin_walk_children(
                        child,
                        source,
                        file_path,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "call_expression" => {
                kotlin_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        kotlin_walk_children(
            child,
            source,
            file_path,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn kotlin_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    edges.push(ParsedEdge {
        kind: "IMPORTS_FROM".to_string(),
        source: file_path.to_string(),
        target: node_text(node, source).trim().to_string(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn kotlin_emit_type(
    node: tree_sitter::Node<'_>,
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "kotlin".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "INHERITS".to_string(),
        source: qualified,
        target: name.to_string(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({
            "relationship_role": "extends",
            "syntax_source": "class_declaration",
        }),
    });
}

fn kotlin_emit_function(
    node: tree_sitter::Node<'_>,
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "kotlin".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn kotlin_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    if let Some(call_name) = kotlin_call_name(node, source) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = kotlin_call_signature(node, source) {
        if let Some(edge) = kotlin_bridge_edge(node, source, file_path, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn kotlin_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = kotlin_call_callee(node)?;
    if callee.kind() == "simple_identifier" {
        return Some(node_text(callee, source));
    }
    kotlin_last_descendant_text(callee, source, &["simple_identifier", "type_identifier"])
}

fn kotlin_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = kotlin_call_callee(node)?;
    let signature = node_text(callee, source).trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn kotlin_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "call_suffix");
    found
}

fn kotlin_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Runtime.getRuntime().exec" | "ProcessBuilder.start" => ("invokes_binary", "subprocess"),
        "System.loadLibrary" | "System.load" => ("loads_shared_library", "ffi"),
        "Files.readString"
        | "Files.readAllBytes"
        | "File.readText"
        | "File.readLines"
        | "File.bufferedReader" => ("reads_file", "file_io"),
        "Files.writeString" | "Files.write" | "File.writeText" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match kotlin_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "kotlin",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn kotlin_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let suffix = kotlin_direct_child(node, &["call_suffix"])?;
    let arguments = kotlin_first_descendant(suffix, &["value_arguments"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        let arg = if child.kind() == "value_argument" {
            kotlin_first_non_punctuation_child(child).unwrap_or(child)
        } else {
            child
        };
        if arg.kind() == "string_literal" {
            return Some(kotlin_string_text(arg, source));
        }
        return None;
    }
    None
}

fn kotlin_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    if let Some(content) = kotlin_first_descendant(node, &["string_content"]) {
        return node_text(content, source);
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn kotlin_first_non_punctuation_child<'a>(
    node: tree_sitter::Node<'a>,
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")"));
    found
}

fn kotlin_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn kotlin_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    kotlin_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn kotlin_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = kotlin_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn kotlin_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            found = Some(node_text(child, source));
        }
        if let Some(value) = kotlin_last_descendant_text(child, source, kinds) {
            found = Some(value);
        }
    }
    found
}

pub fn parse_scala(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_scala_parser();
    parse_scala_with_parser(file_path, source, parser.as_mut())
}

fn parse_scala_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "scala".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            scala_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn scala_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_declaration" => {
                scala_emit_imports(child, source, file_path, edges);
            }
            "trait_definition" | "class_definition" | "object_definition" | "enum_definition" => {
                if let Some(name) = scala_direct_child_text(child, source, &["identifier"]) {
                    scala_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    scala_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_definition" | "function_declaration" => {
                if let Some(name) = scala_direct_child_text(child, source, &["identifier"]) {
                    scala_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    scala_walk_children(
                        child,
                        source,
                        file_path,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "call_expression" => {
                scala_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            "instance_expression" => {
                scala_emit_instance_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        scala_walk_children(
            child,
            source,
            file_path,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn scala_emit_imports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    for target in scala_import_targets(node, source) {
        edges.push(ParsedEdge {
            kind: "IMPORTS_FROM".to_string(),
            source: file_path.to_string(),
            target,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn scala_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let text = node_text(node, source);
    let import = text.trim().trim_start_matches("import").trim();
    if let (Some(open), Some(close)) = (import.find('{'), import.rfind('}')) {
        let prefix = import[..open].trim_end_matches('.').trim();
        return import[open + 1..close]
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| format!("{prefix}.{}", scala_normalize_import_selector(item)))
            .collect();
    }
    vec![scala_normalize_import_selector(import)]
}

fn scala_normalize_import_selector(value: &str) -> String {
    value
        .strip_suffix("._")
        .map(|prefix| format!("{prefix}.*"))
        .unwrap_or_else(|| value.to_string())
}

fn scala_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = match node.kind() {
        "trait_definition" => ("trait", true, true),
        "enum_definition" => ("enum", false, false),
        _ => ("class", false, false),
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "scala".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if node.kind() == "class_definition" {
        for (idx, target) in scala_inheritance_targets(node, source)
            .into_iter()
            .enumerate()
        {
            edges.push(ParsedEdge {
                kind: if idx == 0 { "INHERITS" } else { "IMPLEMENTS" }.to_string(),
                source: qualified.clone(),
                target,
                file_path: file_path.to_string(),
                line: node.start_position().row as i64 + 1,
                extra: json!({
                    "relationship_role": if idx == 0 { "extends" } else { "implements" },
                    "syntax_source": "class_definition",
                }),
            });
        }
    }
}

fn scala_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "scala".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: scala_direct_child_text(node, source, &["parameters"]),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn scala_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    if let Some(call_name) = scala_call_name(node, source) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = scala_call_signature(node, source) {
        if let Some(edge) = scala_bridge_edge(node, source, file_path, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn scala_emit_instance_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = scala_first_descendant_text(node, source, &["type_identifier"]) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller,
        target,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn scala_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = scala_call_callee(node)?;
    if callee.kind() == "identifier" {
        return Some(node_text(callee, source));
    }
    if callee.kind() == "generic_function" {
        if let Some(function) = scala_direct_child(callee, &["field_expression", "identifier"]) {
            return scala_last_descendant_text(
                function,
                source,
                &["identifier", "type_identifier"],
            )
            .or_else(|| Some(node_text(function, source)));
        }
    }
    scala_last_descendant_text(callee, source, &["identifier", "type_identifier"])
}

fn scala_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = scala_call_callee(node)?;
    let signature = node_text(callee, source).trim().to_string();
    (!signature.is_empty()).then_some(signature)
}

fn scala_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "arguments");
    found
}

fn scala_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "Runtime.getRuntime().exec" | "scala.sys.process.Process" => {
            ("invokes_binary", "subprocess")
        }
        "System.loadLibrary" | "System.load" => ("loads_shared_library", "ffi"),
        "Files.readString" | "Files.readAllBytes" | "scala.io.Source.fromFile" => {
            ("reads_file", "file_io")
        }
        "Files.writeString" | "Files.write" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match scala_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "scala",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn scala_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = scala_direct_child(node, &["arguments"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string" {
            return Some(strip_matching_quotes(node_text(child, source).trim()).to_string());
        }
        return None;
    }
    None
}

fn scala_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(extends) = scala_direct_child(node, &["extends_clause"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = extends.walk();
    for child in extends.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => out.push(node_text(child, source)),
            "generic_type" => {
                if let Some(target) =
                    scala_first_descendant_text(child, source, &["type_identifier"])
                {
                    out.push(target);
                }
            }
            _ => {}
        }
    }
    out
}

fn scala_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn scala_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    scala_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn scala_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = scala_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn scala_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            found = Some(node_text(child, source));
        }
        if let Some(value) = scala_last_descendant_text(child, source, kinds) {
            found = Some(value);
        }
    }
    found
}

pub fn parse_solidity(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_solidity_parser();
    parse_solidity_with_parser(file_path, source, parser.as_mut())
}

fn parse_solidity_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "solidity".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            solidity_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn solidity_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_directive" => {
                solidity_emit_import(child, source, file_path, edges);
            }
            "contract_declaration"
            | "interface_declaration"
            | "library_declaration"
            | "struct_declaration"
            | "enum_declaration"
            | "error_declaration"
            | "user_defined_type_definition" => {
                if let Some(name) = solidity_direct_child_text(child, source, &["identifier"]) {
                    solidity_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    solidity_walk_children(
                        child,
                        source,
                        file_path,
                        Some(&name),
                        None,
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "constant_variable_declaration" => {
                if solidity_emit_constant(child, source, file_path, enclosing_class, nodes, edges) {
                    continue;
                }
            }
            "state_variable_declaration" if enclosing_class.is_some() => {
                if solidity_emit_state_variable(
                    child,
                    source,
                    file_path,
                    enclosing_class.unwrap_or_default(),
                    nodes,
                    edges,
                ) {
                    continue;
                }
            }
            "function_definition"
            | "constructor_definition"
            | "modifier_definition"
            | "event_definition"
            | "fallback_receive_definition" => {
                if let Some(name) = solidity_function_name(child, source) {
                    solidity_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    solidity_emit_modifier_invocation_calls(
                        child,
                        source,
                        file_path,
                        &qualify(file_path, &name, enclosing_class),
                        edges,
                    );
                    solidity_walk_children(
                        child,
                        source,
                        file_path,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "using_directive" => {
                solidity_emit_using(child, source, file_path, enclosing_class, edges);
                continue;
            }
            "emit_statement" => {
                solidity_emit_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            "call_expression" => {
                solidity_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        solidity_walk_children(
            child,
            source,
            file_path,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn solidity_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string" {
            let target = strip_matching_quotes(node_text(child, source).trim()).to_string();
            if !target.is_empty() {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.to_string(),
                    line: node.start_position().row as i64 + 1,
                    extra: json!({}),
                });
            }
        }
    }
}

fn solidity_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = match node.kind() {
        "interface_declaration" => ("interface", true, true),
        "struct_declaration" => ("struct", false, false),
        "enum_declaration" => ("enum", false, false),
        _ => ("class", false, false),
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "solidity".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for target in solidity_inheritance_targets(node, source) {
        edges.push(ParsedEdge {
            kind: "INHERITS".to_string(),
            source: qualified.clone(),
            target,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": "extends",
                "syntax_source": "contract_declaration",
            }),
        });
    }
}

fn solidity_emit_constant(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(name) = solidity_direct_child_text(node, source, &["identifier"]) else {
        return false;
    };
    let qualified = qualify(file_path, &name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.clone(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "solidity".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: solidity_direct_child_text(node, source, &["type_name"]),
        modifiers: None,
        is_test: false,
        extra: json!({"solidity_kind": "constant"}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    true
}

fn solidity_emit_state_variable(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(name) = solidity_direct_child_text(node, source, &["identifier"]) else {
        return false;
    };
    let qualified = qualify(file_path, &name, Some(enclosing_class));
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.clone(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "solidity".to_string(),
        parent_name: Some(enclosing_class.to_string()),
        params: None,
        return_type: solidity_direct_child_text(node, source, &["type_name"]),
        modifiers: solidity_direct_child_text(node, source, &["visibility"]),
        is_test: false,
        extra: json!({
            "solidity_kind": "state_variable",
            "mutability": solidity_direct_child_kind(node, &["constant", "immutable"]),
        }),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: qualify(file_path, enclosing_class, None),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    true
}

fn solidity_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "solidity".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: solidity_params(node, source),
        return_type: solidity_direct_child_text(node, source, &["return_type_definition"]),
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn solidity_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(call_name) = solidity_call_name(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller,
        target: call_name,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn solidity_emit_modifier_invocation_calls(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier_invocation" {
            if let Some(name) = solidity_first_descendant_text(child, source, &["identifier"]) {
                edges.push(ParsedEdge {
                    kind: "CALLS".to_string(),
                    source: caller.to_string(),
                    target: name,
                    file_path: file_path.to_string(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
            }
        }
    }
}

fn solidity_emit_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(name) = solidity_first_descendant_text(node, source, &["identifier"]) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller,
        target: name,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn solidity_emit_using(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = solidity_first_descendant_text(node, source, &["identifier"]) else {
        return;
    };
    let source_name = enclosing_class
        .map(|class| qualify(file_path, class, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: "DEPENDS_ON".to_string(),
        source: source_name,
        target,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn solidity_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "constructor_definition" => Some("constructor".to_string()),
        "fallback_receive_definition" => {
            solidity_direct_child_kind(node, &["receive", "fallback"]).map(str::to_string)
        }
        _ => solidity_direct_child_text(node, source, &["identifier"]),
    }
}

fn solidity_params(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let params = solidity_direct_child_texts(node, source, &["parameter"]);
    (!params.is_empty()).then(|| format!("({})", params.join(", ")))
}

fn solidity_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = solidity_call_callee(node)?;
    let callee = if callee.kind() == "expression" {
        solidity_first_non_punctuation_child(callee).unwrap_or(callee)
    } else {
        callee
    };
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        "member_expression" => solidity_last_descendant_text(callee, source, &["identifier"]),
        _ => None,
    }
}

fn solidity_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "call_arguments" | "arguments"));
    found
}

fn solidity_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "inheritance_specifier" {
            if let Some(target) = solidity_first_descendant_text(child, source, &["identifier"]) {
                out.push(target);
            }
        }
    }
    out
}

fn solidity_first_non_punctuation_child(
    node: tree_sitter::Node<'_>,
) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]"));
    found
}

fn solidity_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn solidity_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    solidity_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn solidity_direct_child_kind<'a>(node: tree_sitter::Node<'a>, kinds: &[&str]) -> Option<&'a str> {
    solidity_direct_child(node, kinds).map(|child| child.kind())
}

fn solidity_direct_child_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
    }
    out
}

fn solidity_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = solidity_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn solidity_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            found = Some(node_text(child, source));
        }
        if let Some(value) = solidity_last_descendant_text(child, source, kinds) {
            found = Some(value);
        }
    }
    found
}

pub fn parse_dart(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_dart_parser();
    parse_dart_with_parser(file_path, source, parser.as_mut())
}

fn parse_dart_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "dart".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            dart_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn dart_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    dart_emit_calls_from_children(node, source, file_path, edges);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_or_export" => {
                dart_emit_import(child, source, file_path, edges);
            }
            "class_definition" | "mixin_declaration" | "enum_declaration" => {
                if let Some(name) = dart_direct_child_text(child, source, &["identifier"]) {
                    dart_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    dart_walk_children(child, source, file_path, Some(&name), nodes, edges);
                    continue;
                }
            }
            "function_signature" => {
                if let Some(name) = dart_direct_child_text(child, source, &["identifier"]) {
                    dart_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            _ => {}
        }
        dart_walk_children(child, source, file_path, enclosing_class, nodes, edges);
    }
}

fn dart_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = dart_first_descendant_text(node, source, &["string_literal"]) else {
        return;
    };
    let target = strip_matching_quotes(target.trim()).to_string();
    if target.is_empty() {
        return;
    }
    edges.push(ParsedEdge {
        kind: "IMPORTS_FROM".to_string(),
        source: file_path.to_string(),
        target,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn dart_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract) = match node.kind() {
        "mixin_declaration" => ("mixin", false),
        "enum_declaration" => ("enum", false),
        _ if dart_has_direct_child_kind(node, "abstract") => ("abstract_class", true),
        _ => ("class", false),
    };
    let mut extra = json!({"type_role": type_role});
    if is_abstract {
        if let Some(map) = extra.as_object_mut() {
            map.insert("is_abstract".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "dart".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for target in dart_inheritance_targets(node, source) {
        edges.push(ParsedEdge {
            kind: "INHERITS".to_string(),
            source: qualified.clone(),
            target,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": "extends",
                "syntax_source": "class_definition",
            }),
        });
    }
}

fn dart_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "dart".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: dart_direct_child_text(node, source, &["formal_parameter_list"]),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn dart_emit_calls_from_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut call_name = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                call_name = Some(node_text(child, source));
            }
            "selector" => {
                if let Some(method_name) = dart_selector_method_name(child, source) {
                    call_name = Some(method_name);
                }
                if dart_selector_has_arguments(child) {
                    if let Some(target) = call_name.take() {
                        edges.push(ParsedEdge {
                            kind: "CALLS".to_string(),
                            source: file_path.to_string(),
                            target,
                            file_path: file_path.to_string(),
                            line: node.start_position().row as i64 + 1,
                            extra: json!({}),
                        });
                    }
                }
            }
            "return" | "await" | "yield" | "this" | "const" | "new" => {}
            _ => {
                call_name = None;
            }
        }
    }
}

fn dart_selector_method_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "unconditional_assignable_selector" {
            return dart_first_descendant_text(child, source, &["identifier"]);
        }
    }
    None
}

fn dart_selector_has_arguments(node: tree_sitter::Node<'_>) -> bool {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|child| child.kind() == "argument_part");
    found
}

fn dart_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "superclass" | "interfaces") {
            dart_collect_type_identifiers(child, source, &mut out);
        }
    }
    out
}

fn dart_collect_type_identifiers(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    out: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_identifier" {
            out.push(node_text(child, source));
        } else {
            dart_collect_type_identifiers(child, source, out);
        }
    }
}

fn dart_has_direct_child_kind(node: tree_sitter::Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|child| child.kind() == kind);
    found
}

fn dart_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn dart_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    dart_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn dart_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = dart_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

pub fn parse_lua(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_lua_parser();
    parse_lua_like_with_parser(file_path, source, "lua", parser.as_mut())
}

pub fn parse_luau(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_luau_parser();
    parse_luau_with_parser(file_path, source, parser.as_mut())
}

fn parse_lua_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_lua_like_with_parser(file_path, source, "lua", parser)
}

fn parse_luau_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_lua_like_with_parser(file_path, source, "luau", parser)
}

fn parse_lua_like_with_parser(
    file_path: &str,
    source: &[u8],
    language: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = LuaParseContext {
        source,
        file_path,
        language,
    };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            lua_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_lua_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct LuaParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    language: &'a str,
}

fn lua_walk_children(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "variable_declaration" => {
                if lua_handle_variable_declaration(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) {
                    continue;
                }
            }
            "function_declaration" => {
                if let Some((parent, name)) = lua_table_function_name(child, context.source) {
                    lua_emit_function(child, context, &name, Some(&parent), nodes, edges);
                    lua_walk_children(child, context, Some(&parent), Some(&name), nodes, edges);
                    continue;
                }
                if let Some(name) = lua_direct_child_text(child, context.source, &["identifier"]) {
                    lua_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    lua_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "function_call" => {
                if enclosing_func.is_none() {
                    if let Some(target) = lua_require_target(child, context.source) {
                        edges.push(ParsedEdge {
                            kind: "IMPORTS_FROM".to_string(),
                            source: context.file_path.to_string(),
                            target,
                            file_path: context.file_path.to_string(),
                            line: child.start_position().row as i64 + 1,
                            extra: json!({}),
                        });
                        continue;
                    }
                }
                lua_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            "type_definition" if context.language == "luau" => {
                if let Some(name) = lua_direct_child_text(child, context.source, &["identifier"]) {
                    lua_emit_type(child, context, &name, nodes, edges);
                    continue;
                }
            }
            _ => {}
        }
        lua_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn lua_handle_variable_declaration(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(assign) = lua_direct_child(node, &["assignment_statement"]) else {
        return false;
    };
    let Some(var_name) = lua_assignment_variable_name(assign, context.source) else {
        return false;
    };
    let Some(expr_list) = lua_direct_child(assign, &["expression_list"]) else {
        return false;
    };

    let mut cursor = expr_list.walk();
    for expr in expr_list.children(&mut cursor) {
        if expr.kind() == "function_call" {
            if let Some(target) = lua_require_target(expr, context.source) {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: context.file_path.to_string(),
                    target,
                    file_path: context.file_path.to_string(),
                    line: node.start_position().row as i64 + 1,
                    extra: json!({}),
                });
                return true;
            }
        }
    }

    let mut cursor = expr_list.walk();
    for expr in expr_list.children(&mut cursor) {
        if expr.kind() == "function_definition" {
            lua_emit_function(node, context, &var_name, enclosing_class, nodes, edges);
            lua_walk_children(
                expr,
                context,
                enclosing_class,
                Some(&var_name),
                nodes,
                edges,
            );
            return true;
        }
    }

    let _ = enclosing_func;
    false
}

fn lua_emit_function(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: lua_first_descendant_text(node, context.source, &["parameters"]),
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn lua_emit_type(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn lua_emit_call(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(call_name) = lua_call_name(node, context.source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller.clone(),
        target: call_name,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(signature) = lua_call_signature(node, context.source) {
        if let Some(edge) = lua_bridge_edge(node, context, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn lua_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = lua_call_callee(node)?;
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        "dot_index_expression" | "method_index_expression" => {
            lua_last_direct_child_text(callee, source, "identifier")
        }
        _ => None,
    }
}

fn lua_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = lua_call_callee(node)?;
    let signature = match callee.kind() {
        "identifier" => node_text(callee, source),
        "dot_index_expression" | "method_index_expression" => node_text(callee, source)
            .replace(':', ".")
            .trim()
            .to_string(),
        _ => return None,
    };
    (!signature.is_empty()).then_some(signature)
}

fn lua_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "arguments");
    found
}

fn lua_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "os.execute" | "io.popen" => ("invokes_binary", "subprocess"),
        "io.open" => ("opens_file", "file_io"),
        "io.lines" | "io.read" => ("reads_file", "file_io"),
        "io.write" => ("writes_file", "file_io"),
        "package.loadlib" | "loadlib" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match lua_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": context.language,
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn lua_require_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = lua_call_callee(node)?;
    if first.kind() != "identifier" || !node_text_is(first, source, "require") {
        return None;
    }
    lua_first_string_arg(node, source)
}

fn lua_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = lua_direct_child(node, &["arguments"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string" {
            return Some(lua_string_text(child, source));
        }
        return None;
    }
    None
}

fn lua_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    if let Some(content) = lua_first_descendant_text(node, source, &["string_content"]) {
        return content;
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn lua_table_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<(String, String)> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "dot_index_expression" | "method_index_expression"
        ) {
            let names = lua_direct_child_texts(child, source, &["identifier"]);
            if names.len() >= 2 {
                return Some((names[0].clone(), names[names.len() - 1].clone()));
            }
        }
    }
    None
}

fn lua_assignment_variable_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let variable_list = lua_direct_child(node, &["variable_list"])?;
    lua_first_descendant_text(variable_list, source, &["identifier"])
}

fn lua_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn lua_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    lua_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn lua_direct_child_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
    }
    out
}

fn lua_last_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kind: &str,
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            found = Some(node_text(child, source));
        }
    }
    found
}

fn lua_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = lua_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_lua_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .map(|node| node.name.as_str())
        .collect::<HashSet<_>>();
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS"
                && !edge.target.contains("::")
                && symbols.contains(edge.target.as_str())
            {
                edge.target = qualify(file_path, &edge.target, None);
            }
            edge
        })
        .collect()
}

pub fn parse_elixir(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_elixir_parser();
    parse_elixir_with_parser(file_path, source, parser.as_mut())
}

fn parse_elixir_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "elixir".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = ElixirParseContext { source, file_path };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            elixir_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_elixir_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct ElixirParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
}

fn elixir_walk_children(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call"
            && elixir_handle_call(
                child,
                context,
                enclosing_module,
                enclosing_func,
                nodes,
                edges,
            )
        {
            continue;
        }
        elixir_walk_children(
            child,
            context,
            enclosing_module,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn elixir_handle_call(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(ident) = elixir_call_identifier(node, context.source) else {
        return false;
    };
    match ident.as_str() {
        "defmodule" => {
            let Some(arguments) = elixir_direct_child(node, &["arguments"]) else {
                return false;
            };
            let Some(module_name) = elixir_module_name(arguments, context.source) else {
                return false;
            };
            elixir_emit_module(node, context, &module_name, nodes, edges);
            if let Some(do_block) = elixir_direct_child(node, &["do_block"]) {
                elixir_walk_children(do_block, context, Some(&module_name), None, nodes, edges);
            }
            true
        }
        "def" | "defp" | "defmacro" | "defmacrop" => {
            let Some(arguments) = elixir_direct_child(node, &["arguments"]) else {
                return false;
            };
            let Some((function_name, params)) =
                elixir_function_name_and_params(arguments, context.source)
            else {
                return false;
            };
            elixir_emit_function(
                node,
                context,
                &function_name,
                params.as_deref(),
                enclosing_module,
                nodes,
                edges,
            );
            if let Some(do_block) = elixir_direct_child(node, &["do_block"]) {
                elixir_walk_children(
                    do_block,
                    context,
                    enclosing_module,
                    Some(&function_name),
                    nodes,
                    edges,
                );
            }
            true
        }
        "alias" | "import" | "require" | "use" => {
            if let Some(arguments) = elixir_direct_child(node, &["arguments"]) {
                if let Some(module_name) = elixir_module_name(arguments, context.source) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target: module_name,
                        file_path: context.file_path.to_string(),
                        line: node.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
            }
            true
        }
        _ => {
            elixir_emit_call(node, context, enclosing_module, enclosing_func, edges);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if matches!(child.kind(), "arguments" | "do_block") {
                    elixir_walk_children(
                        child,
                        context,
                        enclosing_module,
                        enclosing_func,
                        nodes,
                        edges,
                    );
                }
            }
            true
        }
    }
}

fn elixir_emit_module(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "elixir".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn elixir_emit_function(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    name: &str,
    params: Option<&str>,
    enclosing_module: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, enclosing_module);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "elixir".to_string(),
        parent_name: enclosing_module.map(str::to_string),
        params: params.map(str::to_string),
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_module
            .map(|module| qualify(context.file_path, module, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn elixir_emit_call(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = elixir_call_target(node, context.source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_module))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller,
        target,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn elixir_call_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = elixir_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "dot" => elixir_last_direct_child_text(first, source, "identifier"),
        _ => None,
    }
}

fn elixir_call_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = elixir_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "dot" => Some(node_text(first, source).replace(' ', "")),
        _ => None,
    }
}

fn elixir_module_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "alias" | "dot") {
            return Some(node_text(child, source).replace(' ', ""));
        }
    }
    None
}

fn elixir_function_name_and_params(
    arguments: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, Option<String>)> {
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if child.kind() == "call" {
            let name = elixir_direct_child_text(child, source, &["identifier"])?;
            let mut params_text = node_text(child, source);
            if params_text.starts_with(&name) {
                params_text = params_text[name.len()..].to_string();
            }
            return Some((name, (!params_text.is_empty()).then_some(params_text)));
        }
        if child.kind() == "identifier" {
            return Some((node_text(child, source), None));
        }
    }
    None
}

fn elixir_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn elixir_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    elixir_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn elixir_first_named_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|child| child.is_named());
    found
}

fn elixir_last_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kind: &str,
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            found = Some(node_text(child, source));
        }
    }
    found
}

fn resolve_elixir_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let mut module_functions = HashMap::<(String, String), String>::new();
    let mut dotted_functions = HashMap::<String, String>::new();
    let mut bare_functions = HashMap::<String, String>::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
    {
        let qualified = qualify(file_path, &node.name, node.parent_name.as_deref());
        bare_functions
            .entry(node.name.clone())
            .or_insert_with(|| qualified.clone());
        if let Some(module) = &node.parent_name {
            module_functions.insert((module.clone(), node.name.clone()), qualified.clone());
            dotted_functions.insert(format!("{module}.{}", node.name), qualified);
        }
    }

    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS" && !edge.target.contains("::") {
                if let Some(target) = dotted_functions.get(&edge.target) {
                    edge.target = target.clone();
                } else if edge.target.contains('.') {
                    edge.target = edge
                        .target
                        .rsplit('.')
                        .next()
                        .unwrap_or(edge.target.as_str())
                        .to_string();
                } else if let Some(module) = elixir_source_module(&edge.source, file_path) {
                    if let Some(target) =
                        module_functions.get(&(module.to_string(), edge.target.clone()))
                    {
                        edge.target = target.clone();
                    } else if let Some(target) = bare_functions.get(&edge.target) {
                        edge.target = target.clone();
                    }
                } else if let Some(target) = bare_functions.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}

fn elixir_source_module<'a>(source: &'a str, file_path: &str) -> Option<&'a str> {
    let suffix = source.strip_prefix(file_path)?.strip_prefix("::")?;
    suffix.rsplit_once('.').map(|(module, _)| module)
}

pub fn parse_gdscript(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_gdscript_parser();
    parse_gdscript_with_parser(file_path, source, parser.as_mut())
}

fn parse_gdscript_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "gdscript".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = GdscriptParseContext { source, file_path };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            gdscript_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_gdscript_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct GdscriptParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
}

fn gdscript_walk_children(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "extends_statement" if enclosing_func.is_none() => {
                if let Some(target) = gdscript_extends_target(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
                continue;
            }
            "class_name_statement" => {
                if let Some(name) = gdscript_direct_child_text(child, context.source, &["name"]) {
                    gdscript_emit_class(child, context, &name, None, nodes, edges);
                }
                continue;
            }
            "class_definition" => {
                if let Some(name) = gdscript_direct_child_text(child, context.source, &["name"]) {
                    gdscript_emit_class(child, context, &name, enclosing_class, nodes, edges);
                    if let Some(body) = gdscript_direct_child(child, &["class_body"]) {
                        gdscript_walk_children(body, context, Some(&name), None, nodes, edges);
                    }
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = gdscript_direct_child_text(child, context.source, &["name"]) {
                    gdscript_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    if let Some(body) = gdscript_direct_child(child, &["body"]) {
                        gdscript_walk_children(
                            body,
                            context,
                            enclosing_class,
                            Some(&name),
                            nodes,
                            edges,
                        );
                    }
                    continue;
                }
            }
            "call" | "attribute_call" => {
                gdscript_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        gdscript_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn gdscript_emit_class(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "gdscript".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn gdscript_emit_function(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "gdscript".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: gdscript_direct_child_text(node, context.source, &["parameters"]),
        return_type: gdscript_direct_child_text(node, context.source, &["type"]),
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn gdscript_emit_call(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = gdscript_call_name(node, context.source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller,
        target,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn gdscript_extends_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let type_node = gdscript_direct_child(node, &["type"])?;
    gdscript_first_descendant_text(type_node, source, &["identifier"])
        .or_else(|| Some(node_text(type_node, source).trim().to_string()))
        .filter(|target| !target.is_empty())
}

fn gdscript_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    gdscript_direct_child_text(node, source, &["identifier"])
}

fn gdscript_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn gdscript_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    gdscript_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn gdscript_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = gdscript_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_gdscript_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, node.parent_name.as_deref()));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS" && !edge.target.contains("::") {
                if let Some(target) = symbols.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}

pub fn parse_r(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_r_parser();
    parse_r_with_parser(file_path, source, parser.as_mut())
}

fn parse_r_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "r".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = RParseContext { source, file_path };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            r_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_r_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct RParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
}

fn r_walk_children(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "binary_operator" => {
                if r_handle_binary_operator(child, context, enclosing_class, nodes, edges) {
                    continue;
                }
            }
            "call" => {
                if r_handle_call(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) {
                    continue;
                }
            }
            _ => {}
        }
        r_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn r_handle_binary_operator(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some((left, operator, right)) = r_binary_operator_parts(node) else {
        return false;
    };
    if !matches!(operator.kind(), "<-" | "=") || left.kind() != "identifier" {
        return false;
    }
    let name = node_text(left, context.source);
    if right.kind() == "function_definition" {
        r_emit_function(right, context, &name, enclosing_class, nodes, edges);
        r_walk_children(right, context, enclosing_class, Some(&name), nodes, edges);
        return true;
    }
    if right.kind() == "call" {
        if let Some(call_name) = r_call_name(right, context.source) {
            if matches!(
                call_name.as_str(),
                "setRefClass" | "setClass" | "setGeneric"
            ) {
                r_emit_class_call(right, context, Some(&name), enclosing_class, nodes, edges);
                return true;
            }
        }
    }
    false
}

fn r_handle_call(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(call_name) = r_call_name(node, context.source) else {
        return false;
    };

    if matches!(call_name.as_str(), "library" | "require" | "source") {
        if let Some(target) = r_import_target(node, context.source) {
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: context.file_path.to_string(),
                target,
                file_path: context.file_path.to_string(),
                line: node.start_position().row as i64 + 1,
                extra: json!({}),
            });
        }
        return true;
    }

    if matches!(
        call_name.as_str(),
        "setRefClass" | "setClass" | "setGeneric"
    ) {
        r_emit_class_call(node, context, None, enclosing_class, nodes, edges);
        return true;
    }

    r_emit_call(
        node,
        context,
        &call_name,
        enclosing_class,
        enclosing_func,
        edges,
    );
    r_walk_children(node, context, enclosing_class, enclosing_func, nodes, edges);
    true
}

fn r_emit_function(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "r".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: r_direct_child_text(node, context.source, &["parameters"]),
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn r_emit_class_call(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    assigned_name: Option<&str>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(class_name) = r_first_string_arg(node, context.source).or_else(|| {
        assigned_name
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    }) else {
        return;
    };
    let qualified = qualify(context.file_path, &class_name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: class_name.clone(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "r".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(methods) = r_find_named_arg(node, context.source, "methods") {
        r_extract_methods(methods, context, &class_name, nodes, edges);
    }
}

fn r_extract_methods(
    list_call: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    class_name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    for (method_name, value) in r_iter_args(list_call, context.source) {
        let Some(method_name) = method_name else {
            continue;
        };
        if value.kind() != "function_definition" {
            continue;
        }
        r_emit_function(value, context, &method_name, Some(class_name), nodes, edges);
        r_walk_children(
            value,
            context,
            Some(class_name),
            Some(&method_name),
            nodes,
            edges,
        );
    }
}

fn r_emit_call(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    call_name: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller.clone(),
        target: call_name.to_string(),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = r_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

fn r_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "system" | "system2" => ("invokes_binary", "subprocess"),
        ".Call" | ".External" => ("loads_native_module", "ffi"),
        "dyn.load" | "library.dynam" => ("loads_shared_library", "ffi"),
        "readLines" | "read.csv" | "read.table" => ("reads_file", "file_io"),
        "writeLines" | "write.csv" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match r_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "r",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn r_binary_operator_parts<'a>(
    node: tree_sitter::Node<'a>,
) -> Option<(
    tree_sitter::Node<'a>,
    tree_sitter::Node<'a>,
    tree_sitter::Node<'a>,
)> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    if children.len() < 3 {
        return None;
    }
    Some((children[0], children[1], children[2]))
}

fn r_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "identifier" | "namespace_operator") {
            return Some(node_text(child, source));
        }
    }
    None
}

fn r_import_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let (_, value) = r_iter_args(node, source).into_iter().next()?;
    match value.kind() {
        "identifier" => Some(node_text(value, source)),
        "string" => r_string_text(value, source),
        _ => None,
    }
}

fn r_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let (_, value) = r_iter_args(node, source).into_iter().next()?;
    if value.kind() == "string" {
        r_string_text(value, source)
    } else {
        None
    }
}

fn r_find_named_arg<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    arg_name: &str,
) -> Option<tree_sitter::Node<'a>> {
    r_iter_args(node, source)
        .into_iter()
        .find_map(|(name, value)| (name.as_deref() == Some(arg_name)).then_some(value))
}

fn r_iter_args<'a>(
    call_node: tree_sitter::Node<'a>,
    source: &[u8],
) -> Vec<(Option<String>, tree_sitter::Node<'a>)> {
    let Some(arguments) = r_direct_child(call_node, &["arguments"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = arguments.walk();
    for argument in arguments.children(&mut cursor) {
        if argument.kind() != "argument" {
            continue;
        }
        let mut name = None;
        let mut value = None;
        let mut seen_equals = false;
        let mut arg_cursor = argument.walk();
        for child in argument.children(&mut arg_cursor) {
            if child.kind() == "=" {
                seen_equals = true;
                continue;
            }
            if !child.is_named() {
                continue;
            }
            if seen_equals {
                value = Some(child);
                break;
            }
            if name.is_none() && child.kind() == "identifier" {
                name = Some(node_text(child, source));
                continue;
            }
            if value.is_none() {
                value = Some(child);
                break;
            }
        }
        if let Some(value) = value.or_else(|| r_first_named_child(argument)) {
            out.push((seen_equals.then_some(name).flatten(), value));
        }
    }
    out
}

fn r_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    r_first_descendant_text(node, source, &["string_content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

fn r_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn r_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    r_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn r_first_named_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|child| child.is_named());
    found
}

fn r_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = r_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_r_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, node.parent_name.as_deref()));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS" && !edge.target.contains("::") {
                if let Some(target) = symbols.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}

pub fn parse_julia(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_julia_parser();
    parse_julia_with_parser(file_path, source, parser.as_mut())
}

fn parse_julia_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "julia".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = JuliaParseContext { source, file_path };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            julia_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_julia_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct JuliaParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
}

fn julia_walk_children(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "module_definition" => {
                if let Some(name) = julia_direct_child_text(child, context.source, &["identifier"])
                {
                    julia_emit_class(
                        child,
                        context,
                        JuliaClassSpec {
                            name: &name,
                            parent_name: None,
                            extra: json!({"type_role": "class"}),
                            contains_from_parent: true,
                        },
                        nodes,
                        edges,
                    );
                    if let Some(block) = julia_direct_child(child, &["block"]) {
                        julia_walk_children(block, context, Some(&name), None, nodes, edges);
                    }
                    continue;
                }
            }
            "using_statement" | "import_statement" => {
                for target in julia_import_targets(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
                continue;
            }
            "export_statement" | "public_statement" => {
                julia_emit_symbol_references(child, context, enclosing_class, edges);
                continue;
            }
            "macrocall_expression" => {
                if julia_handle_macrocall(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) {
                    continue;
                }
            }
            "abstract_definition" | "struct_definition" => {
                if let Some(name) = julia_type_name(child, context.source) {
                    let extra = if child.kind() == "abstract_definition" {
                        json!({"type_role": "abstract_type", "is_abstract": true})
                    } else {
                        json!({"type_role": "struct"})
                    };
                    julia_emit_class(
                        child,
                        context,
                        JuliaClassSpec {
                            name: &name,
                            parent_name: enclosing_class,
                            extra,
                            contains_from_parent: false,
                        },
                        nodes,
                        edges,
                    );
                    if child.kind() == "struct_definition" {
                        julia_emit_inheritance(child, context, &name, enclosing_class, edges);
                    }
                    continue;
                }
            }
            "function_definition" | "macro_definition" => {
                if let Some(name) = julia_function_name(child, context.source) {
                    let parent = julia_function_parent(enclosing_class, enclosing_func);
                    julia_emit_function(child, context, &name, parent.as_deref(), nodes, edges);
                    julia_emit_owner_reference(child, context, &name, parent.as_deref(), edges);
                    if let Some(block) = julia_direct_child(child, &["block"]) {
                        julia_walk_children(
                            block,
                            context,
                            enclosing_class,
                            Some(&name),
                            nodes,
                            edges,
                        );
                    }
                    continue;
                }
            }
            "assignment" => {
                if julia_handle_short_function(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) {
                    continue;
                }
            }
            "call_expression" => {
                if julia_is_signature_call(child) || julia_is_assignment_lhs_call(child) {
                    continue;
                }
                if let Some(call_name) = julia_call_name(child, context.source) {
                    if call_name == "include" {
                        if let Some(target) = julia_first_string_arg(child, context.source) {
                            edges.push(ParsedEdge {
                                kind: "IMPORTS_FROM".to_string(),
                                source: context.file_path.to_string(),
                                target,
                                file_path: context.file_path.to_string(),
                                line: child.start_position().row as i64 + 1,
                                extra: json!({}),
                            });
                        }
                    }
                    julia_emit_call(
                        child,
                        context,
                        &call_name,
                        enclosing_class,
                        enclosing_func,
                        edges,
                    );
                }
            }
            _ => {}
        }
        julia_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn julia_handle_short_function(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(lhs) = julia_assignment_lhs_call(node) else {
        return false;
    };
    let Some(name) = julia_call_name(lhs, context.source) else {
        return false;
    };
    let parent = julia_function_parent(enclosing_class, enclosing_func);
    julia_emit_function(node, context, &name, parent.as_deref(), nodes, edges);
    julia_emit_owner_reference(node, context, &name, parent.as_deref(), edges);
    let mut seen_operator = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !seen_operator {
            if child.kind() == "operator" {
                seen_operator = true;
            }
            continue;
        }
        julia_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
    }
    true
}

fn julia_handle_macrocall(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(macro_name) = julia_macro_name(node, context.source) else {
        return false;
    };
    match macro_name.as_str() {
        "enum" => {
            julia_emit_enum(node, context, enclosing_class, nodes, edges);
            true
        }
        "testset" => {
            julia_emit_testset(node, context, enclosing_class, enclosing_func, nodes, edges);
            true
        }
        _ => {
            julia_emit_call(
                node,
                context,
                &format!("@{macro_name}"),
                enclosing_class,
                enclosing_func,
                edges,
            );
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "macro_argument_list" {
                    julia_walk_children(
                        child,
                        context,
                        enclosing_class,
                        enclosing_func,
                        nodes,
                        edges,
                    );
                }
            }
            true
        }
    }
}

struct JuliaClassSpec<'a> {
    name: &'a str,
    parent_name: Option<&'a str>,
    extra: Value,
    contains_from_parent: bool,
}

fn julia_emit_class(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    spec: JuliaClassSpec<'_>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(context.file_path, spec.name, spec.parent_name);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: spec.name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: spec.parent_name.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: spec.extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: if spec.contains_from_parent {
            spec.parent_name
                .map(|parent| qualify(context.file_path, parent, None))
                .unwrap_or_else(|| context.file_path.to_string())
        } else {
            context.file_path.to_string()
        },
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn julia_emit_function(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    name: &str,
    parent_name: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, parent_name);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: parent_name.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: parent_name
            .map(|parent| qualify(context.file_path, parent, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn julia_emit_call(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    call_name: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller.clone(),
        target: call_name.to_string(),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = julia_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

fn julia_emit_enum(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(args) = julia_direct_child(node, &["macro_argument_list"]) else {
        return;
    };
    let identifiers = julia_direct_child_texts(args, context.source, &["identifier"]);
    let Some(type_name) = identifiers.first() else {
        return;
    };
    let qualified_type = qualify(context.file_path, type_name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: type_name.clone(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"julia_kind": "enum"}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified_type.clone(),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for variant in identifiers.iter().skip(1) {
        nodes.push(ParsedNode {
            kind: "Function".to_string(),
            name: variant.clone(),
            file_path: context.file_path.to_string(),
            line_start: node.start_position().row as i64 + 1,
            line_end: node.end_position().row as i64 + 1,
            language: "julia".to_string(),
            parent_name: Some(type_name.clone()),
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"julia_kind": "enum_variant"}),
        });
        edges.push(ParsedEdge {
            kind: "CONTAINS".to_string(),
            source: qualified_type.clone(),
            target: qualify(context.file_path, variant, Some(type_name)),
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn julia_emit_testset(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let desc = julia_direct_child(node, &["macro_argument_list"])
        .and_then(|args| julia_first_descendant_text(args, context.source, &["content"]));
    let line = node.start_position().row as i64 + 1;
    let name = desc
        .map(|desc| format!("testset:{desc}@L{line}"))
        .unwrap_or_else(|| format!("testset@L{line}"));
    let qualified = qualify(context.file_path, &name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Test".to_string(),
        name: name.clone(),
        file_path: context.file_path.to_string(),
        line_start: line,
        line_end: node.end_position().row as i64 + 1,
        language: "julia".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: true,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_func
            .map(|func| qualify(context.file_path, func, enclosing_class))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({}),
    });
    if let Some(args) = julia_direct_child(node, &["macro_argument_list"]) {
        julia_walk_children(args, context, enclosing_class, Some(&name), nodes, edges);
    }
}

fn julia_emit_symbol_references(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    enclosing_class: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let marker = if node.kind() == "export_statement" {
        "julia_export"
    } else {
        "julia_public"
    };
    let source = enclosing_class
        .map(|class| qualify(context.file_path, class, None))
        .unwrap_or_else(|| context.file_path.to_string());
    for target in julia_direct_child_texts(node, context.source, &["identifier"]) {
        edges.push(ParsedEdge {
            kind: "REFERENCES".to_string(),
            source: source.clone(),
            target,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({marker: true}),
        });
    }
}

fn julia_emit_inheritance(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(type_head) = julia_direct_child(node, &["type_head"]) else {
        return;
    };
    let Some(binary) = julia_direct_child(type_head, &["binary_expression"]) else {
        return;
    };
    let identifiers = julia_direct_child_texts(binary, context.source, &["identifier"]);
    if identifiers.len() < 2 {
        return;
    }
    edges.push(ParsedEdge {
        kind: "INHERITS".to_string(),
        source: qualify(context.file_path, name, enclosing_class),
        target: identifiers[1].clone(),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({"relationship_role": "extends", "syntax_source": "struct_definition"}),
    });
}

fn julia_emit_owner_reference(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    name: &str,
    parent_name: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(owner) = julia_qualified_function_owner(node, context.source) else {
        return;
    };
    edges.push(ParsedEdge {
        kind: "REFERENCES".to_string(),
        source: qualify(context.file_path, name, parent_name),
        target: owner,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn julia_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &JuliaParseContext<'_>,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let signature = julia_call_signature(node, context.source).unwrap_or_else(|| call_name.into());
    let (relationship_role, bridge_kind) = match signature.as_str() {
        "run" | "readchomp" => ("invokes_binary", "subprocess"),
        "open" => ("opens_file", "file_io"),
        "read" | "readlines" => ("reads_file", "file_io"),
        "write" => ("writes_file", "file_io"),
        "Libdl.dlopen" | "dlopen" | "ccall" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match julia_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "julia",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn julia_import_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            targets.push(node_text(child, source));
        } else if child.kind() == "selected_import" {
            let names = julia_direct_child_texts(child, source, &["identifier"]);
            if let Some(module) = names.first() {
                targets.extend(names.iter().skip(1).map(|name| format!("{module}.{name}")));
            }
        }
    }
    targets
}

fn julia_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let type_head = julia_direct_child(node, &["type_head"])?;
    julia_first_descendant_text(type_head, source, &["identifier"])
}

fn julia_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let signature = julia_direct_child(node, &["signature"])?;
    let call = julia_first_descendant(signature, &["call_expression"])?;
    julia_call_name(call, source)
}

fn julia_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = julia_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "field_expression" => julia_last_descendant_text(first, source, &["identifier"]),
        _ => None,
    }
}

fn julia_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = julia_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "field_expression" => Some(node_text(first, source).replace(' ', "")),
        _ => None,
    }
}

fn julia_macro_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let macro_identifier = julia_direct_child(node, &["macro_identifier"])?;
    julia_direct_child_text(macro_identifier, source, &["identifier"])
}

fn julia_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let args = julia_direct_child(node, &["argument_list"])?;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return julia_string_text(child, source);
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn julia_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    julia_first_descendant_text(node, source, &["content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

fn julia_qualified_function_owner(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let signature = if node.kind() == "assignment" {
        julia_assignment_lhs_call(node)
    } else {
        julia_direct_child(node, &["signature"])
            .and_then(|signature| julia_first_descendant(signature, &["call_expression"]))
    }?;
    let field = julia_first_descendant(signature, &["field_expression"])?;
    let names = julia_direct_child_texts(field, source, &["identifier"]);
    names.first().cloned()
}

fn julia_assignment_lhs_call<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let lhs = julia_first_named_child(node)?;
    if lhs.kind() == "call_expression" {
        Some(lhs)
    } else if lhs.kind() == "typed_expression" {
        julia_first_descendant(lhs, &["call_expression"])
    } else {
        None
    }
}

fn julia_is_signature_call(node: tree_sitter::Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == "signature")
}

fn julia_is_assignment_lhs_call(node: tree_sitter::Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() == "assignment" {
        return julia_first_named_child(parent) == Some(node);
    }
    if parent.kind() == "typed_expression" {
        return parent
            .parent()
            .is_some_and(|grandparent| grandparent.kind() == "assignment");
    }
    false
}

fn julia_function_parent(
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
) -> Option<String> {
    match (enclosing_class, enclosing_func) {
        (Some(class), Some(func)) => Some(format!("{class}.{func}")),
        (Some(class), None) => Some(class.to_string()),
        (None, Some(func)) => Some(func.to_string()),
        (None, None) => None,
    }
}

fn julia_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn julia_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    julia_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn julia_direct_child_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
    }
    out
}

fn julia_first_named_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|child| child.is_named());
    found
}

fn julia_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = julia_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn julia_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    julia_first_descendant(node, kinds).map(|child| node_text(child, source))
}

fn julia_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    julia_collect_descendant_texts(node, source, kinds, &mut found);
    found
}

fn julia_collect_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    found: &mut Option<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            *found = Some(node_text(child, source));
        }
        julia_collect_descendant_texts(child, source, kinds, found);
    }
}

fn resolve_julia_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Class" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, node.parent_name.as_deref()));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            if matches!(edge.kind.as_str(), "CALLS" | "REFERENCES") && !edge.target.contains("::") {
                if let Some(target) = symbols.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}

pub fn parse_perl(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_perl_parser();
    parse_perl_with_parser(file_path, source, parser.as_mut())
}

fn parse_perl_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "perl".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = PerlParseContext { source, file_path };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            perl_walk_children(tree.root_node(), &context, None, &mut nodes, &mut edges);
            let mut edges = resolve_perl_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct PerlParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
}

fn perl_walk_children(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "use_statement" | "require_expression" if enclosing_func.is_none() => {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: context.file_path.to_string(),
                    target: node_text(child, context.source),
                    file_path: context.file_path.to_string(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
                continue;
            }
            "package_statement" | "class_statement" | "role_statement" => {
                if let Some(name) = perl_package_name(child, context.source) {
                    perl_emit_class(child, context, &name, nodes, edges);
                }
                continue;
            }
            "subroutine_declaration_statement" | "method_declaration_statement" => {
                if let Some(name) = perl_subroutine_name(child, context.source) {
                    perl_emit_function(child, context, &name, nodes, edges);
                    perl_walk_children(child, context, Some(&name), nodes, edges);
                }
                continue;
            }
            "function_call_expression"
            | "ambiguous_function_call_expression"
            | "method_call_expression"
            | "anonymous_function_call_expression" => {
                if let Some(call_name) = perl_call_name(child, context.source) {
                    perl_emit_call(child, context, &call_name, enclosing_func, edges);
                }
            }
            _ => {}
        }
        perl_walk_children(child, context, enclosing_func, nodes, edges);
    }
}

fn perl_emit_class(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "perl".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn perl_emit_function(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "perl".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn perl_emit_call(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    call_name: &str,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller.clone(),
        target: call_name.to_string(),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = perl_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

fn perl_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match call_name {
        "system" | "exec" => ("invokes_binary", "subprocess"),
        "open" => ("opens_file", "file_io"),
        "File::Slurp::read_file" => ("reads_file", "file_io"),
        "File::Slurp::write_file" => ("writes_file", "file_io"),
        "DynaLoader::dl_load_file" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match perl_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{call_name}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": call_name,
            "source_language": "perl",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn perl_package_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let package_name = node
        .children(&mut cursor)
        .find(|child| child.is_named() && child.kind() == "package")
        .map(|child| node_text(child, source));
    package_name
}

fn perl_subroutine_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    perl_direct_child_text(node, source, &["bareword", "identifier"])
}

fn perl_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "method_call_expression" {
        return perl_direct_child_text(node, source, &["method", "bareword", "identifier"]);
    }
    perl_direct_child_text(node, source, &["function", "bareword", "identifier"])
}

fn perl_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let mut skipped_callee = false;
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "function" | "method") && !skipped_callee {
            skipped_callee = true;
            continue;
        }
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if matches!(
            child.kind(),
            "interpolated_string_literal" | "string_literal" | "quoted_word_list"
        ) {
            return perl_string_text(child, source);
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn perl_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    perl_first_descendant_text(node, source, &["string_content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

fn perl_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn perl_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    perl_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn perl_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = perl_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_perl_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, None));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS" && !edge.target.contains("::") {
                if let Some(target) = symbols.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}

pub fn parse_c(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_c_parser();
    parse_c_like_with_parser(file_path, source, "c", parser.as_mut())
}

pub fn parse_cpp(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_cpp_parser();
    parse_cpp_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_objc(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_objc_parser();
    parse_objc_with_parser(file_path, source, parser.as_mut())
}

fn parse_c_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "c", parser)
}

fn parse_cpp_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "cpp", parser)
}

fn parse_objc_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "objc", parser)
}

fn parse_c_like_with_parser(
    file_path: &str,
    source: &[u8],
    language: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = CParseContext {
        source,
        file_path,
        language,
    };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            c_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_c_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct CParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    language: &'a str,
}

fn c_walk_children(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "preproc_include" if enclosing_func.is_none() => {
                if let Some(target) = c_include_target(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    continue;
                }
            }
            "type_definition" | "struct_specifier" | "class_specifier"
                if enclosing_func.is_none() =>
            {
                if let Some(name) = c_type_name(child, context.source) {
                    c_emit_type(child, context, &name, nodes, edges);
                    c_emit_inheritance(child, context, &name, edges);
                    if context.language == "cpp" {
                        c_walk_children(child, context, Some(&name), enclosing_func, nodes, edges);
                    }
                    continue;
                }
            }
            "class_interface"
            | "class_implementation"
            | "category_interface"
            | "protocol_declaration"
                if context.language == "objc" && enclosing_func.is_none() =>
            {
                if let Some(name) = c_direct_child_text(child, context.source, &["identifier"]) {
                    c_emit_type(child, context, &name, nodes, edges);
                    if child.kind() == "class_implementation" {
                        c_walk_children(child, context, Some(&name), None, nodes, edges);
                    }
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = c_function_name(child, context.source) {
                    c_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    c_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "method_definition" if context.language == "objc" => {
                if let Some(name) = c_direct_child_text(child, context.source, &["identifier"]) {
                    c_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    c_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "call_expression" => {
                c_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            "message_expression" if context.language == "objc" => {
                c_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        c_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn c_emit_type(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn c_emit_function(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn c_emit_call(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    if let Some(call_name) = c_call_name(node, context.source) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = c_call_signature(node, context.source) {
        if let Some(edge) = c_bridge_edge(node, context, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn c_emit_inheritance(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    if context.language != "cpp" {
        return;
    }
    let Some(base_clause) = c_direct_child(node, &["base_class_clause"]) else {
        return;
    };
    let Some(base) = c_last_descendant_text(base_clause, context.source, &["type_identifier"])
    else {
        return;
    };
    edges.push(ParsedEdge {
        kind: "INHERITS".to_string(),
        source: qualify(context.file_path, name, None),
        target: base,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({
            "relationship_role": "extends",
            "syntax_source": "class_specifier",
        }),
    });
}

fn c_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "message_expression" {
        return c_message_selector(node, source);
    }
    let callee = c_call_callee(node)?;
    match callee.kind() {
        "identifier" | "qualified_identifier" => {
            Some(node_text(callee, source).replace(" :: ", "::"))
        }
        "field_expression" => c_last_descendant_text(callee, source, &["field_identifier"]),
        "message_expression" => c_message_selector(callee, source),
        _ => None,
    }
}

fn c_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list");
    found
}

fn c_include_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text_bytes(node, source);
    if text.starts_with(b"#import") {
        return Some(match std::str::from_utf8(text) {
            Ok(text) => text.trim().to_string(),
            Err(_) => String::from_utf8_lossy(text).trim().to_string(),
        });
    }
    let target = c_direct_child(node, &["system_lib_string", "string_literal"])?;
    Some(
        strip_matching_quotes(
            node_text(target, source)
                .trim()
                .trim_matches(['<', '>'].as_ref()),
        )
        .to_string(),
    )
}

fn c_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    c_direct_child_text(node, source, &["type_identifier"])
}

fn c_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let declarator = c_first_descendant(node, &["function_declarator"])?;
    c_direct_child_text(declarator, source, &["identifier"])
}

fn c_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "message_expression" {
        return c_message_selector(node, source);
    }
    let callee = c_call_callee(node)?;
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        "field_expression" => c_last_descendant_text(callee, source, &["field_identifier"]),
        "message_expression" => c_message_selector(callee, source),
        _ => None,
    }
}

fn c_message_selector(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut skipped_receiver = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "[" | "]" | ":") {
            continue;
        }
        if !skipped_receiver {
            skipped_receiver = true;
            continue;
        }
        if child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn c_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "system" | "popen" | "execvp" | "execv" | "execl" | "posix_spawn" => {
            ("invokes_binary", "subprocess")
        }
        "fopen" | "open" => ("opens_file", "file_io"),
        "fread" => ("reads_file", "file_io"),
        "fwrite" => ("writes_file", "file_io"),
        "dlopen" | "LoadLibrary" => ("loads_shared_library", "ffi"),
        "std::system" | "boost::process::child" => ("invokes_binary", "subprocess"),
        "std::ifstream" | "std::ofstream" | "std::fstream" => ("opens_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match c_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": context.language,
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn c_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = c_direct_child(node, &["argument_list"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return Some(c_string_text(child, source));
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn c_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn c_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn c_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    c_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn c_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = c_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn c_collect_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    found: &mut Option<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            *found = Some(node_text(child, source));
        }
        c_collect_descendant_texts(child, source, kinds, found);
    }
}

fn c_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    c_collect_descendant_texts(node, source, kinds, &mut found);
    found
}

fn resolve_c_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, node.parent_name.as_deref()));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS" && !edge.target.contains("::") {
                if let Some(target) = symbols.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}

fn resolve_rust_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Class" | "Type" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, node.parent_name.as_deref()));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            if matches!(edge.kind.as_str(), "CALLS" | "REFERENCES") && !edge.target.contains("::") {
                if let Some(target) = symbols.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}

fn qualify(file_path: &str, name: &str, parent_name: Option<&str>) -> String {
    parent_name
        .filter(|parent| !parent.is_empty())
        .map(|parent| format!("{file_path}::{parent}.{name}"))
        .unwrap_or_else(|| format!("{file_path}::{name}"))
}

fn is_test_function(
    name: &str,
    file_path: &str,
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> bool {
    starts_with_ascii_ignore_case(name, "test")
        || contains_ascii_ignore_case(file_path, "/test/")
        || contains_ascii_ignore_case(file_path, "/tests/")
        || has_rust_test_attribute(node, source)
}

fn has_rust_test_attribute(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "attribute_item" | "inner_attribute_item")
            && node_text(child, source).contains("test")
        {
            return true;
        }
    }
    false
}

fn parsed_compact_json(nodes: Vec<ParsedNode>, edges: Vec<ParsedEdge>) -> String {
    let (compact_nodes, compact_edges) = parsed_compact_values(nodes, edges);
    json!([compact_nodes, compact_edges]).to_string()
}

fn parsed_compact_values(
    nodes: Vec<ParsedNode>,
    edges: Vec<ParsedEdge>,
) -> (Vec<Value>, Vec<Value>) {
    let compact_nodes = nodes
        .into_iter()
        .map(|node| {
            json!([
                node.kind,
                node.name,
                node.file_path,
                node.line_start,
                node.line_end,
                node.language,
                node.parent_name,
                node.params,
                node.return_type,
                node.modifiers,
                node.is_test,
                node.extra,
            ])
        })
        .collect::<Vec<_>>();
    let compact_edges = edges
        .into_iter()
        .map(|edge| {
            json!([
                edge.kind,
                edge.source,
                edge.target,
                edge.file_path,
                edge.line,
                edge.extra,
            ])
        })
        .collect::<Vec<_>>();
    (compact_nodes, compact_edges)
}

pub fn parse_rust_owned_file(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    match rust_owned_path_kind_for_source(file_path, source) {
        RustOwnedPathKind::Markdown => parse_markdown(file_path, source),
        RustOwnedPathKind::Terraform => parse_terraform(file_path, source),
        RustOwnedPathKind::Rust => parse_rust(file_path, source),
        RustOwnedPathKind::Python => parse_python(file_path, source),
        RustOwnedPathKind::Notebook => parse_notebook(file_path, source),
        RustOwnedPathKind::JavaScript => parse_javascript_like(file_path, source, "javascript"),
        RustOwnedPathKind::TypeScript => parse_javascript_like(file_path, source, "typescript"),
        RustOwnedPathKind::Tsx => parse_javascript_like(file_path, source, "tsx"),
        RustOwnedPathKind::Bash => parse_bash(file_path, source),
        RustOwnedPathKind::Go => parse_go(file_path, source),
        RustOwnedPathKind::Java => parse_java(file_path, source),
        RustOwnedPathKind::Ruby => parse_ruby(file_path, source),
        RustOwnedPathKind::CSharp => parse_csharp(file_path, source),
        RustOwnedPathKind::Php => parse_php(file_path, source),
        RustOwnedPathKind::Kotlin => parse_kotlin(file_path, source),
        RustOwnedPathKind::Scala => parse_scala(file_path, source),
        RustOwnedPathKind::Solidity => parse_solidity(file_path, source),
        RustOwnedPathKind::Dart => parse_dart(file_path, source),
        RustOwnedPathKind::Lua => parse_lua(file_path, source),
        RustOwnedPathKind::Luau => parse_luau(file_path, source),
        RustOwnedPathKind::C => parse_c(file_path, source),
        RustOwnedPathKind::Cpp => parse_cpp(file_path, source),
        RustOwnedPathKind::ObjC => parse_objc(file_path, source),
        RustOwnedPathKind::Elixir => parse_elixir(file_path, source),
        RustOwnedPathKind::Gdscript => parse_gdscript(file_path, source),
        RustOwnedPathKind::R => parse_r(file_path, source),
        RustOwnedPathKind::Julia => parse_julia(file_path, source),
        RustOwnedPathKind::Perl => parse_perl(file_path, source),
        RustOwnedPathKind::Vue => parse_vue(file_path, source),
        RustOwnedPathKind::Svelte => parse_svelte(file_path, source),
        RustOwnedPathKind::Zig => parse_zig(file_path, source),
        RustOwnedPathKind::PowerShell => parse_powershell(file_path, source),
        RustOwnedPathKind::ReScript => parse_rescript(file_path, source),
        RustOwnedPathKind::Swift => parse_swift(file_path, source),
        RustOwnedPathKind::Unsupported => (Vec::new(), Vec::new()),
    }
}

pub fn rust_parser_owns_path(file_path: &str) -> bool {
    rust_owned_path_kind(file_path) != RustOwnedPathKind::Unsupported
}

pub fn rust_parser_owns_source(file_path: &str, source: &[u8]) -> bool {
    rust_owned_path_kind_for_source(file_path, source) != RustOwnedPathKind::Unsupported
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RustOwnedPathKind {
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
    ReScript,
    Swift,
    Unsupported,
}

fn rust_owned_path_kind(file_path: &str) -> RustOwnedPathKind {
    if ends_with_ascii_ignore_case(file_path, ".md")
        || ends_with_ascii_ignore_case(file_path, ".markdown")
    {
        RustOwnedPathKind::Markdown
    } else if ends_with_ascii_ignore_case(file_path, ".tf")
        || ends_with_ascii_ignore_case(file_path, ".tfvars")
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
    } else if ends_with_ascii_ignore_case(file_path, ".res")
        || ends_with_ascii_ignore_case(file_path, ".resi")
    {
        RustOwnedPathKind::ReScript
    } else if ends_with_ascii_ignore_case(file_path, ".swift") {
        RustOwnedPathKind::Swift
    } else {
        RustOwnedPathKind::Unsupported
    }
}

fn rust_owned_path_kind_for_source(file_path: &str, source: &[u8]) -> RustOwnedPathKind {
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

fn ends_with_ascii_ignore_case(value: &str, suffix: &str) -> bool {
    let bytes = value.as_bytes();
    let suffix = suffix.as_bytes();
    bytes
        .get(bytes.len().saturating_sub(suffix.len())..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
}

fn sha256_hex(source: &[u8]) -> String {
    let digest = Sha256::digest(source);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[derive(Clone, Debug)]
struct TerraformBlock {
    kind: String,
    labels: Vec<String>,
    body: String,
    line_start: i64,
    line_end: i64,
    body_start_line: i64,
    attrs: Option<Vec<TerraformAttr>>,
    calls: Option<Vec<String>>,
    references: Option<Vec<String>>,
    provider_sources: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
struct TerraformAttr {
    name: String,
    value: String,
    text: String,
    line_start: i64,
    line_end: i64,
    calls: Option<Vec<String>>,
    references: Option<Vec<String>>,
}

struct TerraformNodeSpec<'a> {
    kind: &'a str,
    name: &'a str,
    line_start: i64,
    line_end: i64,
    is_test: bool,
    terraform_kind: &'a str,
}

fn new_terraform_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::terraform_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_rust_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::rust_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_python_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::python_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_javascript_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::javascript_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_typescript_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::typescript_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_tsx_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::tsx_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_bash_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::bash_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_go_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&dagayn_grammars::go_language()).is_ok() {
        Some(parser)
    } else {
        None
    }
}

fn new_java_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::java_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_ruby_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::ruby_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_csharp_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::csharp_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_php_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::php_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_kotlin_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::kotlin_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_scala_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::scala_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_solidity_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::solidity_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_dart_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::dart_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_lua_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::lua_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_luau_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::luau_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_c_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&dagayn_grammars::c_language()).is_ok() {
        Some(parser)
    } else {
        None
    }
}

fn new_cpp_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::cpp_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_objc_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::objc_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_elixir_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::elixir_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_gdscript_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::gdscript_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_r_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&dagayn_grammars::r_language()).is_ok() {
        Some(parser)
    } else {
        None
    }
}

fn new_julia_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::julia_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_perl_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::perl_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_vue_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::vue_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_svelte_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::svelte_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_zig_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::zig_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_powershell_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::powershell_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn new_swift_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::swift_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn collect_terraform_blocks(
    source: &[u8],
    text: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> Vec<TerraformBlock> {
    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let mut blocks = Vec::new();
            collect_terraform_block_nodes(tree.root_node(), source, &mut blocks);
            if !blocks.is_empty() {
                return blocks;
            }
        }
    }
    collect_terraform_blocks_from_text(text)
}

fn collect_terraform_block_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    blocks: &mut Vec<TerraformBlock>,
) {
    if let Some(block) = terraform_block_from_node(node, source) {
        blocks.push(block);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_terraform_block_nodes(child, source, blocks);
    }
}

fn terraform_block_from_node(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<TerraformBlock> {
    let kind = terraform_kind_from_node_kind(node.kind())?.to_string();
    let labels = terraform_block_labels(node, source);
    let body_node = terraform_block_body_node(node);
    let body = body_node
        .map(|body| node_text(body, source))
        .unwrap_or_default();
    let body_start_line = body_node
        .map(|body| body.start_position().row as i64 + 1)
        .unwrap_or_else(|| node.start_position().row as i64 + 1);
    let body_calls = body_node.map(|body| collect_terraform_calls_from_tree(body, source));
    let body_references =
        body_node.map(|body| collect_terraform_references_from_tree(body, source));
    let provider_sources = body_node.map(|body| collect_terraform_provider_sources(body, source));

    Some(TerraformBlock {
        kind,
        labels,
        body,
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        body_start_line,
        attrs: body_node.map(|body| collect_terraform_attrs_from_tree(body, source)),
        calls: body_calls,
        references: body_references,
        provider_sources,
    })
}

fn terraform_kind_from_node_kind(node_kind: &str) -> Option<&'static str> {
    match node_kind {
        "terraform_block" => Some("terraform"),
        "provider_block" => Some("provider"),
        "variable_block" => Some("variable"),
        "locals_block" => Some("locals"),
        "module_block" => Some("module"),
        "data_block" => Some("data"),
        "resource_block" => Some("resource"),
        "check_block" => Some("check"),
        "output_block" => Some("output"),
        "import_block" => Some("import"),
        "moved_block" => Some("moved"),
        "removed_block" => Some("removed"),
        "ephemeral_block" => Some("ephemeral"),
        _ => None,
    }
}

fn terraform_block_labels(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut labels = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_lit" {
            labels.push(strip_tf_string(&node_text(child, source)));
        }
    }
    labels
}

fn terraform_block_body_node(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if let Some(body) = node.child_by_field_name("body") {
        return Some(body);
    }
    let mut cursor = node.walk();
    let body = node
        .children(&mut cursor)
        .find(|child| child.kind() == "block_body");
    body
}

fn collect_terraform_blocks_from_text(text: &str) -> Vec<TerraformBlock> {
    let mut blocks = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let Some(open_rel) = text[offset..].find('{') else {
            break;
        };
        let open = offset + open_rel;
        let header_start = text[..open]
            .rfind(['\n', '}'])
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let header = strip_terraform_line_comment(&text[header_start..open]).trim();
        let Some((kind, labels)) = parse_terraform_header(header) else {
            offset = open + 1;
            continue;
        };
        let Some(close) = find_matching_brace(text, open) else {
            break;
        };
        let body = text[open + 1..close].to_string();
        blocks.push(TerraformBlock {
            kind,
            labels,
            body,
            line_start: line_for_offset(text, header_start),
            line_end: line_for_offset(text, close),
            body_start_line: line_for_offset(text, open),
            attrs: None,
            calls: None,
            references: None,
            provider_sources: None,
        });
        offset = close + 1;
    }
    blocks
}

fn parse_terraform_header(header: &str) -> Option<(String, Vec<String>)> {
    if header.is_empty() || header.contains('=') {
        return None;
    }
    let tokens = TERRAFORM_HEADER_TOKEN_RE
        .captures_iter(header)
        .filter_map(|captures| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .or_else(|| captures.get(3))
                .map(|value| value.as_str().to_string())
        })
        .collect::<Vec<_>>();
    let (kind, labels) = tokens.split_first()?;
    let supported = matches!(
        kind.as_str(),
        "terraform"
            | "provider"
            | "variable"
            | "locals"
            | "module"
            | "data"
            | "resource"
            | "check"
            | "output"
            | "import"
            | "moved"
            | "removed"
            | "ephemeral"
    );
    supported.then(|| (kind.clone(), labels.to_vec()))
}

fn find_matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0_i64;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut in_line_comment = false;
    let mut chars = text.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if idx < open {
            continue;
        }
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = Some(ch);
            continue;
        }
        if ch == '#' {
            in_line_comment = true;
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '/') {
            in_line_comment = true;
            continue;
        }
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn collect_terraform_attrs(body: &str, body_start_line: i64) -> Vec<TerraformAttr> {
    let lines = body.lines().collect::<Vec<_>>();
    let mut attrs = Vec::new();
    let mut idx = 0_usize;
    while idx < lines.len() {
        let Some(captures) = TERRAFORM_ATTR_RE.captures(lines[idx]) else {
            idx += 1;
            continue;
        };
        let name = captures[1].to_string();
        let mut attr_lines = vec![lines[idx]];
        let mut depth = terraform_expr_depth(captures.get(2).map(|m| m.as_str()).unwrap_or(""));
        let start_idx = idx;
        idx += 1;
        while idx < lines.len() {
            let starts_next_attr = depth <= 0 && TERRAFORM_ATTR_RE.is_match(lines[idx]);
            if starts_next_attr {
                break;
            }
            if depth <= 0 && lines[idx].trim() == "}" {
                break;
            }
            attr_lines.push(lines[idx]);
            depth += terraform_expr_depth(lines[idx]);
            idx += 1;
            if depth <= 0
                && attr_lines
                    .last()
                    .is_some_and(|line| line.trim_end().ends_with('}'))
            {
                break;
            }
        }
        let text = attr_lines.join("\n");
        let value = TERRAFORM_ATTR_RE
            .captures(attr_lines[0])
            .and_then(|captures| captures.get(2))
            .map(|value| value.as_str().trim().to_string())
            .unwrap_or_default();
        attrs.push(TerraformAttr {
            name,
            value,
            text,
            line_start: body_start_line + start_idx as i64,
            line_end: body_start_line
                + start_idx as i64
                + attr_lines.len() as i64
                + i64::from(attr_lines.len() > 1)
                - 1,
            calls: None,
            references: None,
        });
    }
    attrs
}

fn terraform_attrs(block: &TerraformBlock) -> Cow<'_, [TerraformAttr]> {
    block.attrs.as_deref().map_or_else(
        || Cow::Owned(collect_terraform_attrs(&block.body, block.body_start_line)),
        Cow::Borrowed,
    )
}

fn terraform_provider_sources(block: &TerraformBlock) -> Cow<'_, [String]> {
    block.provider_sources.as_deref().map_or_else(
        || {
            Cow::Owned(
                TERRAFORM_PROVIDER_SOURCE_FALLBACK_RE
                    .captures_iter(&block.body)
                    .map(|captures| strip_tf_string(&captures[1]))
                    .collect(),
            )
        },
        Cow::Borrowed,
    )
}

fn collect_terraform_attrs_from_tree(
    body: tree_sitter::Node<'_>,
    source: &[u8],
) -> Vec<TerraformAttr> {
    let mut attrs = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() != "attribute" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let Some(value_node) = child.child_by_field_name("value") else {
            continue;
        };
        attrs.push(TerraformAttr {
            name: node_text(name_node, source),
            value: node_text(value_node, source).trim().to_string(),
            text: node_text(child, source),
            line_start: child.start_position().row as i64 + 1,
            line_end: child.end_position().row as i64 + 1,
            calls: Some(collect_terraform_calls_from_tree(child, source)),
            references: Some(collect_terraform_references_from_tree(child, source)),
        });
    }
    attrs
}

fn collect_terraform_provider_sources(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut sources = Vec::new();
    collect_terraform_provider_source_nodes(node, source, &mut sources);
    dedupe_strings(sources)
}

fn collect_terraform_provider_source_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    sources: &mut Vec<String>,
) {
    if node.kind() == "attribute" {
        if let (Some(name), Some(value)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("value"),
        ) {
            if node_text_is(name, source, "source") {
                sources.push(strip_tf_string(&node_text(value, source)));
            }
        }
    } else if node.kind() == "object_elem" {
        if let (Some(key), Some(value)) = (
            node.child_by_field_name("key"),
            node.child_by_field_name("value"),
        ) {
            if node_text_tf_string_is(key, source, "source") {
                sources.push(strip_tf_string(&node_text(value, source)));
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_terraform_provider_source_nodes(child, source, sources);
    }
}

fn collect_terraform_calls_from_tree(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut calls = Vec::new();
    collect_terraform_call_nodes(node, source, &mut calls);
    dedupe_strings(calls)
}

fn collect_terraform_call_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    calls: &mut Vec<String>,
) {
    if node.kind() == "function_call" {
        if let Some(name) = node.child_by_field_name("name") {
            let name = node_text(name, source);
            if !matches!(name.as_str(), "for" | "if") {
                calls.push(name);
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_terraform_call_nodes(child, source, calls);
    }
}

fn collect_terraform_references_from_tree(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Vec<String> {
    let mut references = Vec::new();
    collect_terraform_reference_nodes(node, source, &mut references);
    dedupe_strings(references)
}

fn collect_terraform_reference_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    references: &mut Vec<String>,
) {
    if matches!(node.kind(), "template_expr" | "quoted_template") {
        references.extend(collect_terraform_reference_targets(&node_text(
            node, source,
        )));
    }
    if node.kind() == "expression" {
        if let Some(segments) = terraform_traversal_segments(node, source) {
            if let Some(target) = terraform_reference_from_segments(&segments) {
                references.push(target);
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_terraform_reference_nodes(child, source, references);
    }
}

fn terraform_traversal_segments(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    if node.kind() == "variable_expr" {
        return node
            .child(0)
            .filter(|child| child.kind() == "identifier")
            .map(|identifier| vec![node_text(identifier, source)]);
    }

    if node.kind() != "expression" {
        return None;
    }

    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    if children.len() == 1 {
        return terraform_traversal_segments(children[0], source);
    }

    let mut segments = terraform_traversal_segments(*children.first()?, source)?;
    for child in children.iter().skip(1) {
        if child.kind() != "get_attr" {
            return None;
        }
        let name = child.child_by_field_name("name")?;
        segments.push(node_text(name, source));
    }
    Some(segments)
}

fn terraform_reference_from_segments(segments: &[String]) -> Option<String> {
    let root = segments.first()?.as_str();
    if root == "data" {
        return segments
            .get(1)
            .zip(segments.get(2))
            .map(|(block_type, name)| format!("data.{block_type}.{name}"));
    }
    if matches!(
        root,
        "module" | "var" | "local" | "output" | "provider" | "check"
    ) {
        return segments.get(1).map(|name| format!("{root}.{name}"));
    }
    if matches!(
        root,
        "count" | "each" | "ingress" | "egress" | "path" | "self" | "terraform"
    ) {
        return None;
    }
    segments
        .get(1)
        .map(|name| format!("resource.{root}.{name}"))
}

fn dedupe_strings(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn terraform_expr_depth(text: &str) -> i64 {
    let mut depth = 0_i64;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    for ch in strip_terraform_line_comment(text).chars() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = Some(ch);
            continue;
        }
        if matches!(ch, '{' | '[' | '(') {
            depth += 1;
        } else if matches!(ch, '}' | ']' | ')') {
            depth -= 1;
        }
    }
    depth
}

fn terraform_defined_name(block: &TerraformBlock) -> Option<String> {
    match block.kind.as_str() {
        "terraform" => Some("terraform".to_string()),
        "provider" => block.labels.first().map(|name| format!("provider.{name}")),
        "variable" => block.labels.first().map(|name| format!("var.{name}")),
        "module" => block.labels.first().map(|name| format!("module.{name}")),
        "data" => block
            .labels
            .first()
            .zip(block.labels.get(1))
            .map(|(block_type, name)| format!("data.{block_type}.{name}")),
        "resource" => block
            .labels
            .first()
            .zip(block.labels.get(1))
            .map(|(block_type, name)| format!("resource.{block_type}.{name}")),
        "ephemeral" => block
            .labels
            .first()
            .zip(block.labels.get(1))
            .map(|(block_type, name)| format!("ephemeral.{block_type}.{name}")),
        "output" => block.labels.first().map(|name| format!("output.{name}")),
        "check" => block.labels.first().map(|name| format!("check.{name}")),
        _ => None,
    }
}

fn terraform_kind_for_block(block: &TerraformBlock) -> &str {
    match block.kind.as_str() {
        "terraform" => "terraform",
        "provider" => "provider",
        "variable" => "variable",
        "module" => "module",
        "data" => "data",
        "resource" => "resource",
        "ephemeral" => "ephemeral",
        "output" => "output",
        "check" => "check",
        other => other,
    }
}

fn push_terraform_node(
    file_path: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
    spec: TerraformNodeSpec<'_>,
) {
    let qualified = terraform_qualified(file_path, spec.name);
    nodes.push(ParsedNode {
        kind: spec.kind.to_string(),
        name: spec.name.to_string(),
        file_path: file_path.to_string(),
        line_start: spec.line_start,
        line_end: spec.line_end,
        language: "terraform".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: spec.is_test,
        extra: json!({"terraform_kind": spec.terraform_kind}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualified,
        file_path: file_path.to_string(),
        line: spec.line_start,
        extra: json!({}),
    });
}

fn handle_terraform_meta_block(
    file_path: &str,
    block: &TerraformBlock,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    let attrs = terraform_attrs(block);
    let attr_value = |name: &str| {
        attrs
            .iter()
            .find(|attr| attr.name == name)
            .map(|attr| strip_tf_string(&attr.value))
    };
    match block.kind.as_str() {
        "import" => {
            if let Some(target) = attr_value("id").or_else(|| attr_value("to")) {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({}),
                });
            }
        }
        "moved" => {
            if let (Some(source), Some(target)) = (attr_value("from"), attr_value("to")) {
                edges.push(ParsedEdge {
                    kind: "REFERENCES".to_string(),
                    source,
                    target,
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({"terraform_kind": "moved"}),
                });
            }
        }
        "removed" => {
            if let Some(target) = attr_value("from") {
                edges.push(ParsedEdge {
                    kind: "REFERENCES".to_string(),
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.to_string(),
                    line: block.line_start,
                    extra: json!({"terraform_kind": "removed"}),
                });
            }
        }
        _ => {}
    }
    scan_terraform_block(
        block,
        file_path,
        file_path,
        block.line_start,
        defined_names,
        edges,
    );
}

fn scan_terraform_body(
    body: &str,
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    collect_terraform_calls(body, caller, file_path, line, edges);
    collect_terraform_references(body, caller, file_path, line, defined_names, edges);
}

fn scan_terraform_block(
    block: &TerraformBlock,
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    if let (Some(calls), Some(references)) = (&block.calls, &block.references) {
        push_terraform_calls(calls, caller, file_path, line, edges);
        push_terraform_references(references, caller, file_path, line, defined_names, edges);
    } else {
        scan_terraform_body(&block.body, caller, file_path, line, defined_names, edges);
    }
}

fn scan_terraform_attr(
    attr: &TerraformAttr,
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    if let (Some(calls), Some(references)) = (&attr.calls, &attr.references) {
        push_terraform_calls(calls, caller, file_path, line, edges);
        push_terraform_references(references, caller, file_path, line, defined_names, edges);
    } else {
        scan_terraform_body(&attr.text, caller, file_path, line, defined_names, edges);
    }
}

fn collect_terraform_calls(
    text: &str,
    caller: &str,
    file_path: &str,
    line: i64,
    edges: &mut Vec<ParsedEdge>,
) {
    let calls = TERRAFORM_CALL_RE
        .captures_iter(text)
        .map(|captures| captures[1].to_string())
        .filter(|name| !matches!(name.as_str(), "for" | "if"))
        .collect::<Vec<_>>();
    push_terraform_calls(&calls, caller, file_path, line, edges);
}

fn push_terraform_calls(
    calls: &[String],
    caller: &str,
    file_path: &str,
    line: i64,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut seen = HashSet::new();
    for name in calls {
        if !seen.insert(name.clone()) {
            continue;
        }
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.to_string(),
            target: name.clone(),
            file_path: file_path.to_string(),
            line,
            extra: json!({}),
        });
    }
}

fn collect_terraform_references(
    text: &str,
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    let references = collect_terraform_reference_targets(text);
    push_terraform_references(&references, caller, file_path, line, defined_names, edges);
}

fn collect_terraform_reference_targets(text: &str) -> Vec<String> {
    TERRAFORM_REFERENCE_RE
        .captures_iter(text)
        .filter_map(|captures| {
            let target = if captures.get(1).is_some() {
                format!("data.{}.{}", &captures[2], &captures[3])
            } else if captures.get(4).is_some() {
                format!("{}.{}", &captures[4], &captures[5])
            } else {
                let root = &captures[6];
                if matches!(
                    root,
                    "count" | "each" | "ingress" | "egress" | "path" | "self" | "terraform"
                ) {
                    return None;
                }
                format!("resource.{}.{}", root, &captures[7])
            };
            Some(target)
        })
        .collect::<Vec<_>>()
}

fn push_terraform_references(
    references: &[String],
    caller: &str,
    file_path: &str,
    line: i64,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut seen = HashSet::new();
    for target in references {
        if target == caller || !seen.insert(target.clone()) {
            continue;
        }
        let resolved = if defined_names.contains(target) {
            terraform_qualified(file_path, target)
        } else {
            target.clone()
        };
        edges.push(ParsedEdge {
            kind: "REFERENCES".to_string(),
            source: caller.to_string(),
            target: resolved,
            file_path: file_path.to_string(),
            line,
            extra: json!({}),
        });
    }
}

fn strip_tf_string(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn strip_terraform_line_comment(line: &str) -> &str {
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut prev = '\0';
    for (idx, ch) in line.char_indices() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            prev = ch;
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = Some(ch);
        } else if ch == '#' || (prev == '/' && ch == '/') {
            let start = if ch == '/' {
                idx.saturating_sub(1)
            } else {
                idx
            };
            return &line[..start];
        }
        prev = ch;
    }
    line
}

fn terraform_qualified(file_path: &str, name: &str) -> String {
    format!("{file_path}::{name}")
}

fn new_markdown_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::markdown_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

fn collect_markdown_headings(
    source: &[u8],
    text: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> Vec<Heading> {
    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            let headings = collect_markdown_headings_from_tree(tree.root_node(), source);
            if !headings.is_empty() {
                return headings;
            }
        }
    }
    collect_markdown_headings_from_text(text)
}

fn collect_markdown_headings_from_tree(root: tree_sitter::Node<'_>, source: &[u8]) -> Vec<Heading> {
    let mut raw = Vec::new();
    collect_markdown_heading_nodes(root, source, &mut raw);
    assign_heading_slugs(raw)
}

fn collect_markdown_heading_nodes(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    raw: &mut Vec<(String, i64, i64)>,
) {
    if matches!(node.kind(), "atx_heading" | "setext_heading") {
        let text = markdown_heading_text(node, source);
        if !text.is_empty() {
            raw.push((
                text,
                markdown_heading_level(node, source),
                node.start_position().row as i64 + 1,
            ));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_markdown_heading_nodes(child, source, raw);
    }
}

fn markdown_heading_level(node: tree_sitter::Node<'_>, source: &[u8]) -> i64 {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind.starts_with("atx_h") && kind.ends_with("_marker") {
            return node_text(child, source).chars().count() as i64;
        }
        if kind == "setext_h1_underline" {
            return 1;
        }
        if kind == "setext_h2_underline" {
            return 2;
        }
    }
    1
}

fn markdown_heading_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "atx_h1_marker"
                | "atx_h2_marker"
                | "atx_h3_marker"
                | "atx_h4_marker"
                | "atx_h5_marker"
                | "atx_h6_marker"
                | "setext_h1_underline"
                | "setext_h2_underline"
        ) {
            continue;
        }
        let text = node_text(child, source).trim().to_string();
        if !text.is_empty() {
            parts.push(text);
        }
    }
    parts.join(" ").trim().to_string()
}

fn node_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let text = node_text_bytes(node, source);
    match std::str::from_utf8(text) {
        Ok(text) => text.to_owned(),
        Err(_) => String::from_utf8_lossy(text).into_owned(),
    }
}

fn node_text_bytes<'source>(node: tree_sitter::Node<'_>, source: &'source [u8]) -> &'source [u8] {
    &source[node.start_byte()..node.end_byte()]
}

fn node_text_is(node: tree_sitter::Node<'_>, source: &[u8], expected: &str) -> bool {
    node_text_bytes(node, source) == expected.as_bytes()
}

fn node_text_tf_string_is(node: tree_sitter::Node<'_>, source: &[u8], expected: &str) -> bool {
    let text = trim_ascii_bytes(node_text_bytes(node, source));
    let unquoted = match text {
        [b'"', inner @ .., b'"'] | [b'\'', inner @ .., b'\''] => inner,
        _ => text,
    };
    unquoted == expected.as_bytes()
}

fn trim_ascii_bytes(mut value: &[u8]) -> &[u8] {
    while let Some((first, rest)) = value.split_first() {
        if !first.is_ascii_whitespace() {
            break;
        }
        value = rest;
    }
    while let Some((last, rest)) = value.split_last() {
        if !last.is_ascii_whitespace() {
            break;
        }
        value = rest;
    }
    value
}

fn line_count(source: &[u8]) -> i64 {
    memchr::memchr_iter(b'\n', source).count() as i64 + 1
}

fn collect_markdown_headings_from_text(text: &str) -> Vec<Heading> {
    let mut raw = Vec::new();
    let lines = text.lines().collect::<Vec<_>>();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx];
        let stripped = line.trim();
        if stripped.starts_with('#') {
            let marker = stripped.chars().take_while(|char| *char == '#').count();
            if (1..=6).contains(&marker)
                && stripped.len() > marker
                && stripped.as_bytes().get(marker) == Some(&b' ')
            {
                let title = stripped[marker + 1..].trim().trim_end_matches('#').trim();
                if !title.is_empty() {
                    raw.push((title.to_string(), marker as i64, idx as i64 + 1));
                }
            }
        } else if idx + 1 < lines.len() {
            let underline = lines[idx + 1].trim();
            if !stripped.is_empty()
                && !underline.is_empty()
                && underline.chars().all(|char| char == '=')
            {
                raw.push((stripped.to_string(), 1, idx as i64 + 1));
                idx += 1;
            } else if !stripped.is_empty()
                && !underline.is_empty()
                && underline.chars().all(|char| char == '-')
            {
                raw.push((stripped.to_string(), 2, idx as i64 + 1));
                idx += 1;
            }
        }
        idx += 1;
    }
    assign_heading_slugs(raw)
}

fn assign_heading_slugs(raw: Vec<(String, i64, i64)>) -> Vec<Heading> {
    let mut counts = HashMap::<String, usize>::new();
    let mut assigned = std::collections::HashSet::<String>::new();
    raw.into_iter()
        .map(|(text, level, line)| {
            let base = markdown_slugify(&text);
            let n = counts.get(&base).copied().unwrap_or(0);
            let slug = if n == 0 && !assigned.contains(&base) {
                base.clone()
            } else {
                let mut k = n.max(1);
                loop {
                    let candidate = format!("{base}-{k}");
                    if !assigned.contains(&candidate) {
                        break candidate;
                    }
                    k += 1;
                }
            };
            counts.insert(base, n + 1);
            assigned.insert(slug.clone());
            Heading {
                text,
                slug,
                level,
                line,
            }
        })
        .collect()
}

fn markdown_slugify(text: &str) -> String {
    let mut out = String::new();
    for char in text.chars() {
        if char.is_alphanumeric() {
            out.extend(char.to_lowercase());
        } else if char == ' ' || char == '-' {
            out.push('-');
        } else if char == '_' {
            out.push('_');
        }
    }
    out
}

fn extract_markdown_directives(
    line_context: &MarkdownLineContext<'_>,
    text: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut lines = LineCursor::new(text);
    for captures in MARKDOWN_DIRECTIVE_RE.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        let kind = captures[1].to_ascii_lowercase();
        let raw_target = captures[2].trim();
        let line = lines.line_for_offset(matched.start());
        let source = line_context.source_for_line(line);
        let file_path = line_context.file_path;
        let Some(target) = markdown_target(raw_target, file_path) else {
            continue;
        };
        edges.push(ParsedEdge {
            kind: "DEPENDS_ON".to_string(),
            source,
            target: target.clone(),
            file_path: file_path.to_string(),
            line,
            extra: json!({"markdown_directive_kind": kind}),
        });
        let target_file = target
            .split_once("::")
            .map(|(target_file, _)| target_file)
            .unwrap_or(target.as_str());
        if target_file != file_path {
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: file_path.to_string(),
                target: target_file.to_string(),
                file_path: file_path.to_string(),
                line,
                extra: json!({
                    "markdown_import_kind": "directive",
                    "markdown_directive_kind": kind,
                }),
            });
        }
    }
}

fn extract_markdown_links(
    line_context: &MarkdownLineContext<'_>,
    text: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut lines = LineCursor::new(text);
    for captures in MARKDOWN_INLINE_LINK_RE
        .captures_iter(text)
        .chain(MARKDOWN_REF_LINK_RE.captures_iter(text))
    {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        let raw_target = normalize_link_target(&captures[1]);
        if raw_target.is_empty() || is_external_target(&raw_target) {
            continue;
        }
        let line = lines.line_for_offset(matched.start());
        let source = line_context.source_for_line(line);
        let file_path = line_context.file_path;
        let Some(target) = markdown_target(&raw_target, file_path) else {
            continue;
        };
        if let Some((target_file, _target_section)) = target.split_once("::") {
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: file_path.to_string(),
                target: target_file.to_string(),
                file_path: file_path.to_string(),
                line,
                extra: json!({"markdown_import_kind": "link"}),
            });
            edges.push(ParsedEdge {
                kind: "REFERENCES".to_string(),
                source,
                target,
                file_path: file_path.to_string(),
                line,
                extra: json!({"markdown_reference_kind": "link"}),
            });
        } else if target != file_path {
            edges.push(ParsedEdge {
                kind: "IMPORTS_FROM".to_string(),
                source: file_path.to_string(),
                target,
                file_path: file_path.to_string(),
                line,
                extra: json!({"markdown_import_kind": "link"}),
            });
        }
    }
}

fn extract_markdown_code_spans(
    line_context: &MarkdownLineContext<'_>,
    text: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut seen = std::collections::HashSet::new();
    let mut lines = LineCursor::new(text);
    for captures in MARKDOWN_CODE_SPAN_RE.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        let sym = captures[1].trim();
        if sym.len() < 3 || !MARKDOWN_SYMBOL_RE.is_match(sym) {
            continue;
        }
        if !sym.contains('_') && !sym.contains('.') && sym.len() < 10 {
            continue;
        }
        let line = lines.line_for_offset(matched.start());
        let source = line_context.source_for_line(line);
        let file_path = line_context.file_path;
        if !seen.insert((source.clone(), sym.to_string(), line)) {
            continue;
        }
        edges.push(ParsedEdge {
            kind: "CROSS_ARTIFACT".to_string(),
            source,
            target: format!("<unresolved:{sym}>"),
            file_path: file_path.to_string(),
            line,
            extra: json!({
                "relationship_role": "describes_symbol",
                "bridge_kind": "documentation",
                "evidence_kind": "markdown_code_span",
                "evidence_source": "code_span",
                "source_language": "markdown",
                "target_language": "unknown",
                "confidence": 0.2,
                "confidence_tier": "LOW",
                "original_symbol_name": sym,
            }),
        });
    }
}

fn normalize_link_target(target: &str) -> String {
    let mut target = target.trim().to_string();
    if target.is_empty() {
        return String::new();
    }
    if let Some(matched) = MARKDOWN_TITLE_RE.find(&target) {
        target = target[..matched.start()].trim_end().to_string();
    }
    if target.starts_with('<') && target.ends_with('>') {
        target = target[1..target.len() - 1].trim().to_string();
    }
    target
}

fn is_external_target(target: &str) -> bool {
    let lowered = target.to_ascii_lowercase();
    lowered.starts_with("http://")
        || lowered.starts_with("https://")
        || lowered.starts_with("mailto:")
        || lowered.starts_with("tel:")
}

fn markdown_target(raw_target: &str, source_file: &str) -> Option<String> {
    let raw_target = raw_target.trim();
    if raw_target.is_empty() || raw_target.starts_with('/') {
        return None;
    }
    if let Some(section) = raw_target.strip_prefix('#') {
        let slug = markdown_slugify(section.trim());
        return (!slug.is_empty()).then(|| format!("{source_file}::{slug}"));
    }

    let (path_part, section_part) = raw_target
        .split_once('#')
        .map(|(path, section)| (path, Some(section.trim())))
        .unwrap_or((raw_target, None));
    let source = Path::new(source_file);
    let target_path = normalize_relative_path(
        &source
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(path_part),
    );
    if let Some(section_part) = section_part {
        let slug = markdown_slugify(section_part);
        if !slug.is_empty() {
            return Some(format!("{target_path}::{slug}"));
        }
    }
    Some(target_path)
}

fn line_for_offset(text: &str, offset: usize) -> i64 {
    text.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as i64
        + 1
}

fn normalize_relative_path(path: &Path) -> String {
    let mut parts = Vec::<String>::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(part) => {
                parts.push(part.to_string_lossy().to_string());
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                parts.push(component.as_os_str().to_string_lossy().to_string());
            }
        }
    }
    parts.join("/")
}

fn dedupe_edges(edges: Vec<ParsedEdge>) -> Vec<ParsedEdge> {
    let mut seen = std::collections::HashSet::new();
    edges
        .into_iter()
        .filter(|edge| {
            seen.insert((
                edge.kind.clone(),
                edge.source.clone(),
                edge.target.clone(),
                edge.line,
            ))
        })
        .collect()
}

fn is_test_file(file_path: &str) -> bool {
    contains_ascii_ignore_case(file_path, "/test/")
        || contains_ascii_ignore_case(file_path, "/tests/")
        || starts_with_ascii_ignore_case(file_path, "test/")
        || starts_with_ascii_ignore_case(file_path, "tests/")
        || starts_with_ascii_ignore_case(file_path, "test_")
        || ends_with_ascii_ignore_case(file_path, "_test.md")
        || ends_with_ascii_ignore_case(file_path, ".test.md")
        || ends_with_ascii_ignore_case(file_path, "_test.py")
        || ends_with_ascii_ignore_case(file_path, ".test.py")
        || ends_with_ascii_ignore_case(file_path, ".spec.py")
}

fn starts_with_ascii_ignore_case(value: &str, prefix: &str) -> bool {
    value
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn contains_ascii_ignore_case(value: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    !needle.is_empty()
        && value
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

fn build_globset(patterns: &[String]) -> Option<globset::GlobSet> {
    let mut builder = GlobSetBuilder::new();
    let mut added = false;
    for pattern in patterns {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
            added = true;
        }
    }
    added.then(|| builder.build().ok()).flatten()
}

fn load_ignore_patterns(repo_root: &Path) -> Vec<String> {
    let mut patterns = default_ignore_patterns()
        .iter()
        .map(|pattern| pattern.to_string())
        .collect::<Vec<_>>();
    let ignore_file = repo_root.join(".dagaynignore");
    if let Ok(raw) = std::fs::read_to_string(ignore_file) {
        patterns.extend(
            raw.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_string),
        );
    }
    patterns
}

fn get_git_tracked_files(
    repo_root: &Path,
    recurse_submodules: Option<bool>,
) -> Option<Vec<String>> {
    if !repo_root.join(".git").exists() {
        return None;
    }
    let mut cmd = Command::new("git");
    cmd.arg("ls-files");
    if recurse_submodules.unwrap_or(false) {
        cmd.arg("--recurse-submodules");
    }
    let output = cmd.current_dir(repo_root).output().ok()?;
    if !output.status.success() {
        return Some(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(
        stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn walk_files(
    repo_root: &Path,
    ignore_patterns: &[String],
    globset: Option<&globset::GlobSet>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![repo_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel_path = path
                .strip_prefix(repo_root)
                .ok()
                .map(|rel| rel.to_string_lossy().replace('\\', "/"));
            if path.is_dir() {
                if rel_path
                    .as_deref()
                    .is_some_and(|rel| should_ignore(rel, ignore_patterns, globset))
                {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            if let Some(rel) = rel_path {
                out.push(rel);
            }
        }
    }
    out
}

fn should_ignore(path: &str, patterns: &[String], globset: Option<&globset::GlobSet>) -> bool {
    if globset.is_some_and(|set| set.is_match(path)) {
        return true;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    for pattern in patterns {
        let Some(prefix) = pattern.strip_suffix("/**") else {
            continue;
        };
        if prefix.is_empty() || prefix.contains('/') {
            continue;
        }
        if parts.contains(&prefix) {
            return true;
        }
    }
    false
}

fn is_binary(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return true,
    };
    let mut head = [0_u8; 8192];
    match file.read(&mut head) {
        Ok(size) => head[..size].contains(&0),
        Err(_) => true,
    }
}

fn detect_language_from_shebang(path: &Path) -> Option<&'static str> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0_u8; 256];
    let size = file.read(&mut head).ok()?;
    detect_language_from_shebang_bytes(&head[..size])
}

fn detect_language_from_shebang_bytes(head: &[u8]) -> Option<&'static str> {
    if !head.starts_with(b"#!") {
        return None;
    }
    let first_line = head.split(|byte| *byte == b'\n').next()?;
    let first_line = first_line.split(|byte| *byte == 0).next()?;
    let line = std::str::from_utf8(&first_line[2..]).ok()?.trim();
    if line.is_empty() {
        return None;
    }
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    let first = tokens.first()?;
    let interpreter = if first.ends_with("/env") || *first == "env" {
        tokens
            .iter()
            .skip(1)
            .find(|token| !token.starts_with('-'))?
            .rsplit('/')
            .next()?
    } else {
        first.rsplit('/').next()?
    };
    SHEBANG_TO_LANGUAGE.get(interpreter).copied()
}

fn default_ignore_patterns() -> &'static [&'static str] {
    &[
        ".dagayn/**",
        "node_modules/**",
        ".git/**",
        ".svn/**",
        "__pycache__/**",
        "*.pyc",
        ".venv/**",
        "venv/**",
        "dist/**",
        "build/**",
        ".next/**",
        "target/**",
        "vendor/**",
        "bootstrap/cache/**",
        "public/build/**",
        ".bundle/**",
        ".gradle/**",
        "*.jar",
        ".dart_tool/**",
        ".pub-cache/**",
        "coverage/**",
        ".cache/**",
        "*.min.js",
        "*.min.css",
        "*.map",
        "*.lock",
        "package-lock.json",
        "yarn.lock",
        "*.db",
        "*.sqlite",
        "*.db-journal",
        "*.db-wal",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_extensions_and_shebangs() {
        assert_eq!(detect_language(Path::new("main.py")), Some("python"));
        assert_eq!(detect_language(Path::new("main.R")), Some("r"));
        assert_eq!(detect_language(Path::new("main.unknown")), None);
    }

    #[test]
    fn nested_dir_ignore_matches_python_behavior() {
        let patterns = vec!["node_modules/**".to_string()];
        assert!(should_ignore(
            "pkg/app/node_modules/react/index.js",
            &patterns,
            None
        ));
        assert!(should_ignore(
            "node_modules/react/index.js",
            &patterns,
            None
        ));
        assert!(!should_ignore("pkg/app/src/index.js", &patterns, None));
    }

    #[test]
    fn walk_files_prunes_ignored_directories() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-walk-ignore-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(repo_root.join("src")).unwrap();
        std::fs::create_dir_all(repo_root.join("pkg/node_modules/lib")).unwrap();
        std::fs::write(repo_root.join("src/main.py"), b"def main():\n    pass\n").unwrap();
        std::fs::write(
            repo_root.join("pkg/node_modules/lib/index.js"),
            b"export const slow = 1;\n",
        )
        .unwrap();

        let patterns = load_ignore_patterns(&repo_root);
        let globset = build_globset(&patterns);
        let files = walk_files(&repo_root, &patterns, globset.as_ref());

        assert!(files.contains(&"src/main.py".to_string()));
        assert!(!files.iter().any(|file| file.contains("node_modules")));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn parses_markdown_sections_and_edges() {
        let source = b"# API Reference

<!-- derived-from ./guide.md#Installation -->

See [Getting Started](./guide.md#Getting-Started).

## Endpoints

Call `build_graph`.
";
        let (nodes, edges) = parse_markdown("api.md", source);
        assert_eq!(nodes.len(), 3);
        assert!(nodes.iter().any(|node| node.name == "api-reference"));
        assert!(nodes.iter().any(|node| node.name == "endpoints"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "DEPENDS_ON"
                && edge.source == "api.md::api-reference"
                && edge.target == "guide.md::installation"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "api.md::api-reference"
                && edge.target == "guide.md::getting-started"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT" && edge.target == "<unresolved:build_graph>"
        }));
    }

    #[test]
    fn parses_terraform_blocks_calls_and_refs() {
        let source = br#"terraform {
  required_providers {
    aws = {
      source = "hashicorp/aws"
    }
  }
}

variable "tags" {
  type = map(string)
}

locals {
  common_tags = merge(var.tags, {
    ManagedBy = "dagayn"
  })
}

module "network" {
  source = "./modules/network"
}

data "aws_caller_identity" "current" {}

resource "aws_vpc" "main" {
  cidr_block = module.network.cidr_block
  tags = merge(local.common_tags, {
    Account = data.aws_caller_identity.current.account_id
  })
}

check "vpc_ready" {
  assert {
    condition = length(module.network.public_subnet_ids) > 0
  }
}

output "vpc_id" {
  value = aws_vpc.main.id
}
"#;
        let (nodes, edges) = parse_terraform("main.tf", source);
        let names = nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"terraform"));
        assert!(names.contains(&"var.tags"));
        assert!(names.contains(&"local.common_tags"));
        assert!(names.contains(&"module.network"));
        assert!(names.contains(&"data.aws_caller_identity.current"));
        assert!(names.contains(&"resource.aws_vpc.main"));
        assert!(names.contains(&"check.vpc_ready"));
        assert!(names.contains(&"output.vpc_id"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "DEPENDS_ON"
                && edge.source == "main.tf::terraform"
                && edge.target == "hashicorp/aws"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.source == "main.tf::module.network"
                && edge.target == "./modules/network"
        }));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "merge"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "resource.aws_vpc.main"
                && edge.target == "main.tf::data.aws_caller_identity.current"
        }));
    }

    #[test]
    fn parses_bash_functions_calls_and_sources() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-bash-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(repo_root.join("scripts")).unwrap();
        std::fs::write(repo_root.join("scripts/lib.sh"), b"helper() { echo ok; }\n").unwrap();

        let source = br#"#!/usr/bin/env bash
source ./lib.sh

greet() {
  echo "hi"
}

main() {
  greet
}

main "$@"
"#;
        let mut parser = RustOwnedParser::new();
        let (nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "scripts/app.sh", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "greet"
                && node.language == "bash"
                && node.file_path == "scripts/app.sh"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "main"
                && node.language == "bash"
                && node.file_path == "scripts/app.sh"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.source == "scripts/app.sh"
                && edge.target == "scripts/lib.sh"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "scripts/app.sh::main"
                && edge.target == "scripts/app.sh::greet"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "scripts/app.sh"
                && edge.target == "scripts/app.sh::main"
        }));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn parses_extensionless_shebang_script_as_rust_owned() {
        let source = br#"#!/usr/bin/env bash
deploy() {
  echo "deploy"
}

deploy "$@"
"#;
        assert!(!rust_parser_owns_path("bin/deploy"));
        assert!(rust_parser_owns_source("bin/deploy", source));

        let (nodes, edges) = parse_rust_owned_file("bin/deploy", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Function" && node.name == "deploy" && node.language == "bash"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "bin/deploy"
                && edge.target == "bin/deploy::deploy"
        }));
    }

    #[test]
    fn parses_go_types_methods_calls_and_bridges() {
        let source = br#"package main

import (
  "os"
  "os/exec"
  "plugin"
)

type Repo struct {}

func NewRepo() *Repo {
  return &Repo{}
}

func (r *Repo) Save() {
  os.WriteFile("output.json", []byte("ok"), 0644)
}

func runCommand(path string) {
  exec.Command("git", "status")
  os.ReadFile(path)
  plugin.Open("mylib.so")
}
"#;
        let (nodes, edges) = parse_go("main.go", source);
        assert!(nodes
            .iter()
            .any(|node| { node.kind == "Class" && node.name == "Repo" && node.language == "go" }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "Save"
                && node.parent_name.as_deref() == Some("Repo")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.source == "main.go" && edge.target == "os/exec"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CONTAINS"
                && edge.source == "main.go::Repo"
                && edge.target == "main.go::Repo.Save"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "git"
                && edge.extra["evidence_source"] == "exec.Command"
                && edge.extra["confidence_tier"] == "HIGH"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "<dynamic:os.ReadFile@main.go:21>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "mylib.so"
                && edge.extra["evidence_source"] == "plugin.Open"
        }));
    }

    #[test]
    fn parses_java_types_imports_calls_and_bridges() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-java-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(repo_root.join("src/main/java/com/example/util")).unwrap();
        std::fs::create_dir_all(repo_root.join("src/main/java/com/example/app")).unwrap();
        std::fs::write(
            repo_root.join("src/main/java/com/example/util/Helper.java"),
            b"package com.example.util;\npublic class Helper {}\n",
        )
        .unwrap();

        let source = br#"package com.example.app;

import static com.example.util.Helper.MAX;
import java.util.Map;

public interface Repository {
  void save(User user);
}

abstract class BaseRepo implements Repository {
  public void save(User user) {
    Runtime.getRuntime().exec("./bin/dagayn");
    Runtime.getRuntime().exec(command());
    System.loadLibrary("dagayn");
  }
}

class CachedRepo extends BaseRepo {
  public void save(User user) {
    super.save(user);
  }
}
"#;
        let mut parser = RustOwnedParser::new();
        let (nodes, edges) = parser.parse_file_in_repo(
            Some(&repo_root),
            "src/main/java/com/example/app/App.java",
            source,
        );
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "Repository"
                && node.extra["type_role"] == "interface"
                && node.extra["is_contract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "BaseRepo"
                && node.extra["type_role"] == "abstract_class"
                && node.extra["is_abstract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "save"
                && node.parent_name.as_deref() == Some("BaseRepo")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.target == "src/main/java/com/example/util/Helper.java"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPLEMENTS"
                && edge.source == "src/main/java/com/example/app/App.java::BaseRepo"
                && edge.target == "Repository"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS"
                && edge.source == "src/main/java/com/example/app/App.java::CachedRepo"
                && edge.target == "BaseRepo"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "./bin/dagayn"
                && edge.extra["evidence_source"] == "Runtime.getRuntime().exec"
                && edge.extra["confidence_tier"] == "HIGH"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target
                    == "<dynamic:Runtime.getRuntime().exec@src/main/java/com/example/app/App.java:13>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "dagayn"
                && edge.extra["evidence_source"] == "System.loadLibrary"
        }));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn parses_ruby_classes_calls_imports_and_bridges() {
        let source = br#"require 'json'

module Auth
  class UserRepository
    def save(user)
      File.write("output.json", "{}")
      puts "Saved #{user}"
    end

    def create_user(name)
      save(name)
    end
  end
end

def run_command(path)
  system("git status")
  File.read(path)
  Fiddle.dlopen("mylib.so")
end
"#;
        let (nodes, edges) = parse_ruby("app.rb", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "Auth"
                && node.parent_name.is_none()
                && node.language == "ruby"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "UserRepository"
                && node.parent_name.as_deref() == Some("Auth")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "save"
                && node.parent_name.as_deref() == Some("UserRepository")
                && node.params.is_none()
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.source == "app.rb" && edge.target == "json"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "app.rb::UserRepository.create_user"
                && edge.target == "app.rb::UserRepository.save"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "output.json"
                && edge.extra["evidence_source"] == "File.write"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "git status"
                && edge.extra["evidence_source"] == "system"
                && edge.extra["confidence_tier"] == "HIGH"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "<dynamic:File.read@app.rb:18>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "mylib.so"
                && edge.extra["evidence_source"] == "Fiddle.dlopen"
        }));
    }

    #[test]
    fn parses_csharp_types_imports_and_bridges() {
        let source = br#"using System.IO;
using System.Diagnostics;
using System.Reflection;

interface IRepository
{
    User FindById(int id);
    void Save(User user);
}

class BridgeSamples : IRepository
{
    public User FindById(int id)
    {
        return null;
    }

    public void Save(User user)
    {
        Process.Start("git", "status");
        File.ReadAllText(user.Path);
        Assembly.LoadFile("mylib.dll");
    }
}
"#;
        let (nodes, edges) = parse_csharp("sample.cs", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "IRepository"
                && node.extra["type_role"] == "interface"
                && node.extra["is_contract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "BridgeSamples"
                && node.extra["type_role"] == "class"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "User"
                && node.parent_name.as_deref() == Some("BridgeSamples")
                && node.params.as_deref() == Some("(int id)")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "Save"
                && node.parent_name.as_deref() == Some("BridgeSamples")
                && node.params.as_deref() == Some("(User user)")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.source == "sample.cs" && edge.target == "System.IO"
        }));
        assert!(edges.iter().all(|edge| edge.kind != "IMPLEMENTS"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "git"
                && edge.extra["evidence_source"] == "Process.Start"
                && edge.extra["confidence_tier"] == "HIGH"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "<dynamic:File.ReadAllText@sample.cs:21>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "mylib.dll"
                && edge.extra["evidence_source"] == "Assembly.LoadFile"
        }));
    }

    #[test]
    fn parses_php_types_calls_imports_and_bridges() {
        let source = br#"<?php
use Exception;

interface Repository {
    public function save(User $user): void;
}

class User {
    public function __construct(int $id) {}
    public function toString(): string { return "u"; }
}

class ExtendedRepo implements Repository {
    public function save(User $user): void {
        $user->toString();
        file_put_contents("output.json", "{}");
    }

    public function run($path): void {
        sqlQuery("SELECT 1");
        $this->save(new User(1));
        parent::__construct();
        FFI::cdef("", "mylib.so");
        file_get_contents($path);
    }
}

function sqlQuery(string $query): array { return []; }
"#;
        let (nodes, edges) = parse_php("sample.php", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "Repository"
                && node.extra["type_role"] == "interface"
                && node.extra["is_contract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "save"
                && node.parent_name.as_deref() == Some("Repository")
                && node.params.as_deref() == Some("(User $user)")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "sqlQuery"
                && node.parent_name.is_none()
                && node.params.as_deref() == Some("(string $query)")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.source == "sample.php"
                && edge.target == "use Exception;"
        }));
        assert!(edges.iter().all(|edge| edge.kind != "IMPLEMENTS"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.php::ExtendedRepo.save"
                && edge.target == "sample.php::User.toString"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.php::ExtendedRepo.run"
                && edge.target == "sample.php::sqlQuery"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "output.json"
                && edge.extra["evidence_source"] == "file_put_contents"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "<dynamic:FFI::cdef@sample.php:23>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "<dynamic:file_get_contents@sample.php:24>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
    }

    #[test]
    fn parses_kotlin_types_calls_imports_and_bridges() {
        let source = br#"import java.nio.file.Files

interface UserRepository {
    fun save(user: User)
}

class User(val id: Int)

class InMemoryRepo : UserRepository {
    fun save(user: User) {
        println(user)
        Files.writeString(java.nio.file.Path.of("output.txt"), "ok")
    }

    fun run(path: String) {
        Runtime.getRuntime().exec("git status")
        Files.readString(java.nio.file.Path.of(path))
        System.loadLibrary("mylib")
    }
}

fun createUser(repo: UserRepository) {
    val user = User(1)
    repo.save(user)
}
"#;
        let (nodes, edges) = parse_kotlin("sample.kt", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "UserRepository"
                && node.extra["type_role"] == "class"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "save"
                && node.parent_name.as_deref() == Some("InMemoryRepo")
                && node.params.is_none()
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.source == "sample.kt"
                && edge.target == "import java.nio.file.Files"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS"
                && edge.source == "sample.kt::InMemoryRepo"
                && edge.target == "InMemoryRepo"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.kt::createUser"
                && edge.target == "sample.kt::UserRepository.save"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "git status"
                && edge.extra["evidence_source"] == "Runtime.getRuntime().exec"
                && edge.extra["confidence_tier"] == "HIGH"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "<dynamic:Files.readString@sample.kt:17>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "mylib"
                && edge.extra["evidence_source"] == "System.loadLibrary"
        }));
    }

    #[test]
    fn parses_scala_types_calls_imports_and_bridges() {
        let source = br#"import scala.collection.mutable.{HashMap, ListBuffer}
import java.nio.file.Files

trait Repository[T]:
  def save(entity: T): Unit

case class User(id: Int)

class InMemoryRepo extends Repository[User] with Serializable:
  private val users = mutable.HashMap[Int, User]()

  override def save(user: User): Unit =
    users.put(user.id, user)
    Files.writeString(Path.of("output.json"), "{}")

object BridgeSamples:
  def runCommand(): Unit =
    Runtime.getRuntime().exec("git status")

  def loadLib(): Unit =
    System.loadLibrary("mylib")
"#;
        let (nodes, edges) = parse_scala("sample.scala", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "Repository"
                && node.extra["type_role"] == "trait"
                && node.extra["is_contract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "save"
                && node.parent_name.as_deref() == Some("InMemoryRepo")
                && node.params.as_deref() == Some("(user: User)")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.target == "scala.collection.mutable.HashMap"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPLEMENTS"
                && edge.source == "sample.scala::InMemoryRepo"
                && edge.target == "Serializable"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "sample.scala" && edge.target == "HashMap"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "git status"
                && edge.extra["evidence_source"] == "Runtime.getRuntime().exec"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "<dynamic:Files.writeString@sample.scala:14>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "mylib"
                && edge.extra["evidence_source"] == "System.loadLibrary"
        }));
    }

    #[test]
    fn parses_solidity_contracts_state_calls_and_imports() {
        let source = br#"import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

uint256 constant MAX_SUPPLY = 1_000_000 ether;

struct Position {
    address wallet;
}

interface IPool {
    function stake(uint256 amount) external;
}

library RewardMath {
    uint256 internal constant PRECISION = 1e18;
    function mulPrecise(uint256 a, uint256 b) internal pure returns (uint256) {
        require(b > 0, "zero");
        return (a * b) / PRECISION;
    }
}

contract Vault is ERC20, IPool {
    using RewardMath for uint256;

    mapping(address => uint256) public stakes;
    uint256 immutable launchTime;

    event Staked(address indexed user, uint256 amount);

    modifier nonZero(uint256 amount) {
        require(amount > 0, "zero");
        _;
    }

    constructor(string memory name) ERC20(name, "V") {
        launchTime = block.timestamp;
    }

    function stake(uint256 amount) external nonZero(amount) {
        stakes[msg.sender] += amount;
        _mint(msg.sender, amount);
        emit Staked(msg.sender, amount);
    }
}
"#;
        let (nodes, edges) = parse_solidity("sample.sol", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "IPool" && node.extra["type_role"] == "interface"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "stakes"
                && node.parent_name.as_deref() == Some("Vault")
                && node.return_type.as_deref() == Some("mapping(address => uint256)")
                && node.modifiers.as_deref() == Some("public")
                && node.extra["solidity_kind"] == "state_variable"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.target == "@openzeppelin/contracts/token/ERC20/ERC20.sol"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS" && edge.source == "sample.sol::Vault" && edge.target == "ERC20"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "DEPENDS_ON"
                && edge.source == "sample.sol::Vault"
                && edge.target == "RewardMath"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.sol::Vault.constructor"
                && edge.target == "ERC20"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.sol::Vault.stake"
                && edge.target == "sample.sol::Vault.nonZero"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.sol::Vault.stake"
                && edge.target == "sample.sol::Vault.Staked"
        }));
    }

    #[test]
    fn parses_dart_types_imports_and_calls() {
        let source = br#"import 'dart:async';

abstract class Animal {
  void speak();
}

mixin SwimmingMixin {
  void swim() => print('swimming');
}

enum PetType { dog, cat }

class Dog extends Animal with SwimmingMixin {
  void speak() {
    print('woof');
  }

  Future<void> fetch(String item) async {
    await _run();
    print(item);
  }

  void _run() {
    print('running');
  }

  static Dog create(String name) {
    return Dog(name);
  }
}

Dog createDog(String name) {
  return Dog(name);
}
"#;
        let (nodes, edges) = parse_dart("sample.dart", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "Animal"
                && node.extra["type_role"] == "abstract_class"
                && node.extra["is_abstract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "SwimmingMixin"
                && node.extra["type_role"] == "mixin"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "fetch"
                && node.parent_name.as_deref() == Some("Dog")
                && node.params.as_deref() == Some("(String item)")
        }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "dart:async" }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS" && edge.source == "sample.dart::Dog" && edge.target == "Animal"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS"
                && edge.source == "sample.dart::Dog"
                && edge.target == "SwimmingMixin"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.dart"
                && edge.target == "sample.dart::Dog._run"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.dart"
                && edge.target == "sample.dart::Dog"
        }));
    }

    #[test]
    fn parses_lua_functions_methods_imports_tests_and_bridges() {
        let source = br#"local json = require("cjson")
local log = require("logging").getLogger("sample")

function greet(name)
    print("Hello, " .. name)
    return name
end

local transform = function(data)
    return json.encode(data)
end

function Animal.new(name)
    return setmetatable({}, Animal)
end

function Animal:speak()
    log:info(self.name)
end

function Dog:fetch(item)
    self:speak()
    os.execute("git status")
    return item
end

local function test_greet()
    local result = greet("World")
    assert(result == "World")
end
"#;
        let (nodes, edges) = parse_lua("sample.lua", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "new"
                && node.parent_name.as_deref() == Some("Animal")
                && node.params.as_deref() == Some("(name)")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "fetch"
                && node.parent_name.as_deref() == Some("Dog")
        }));
        assert!(nodes
            .iter()
            .any(|node| { node.kind == "Test" && node.name == "test_greet" && node.is_test }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "cjson" }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.lua::Dog.fetch"
                && edge.target == "sample.lua::speak"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.source == "sample.lua::Dog.fetch"
                && edge.target == "git status"
                && edge.extra["evidence_source"] == "os.execute"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "TESTED_BY"
                && edge.source == "sample.lua::greet"
                && edge.target == "sample.lua::test_greet"
        }));
    }

    #[test]
    fn parses_luau_types_functions_methods_imports_and_tests() {
        let source = br#"local utils = require("lib.utils")
local log = require("logging").getLogger("sample")

type Vector3 = {
    x: number,
    y: number,
    z: number,
}

type Callback = (input: string) -> string

function greet(name: string): string
    print("Hello, " .. name)
    return name
end

local transform = function(data: any): string
    return utils.encode(data)
end

function Animal:speak(): string
    log:info(self.name)
    return self.name
end

local function test_greet()
    local result = greet("World")
    assert(result == "World")
end
"#;
        let (nodes, edges) = parse_luau("sample.luau", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "Vector3" && node.language == "luau"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "Callback" && node.language == "luau"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "greet"
                && node.language == "luau"
                && node.params.as_deref() == Some("(name: string)")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "speak"
                && node.parent_name.as_deref() == Some("Animal")
                && node.language == "luau"
        }));
        assert!(nodes
            .iter()
            .any(|node| { node.kind == "Test" && node.name == "test_greet" && node.is_test }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "lib.utils" }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.luau::test_greet"
                && edge.target == "sample.luau::greet"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "TESTED_BY"
                && edge.source == "sample.luau::greet"
                && edge.target == "sample.luau::test_greet"
        }));
    }

    #[test]
    fn parses_elixir_modules_functions_imports_and_calls() {
        let source = br#"defmodule Calculator do
  @moduledoc """
  Simple calculator module.
  """

  def add(a, b) do
    a + b
  end

  def subtract(a, b), do: a - b

  defp log(msg) do
    IO.puts(msg)
    :ok
  end

  def compute(a, b) do
    result = add(a, b)
    log("result: #{result}")
    result
  end
end

defmodule MathHelpers do
  alias Calculator
  import Calculator, only: [add: 2]
  require Logger

  def double(x) do
    Calculator.compute(x, x)
  end

  def triple(x) do
    double(x) + x
  end
end
"#;
        let (nodes, edges) = parse_elixir("sample.ex", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "Calculator" && node.language == "elixir"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "MathHelpers" && node.language == "elixir"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "add"
                && node.parent_name.as_deref() == Some("Calculator")
                && node.params.as_deref() == Some("(a, b)")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "log"
                && node.parent_name.as_deref() == Some("Calculator")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "triple"
                && node.parent_name.as_deref() == Some("MathHelpers")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.source == "sample.ex" && edge.target == "Logger"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.ex::Calculator.compute"
                && edge.target == "sample.ex::Calculator.add"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.ex::Calculator.compute"
                && edge.target == "sample.ex::Calculator.log"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.ex::MathHelpers.double"
                && edge.target == "sample.ex::Calculator.compute"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.ex::MathHelpers.triple"
                && edge.target == "sample.ex::MathHelpers.double"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "sample.ex" && edge.target == "moduledoc"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.ex::Calculator.log"
                && edge.target == "puts"
        }));
    }

    #[test]
    fn parses_gdscript_classes_functions_imports_and_calls() {
        let source = br#"extends Node
class_name SampleManager

const MAX_SIZE = 10
const OtherScript = preload("res://scripts/other.gd")

signal item_added(item: Item)

@export var speed: float = 2.5
@onready var timer: Timer = $Timer

var items: Array[Item] = []


class Item:
	var name: String
	var level: int

	func promote() -> void:
		level += 1


func _ready() -> void:
	timer.start()
	_load_items()
	OtherScript.register(self)


func _load_items() -> void:
	for i in range(MAX_SIZE):
		var item := Item.new()
		items.append(item)
		item_added.emit(item)


func get_item(idx: int) -> Item:
	return items[idx]


static func helper() -> int:
	return 42
"#;
        let (nodes, edges) = parse_gdscript("sample.gd", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "SampleManager"
                && node.language == "gdscript"
                && node.extra["type_role"] == "class"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "Item" && node.language == "gdscript"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "promote"
                && node.parent_name.as_deref() == Some("Item")
                && node.params.as_deref() == Some("()")
                && node.return_type.as_deref() == Some("void")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function" && node.name == "_load_items" && node.parent_name.is_none()
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "get_item"
                && node.params.as_deref() == Some("(idx: int)")
                && node.return_type.as_deref() == Some("Item")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.source == "sample.gd" && edge.target == "Node"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "sample.gd" && edge.target == "preload"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.gd::_ready"
                && edge.target == "sample.gd::_load_items"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "sample.gd::_ready" && edge.target == "start"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.gd::_load_items"
                && edge.target == "append"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CONTAINS"
                && edge.source == "sample.gd::Item"
                && edge.target == "sample.gd::Item.promote"
        }));
    }

    #[test]
    fn parses_r_functions_classes_imports_calls_and_bridges() {
        let source = br#"library(dplyr)
require(ggplot2)
source("utils.R")

add <- function(x, y) {
  x + y
}

multiply = function(a, b) {
  a * b
}

MyClass <- setRefClass("MyClass",
  fields = list(name = "character", age = "numeric"),
  methods = list(
    greet = function() {
      cat(paste("Hello", name))
    },
    get_age = function() {
      return(age)
    }
  )
)

process_data <- function(data) {
  result <- dplyr::filter(data, x > 5)
  summary <- dplyr::summarize(result, mean_x = mean(x))
  add(1, 2)
  summary
}
"#;
        let (nodes, edges) = parse_r("sample.R", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "add"
                && node.params.as_deref() == Some("(x, y)")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "MyClass" && node.language == "r"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "greet"
                && node.parent_name.as_deref() == Some("MyClass")
        }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "dplyr" }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "utils.R" }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.R::process_data"
                && edge.target == "dplyr::filter"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.R::process_data"
                && edge.target == "sample.R::add"
        }));

        let bridge_source = br#"system("./target/release/dagayn-core build .")
system2("./scripts/build.sh", args = c("--strict"))
.Call("dagayn_compute")
.External("dagayn_helper")
dyn.load("./target/release/libdagayn.so")
library.dynam("dagayn", "./target/release")

run_dynamic <- function(cmd) {
  system(cmd)
}
"#;
        let (_nodes, bridge_edges) = parse_r("bridge.R", bridge_source);
        let cross_edges = bridge_edges
            .iter()
            .filter(|edge| edge.kind == "CROSS_ARTIFACT")
            .collect::<Vec<_>>();
        assert_eq!(cross_edges.len(), 7);
        assert!(cross_edges.iter().any(|edge| {
            edge.target == "./target/release/libdagayn.so"
                && edge.extra["evidence_source"] == "dyn.load"
                && edge.extra["confidence_tier"] == "HIGH"
        }));
        assert!(cross_edges.iter().any(|edge| {
            edge.target == "<dynamic:system@bridge.R:9>"
                && edge.extra["evidence_source"] == "system"
                && edge.extra["confidence_tier"] == "LOW"
        }));
    }

    #[test]
    fn parses_julia_modules_types_functions_macros_and_bridges() {
        let source = br#"module SampleModule

using LinearAlgebra
using Statistics: mean, std
import Base: show, print
import JSON

export greet, Dog, process
public square, add

@enum Color RED BLUE GREEN

abstract type AbstractAnimal end

struct Dog <: AbstractAnimal
    name::String
    age::Int
end

mutable struct MutablePoint
    x::Float64
    y::Float64
end

function greet(name::String)
    println("Hello, $name")
end

function Base.show(io::IO, d::Dog)
    print(io, "Dog($(d.name))")
end

add(a, b) = a + b

square(x) = x^2

macro sayhello(name)
    :(println("Hello, ", $name))
end

function outer()
    function inner()
        return 1
    end
    x = inner()
    result = map(v -> v^2, [1,2,3])
    return x
end

function process(data::Vector{Float64}; verbose=false)
    if verbose
        println("Processing...")
    end
    normed = data ./ maximum(data)
    return sum(normed) / length(normed)
end

include("utils.jl")

@testset "Arithmetic" begin
    @test add(1, 2) == 3
    @test square(4) == 16
end

end # module
"#;
        let (nodes, edges) = parse_julia("sample.jl", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "SampleModule" && node.language == "julia"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "Color" && node.extra["julia_kind"] == "enum"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "GREEN"
                && node.parent_name.as_deref() == Some("Color")
                && node.extra["julia_kind"] == "enum_variant"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "AbstractAnimal"
                && node.extra["type_role"] == "abstract_type"
                && node.extra["is_abstract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "show"
                && node.parent_name.as_deref() == Some("SampleModule")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "inner"
                && node.parent_name.as_deref() == Some("SampleModule.outer")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Test"
                && node.name.starts_with("testset:Arithmetic@L")
                && node.parent_name.as_deref() == Some("SampleModule")
        }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "Statistics.mean" }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "utils.jl" }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS"
                && edge.source == "sample.jl::SampleModule.Dog"
                && edge.target == "AbstractAnimal"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "sample.jl::SampleModule.show"
                && edge.target == "Base"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.jl::SampleModule.outer"
                && edge.target == "sample.jl::SampleModule.outer.inner"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge
                    .source
                    .starts_with("sample.jl::SampleModule.testset:Arithmetic@L")
                && edge.target == "sample.jl::SampleModule.add"
        }));

        let bridge_source = br#"function run_command()
    run(`git status`)
end

function read_config()
    open("config.yaml", "r")
end

function write_output()
    write("output.json", "{}")
end

function load_lib()
    Libdl.dlopen("mylib.so")
end
"#;
        let (_nodes, bridge_edges) = parse_julia("bridge.jl", bridge_source);
        assert!(bridge_edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.extra["evidence_source"] == "run"
                && edge.extra["confidence_tier"] == "LOW"
        }));
        assert!(bridge_edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "mylib.so"
                && edge.extra["evidence_source"] == "Libdl.dlopen"
        }));
    }

    #[test]
    fn parses_perl_packages_subroutines_imports_calls_and_bridges() {
        let source = br#"use strict;
use warnings;
use File::Basename;

package Animal;

sub new {
    my ($class, %args) = @_;
    return bless \%args, $class;
}

sub speak {
    my ($self) = @_;
    return "...";
}

package Dog;

sub new {
    my ($class, %args) = @_;
    my $self = Animal::new($class, %args);
    return $self;
}

sub fetch {
    my ($self, $item) = @_;
    return "Fetched $item";
}

sub bark {
    my ($self) = @_;
    print $self->speak() . "\n";
}
"#;
        let (nodes, edges) = parse_perl("sample.pl", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "Animal"
                && node.language == "perl"
                && node.extra["type_role"] == "class"
        }));
        assert!(nodes
            .iter()
            .any(|node| { node.kind == "Class" && node.name == "Dog" }));
        assert!(nodes
            .iter()
            .any(|node| { node.kind == "Function" && node.name == "bark" }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "use strict;" }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "sample.pl::new" && edge.target == "bless"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.pl::bark"
                && edge.target == "sample.pl::speak"
        }));

        let bridge_source = br#"sub run_command {
    system("git status");
}

sub read_config {
    open(my $fh, '<', "config.yaml") or die;
    return $fh;
}

sub run_dynamic {
    my ($cmd) = @_;
    system($cmd);
}
"#;
        let (_nodes, bridge_edges) = parse_perl("bridge.pl", bridge_source);
        assert!(bridge_edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "git status"
                && edge.extra["evidence_source"] == "system"
                && edge.extra["confidence_tier"] == "HIGH"
        }));
        assert!(bridge_edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "<dynamic:open@bridge.pl:6>"
                && edge.extra["evidence_source"] == "open"
                && edge.extra["confidence_tier"] == "LOW"
        }));
    }

    #[test]
    fn parses_vue_script_blocks_with_typescript_offsets() {
        let source = br#"<template>
  <div class="app">
    <UserList :users="users" @select="onSelectUser" />
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import UserList from './UserList.vue'

interface User {
  id: number
  name: string
}

const count = ref(0)

function increment() {
  count.value++
}

function onSelectUser(user: User) {
  console.log(user.name)
}

const doubled = computed(() => count.value * 2)
</script>
"#;
        let (nodes, edges) = parse_vue("sample.vue", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "File" && node.name == "sample.vue" && node.language == "vue"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "User"
                && node.language == "vue"
                && node.extra["type_role"] == "interface"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "increment"
                && node.language == "vue"
                && node.line_start == 18
        }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "vue" && edge.line == 8 }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "sample.vue" && edge.target == "ref"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.vue::onSelectUser"
                && edge.target == "log"
                && edge.line == 23
        }));
    }

    #[test]
    fn parses_svelte_script_blocks_with_typescript_offsets() {
        let source = br#"<script lang="ts">
import { writable } from 'svelte/store'

interface User {
  name: string
}

const count = writable(0)

function increment() {
  console.log('increment')
}

function selectUser(user: User) {
  return user.name
}
</script>

<button on:click={increment}>{$count}</button>
"#;
        let (nodes, edges) = parse_svelte("sample.svelte", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "File" && node.name == "sample.svelte" && node.language == "svelte"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "User"
                && node.language == "svelte"
                && node.extra["type_role"] == "interface"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "increment"
                && node.language == "svelte"
                && node.line_start == 10
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.target == "svelte/store" && edge.line == 2
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "sample.svelte" && edge.target == "writable"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.svelte::increment"
                && edge.target == "log"
                && edge.line == 11
        }));
    }

    #[test]
    fn parses_zig_file_without_extra_nodes_for_python_parity() {
        let source = br#"const std = @import("std");

pub fn main() void {
    std.debug.print("hello\n", .{});
}
"#;
        let (nodes, edges) = parse_zig("src/main.zig", source);

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, "File");
        assert_eq!(nodes[0].name, "src/main.zig");
        assert_eq!(nodes[0].language, "zig");
        assert_eq!(nodes[0].line_start, 1);
        assert_eq!(nodes[0].line_end, 6);
        assert!(edges.is_empty());
    }

    #[test]
    fn parses_powershell_file_without_extra_nodes_for_python_parity() {
        let source = br#"function Invoke-Hello {
    param($Name)
    Write-Host "Hello $Name"
}

Invoke-Hello -Name World
"#;
        let (nodes, edges) = parse_powershell("scripts/hello.ps1", source);

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, "File");
        assert_eq!(nodes[0].name, "scripts/hello.ps1");
        assert_eq!(nodes[0].language, "powershell");
        assert_eq!(nodes[0].line_start, 1);
        assert_eq!(nodes[0].line_end, 7);
        assert!(edges.is_empty());
    }

    #[test]
    fn parses_rescript_modules_functions_and_calls() {
        let source = br#"open Belt

module User = {
  type t = {name: string}
  let make = name => {name}
  let greet = user => Js.log(user.name)
}

let main = () => {
  let user = User.make("Ada")
  User.greet(user)
}
"#;
        let (nodes, edges) = parse_rescript("src/App.res", source);

        assert!(nodes
            .iter()
            .any(|node| node.kind == "Class" && node.name == "User"));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "greet"
                && node.parent_name.as_deref() == Some("User")
        }));
        assert!(nodes
            .iter()
            .any(|node| node.kind == "Type" && node.name == "t"));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "Belt"));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "User.make"));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "CONTAINS" && edge.target == "src/App.res::User.greet" }));
    }

    #[test]
    fn parses_perl_xs_as_c_for_python_parity() {
        let source = br#"#include "EXTERN.h"
#include "perl.h"
#include "XSUB.h"
#include <string.h>

typedef struct {
    int x;
    int y;
} Point;

static int
_add(int a, int b) {
    return a + b;
}

static double
compute_distance(int x1, int y1, int x2, int y2) {
    return _add(x1, x2);
}

MODULE = MyModule  PACKAGE = MyModule

int
add(a, b)
    int a
    int b
  CODE:
    RETVAL = _add(a, b);
  OUTPUT:
    RETVAL
"#;
        let (nodes, edges) = parse_rust_owned_file("MyModule.xs", source);

        assert!(nodes
            .iter()
            .any(|node| node.kind == "Class" && node.name == "Point"));
        assert!(nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "_add"));
        assert!(nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "compute_distance"));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "XSUB.h"));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target.ends_with("::_add")));
    }

    #[test]
    fn parses_c_header_as_c_for_python_parity() {
        let source = br#"#ifndef USER_H
#define USER_H
#include <stdint.h>

typedef struct {
    int id;
} User;

static inline int user_id(User *user) {
    return user->id;
}

#endif
"#;
        let (nodes, edges) = parse_rust_owned_file("include/user.h", source);

        assert!(nodes
            .iter()
            .any(|node| node.kind == "File" && node.language == "c"));
        assert!(nodes
            .iter()
            .any(|node| node.kind == "Class" && node.name == "User"));
        assert!(nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "user_id"));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "stdint.h"));
    }

    #[test]
    fn parses_swift_types_functions_calls_and_bridges() {
        let source = br#"import Foundation

struct User {
    let name: String
}

class Repo {
    func save(_ user: User) {
        print(user.name)
    }
}

func runProcess() {
    let p = Process.run(URL(fileURLWithPath: "/usr/bin/git"), arguments: ["status"])
    _ = p
}

func loadLib() {
    dlopen("mylib.dylib", RTLD_NOW)
}
"#;
        let (nodes, edges) = parse_swift("App.swift", source);

        assert!(nodes
            .iter()
            .any(|node| node.kind == "Class" && node.name == "User"));
        assert!(nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "save"));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "import Foundation"));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "print"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.extra["evidence_source"] == "Process.run"
                && edge.extra["confidence_tier"] == "LOW"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "mylib.dylib"
                && edge.extra["evidence_source"] == "dlopen"
        }));
    }

    #[test]
    fn parses_c_structs_functions_imports_calls_and_bridges() {
        let source = br#"#include <stdio.h>
#include <dlfcn.h>

typedef struct {
    int id;
} User;

User* create_user(void) {
    return malloc(sizeof(User));
}

void print_user(User* user) {
    printf("%d", user->id);
}

void run_command(const char *cmd) {
    system("git status");
    fopen("config.yaml", "r");
    dlopen("mylib.so", RTLD_NOW);
    system(cmd);
}

int main() {
    User* u = create_user();
    print_user(u);
    return 0;
}
"#;
        let (nodes, edges) = parse_c("sample.c", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "User"
                && node.language == "c"
                && node.extra["type_role"] == "class"
        }));
        assert!(nodes
            .iter()
            .any(|node| { node.kind == "Function" && node.name == "create_user" }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "stdio.h" }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.c::main"
                && edge.target == "sample.c::create_user"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.source == "sample.c::run_command"
                && edge.target == "git status"
                && edge.extra["evidence_source"] == "system"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "config.yaml"
                && edge.extra["relationship_role"] == "opens_file"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "mylib.so"
                && edge.extra["bridge_kind"] == "ffi"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.target == "<dynamic:system@sample.c:20>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
    }

    #[test]
    fn parses_cpp_classes_inheritance_functions_imports_calls_and_bridges() {
        let source = br#"#include <iostream>
#include <cstdlib>

class Animal {
public:
    Animal() {}
};

class Dog : public Animal {
public:
    Dog() : Animal() {}
    void speak() {}
};

void greet(const Animal& animal) {}

int main() {
    Dog d;
    d.speak();
    greet(d);
    return 0;
}

void run_command() {
    std::system("git status");
}
"#;
        let (nodes, edges) = parse_cpp("sample.cpp", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == "Animal"
                && node.language == "cpp"
                && node.extra["type_role"] == "class"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "Dog"
                && node.parent_name.as_deref() == Some("Dog")
        }));
        assert!(edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "iostream" }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS" && edge.source == "sample.cpp::Dog" && edge.target == "Animal"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "sample.cpp::main" && edge.target == "speak"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.cpp::main"
                && edge.target == "sample.cpp::greet"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.source == "sample.cpp::run_command"
                && edge.target == "git status"
                && edge.extra["evidence_source"] == "std::system"
                && edge.extra["source_language"] == "cpp"
        }));
    }

    #[test]
    fn parses_objc_classes_methods_imports_messages_and_c_functions() {
        let source = br#"#import <Foundation/Foundation.h>
#import "Logger.h"

@interface Calculator : NSObject
- (NSInteger)add:(NSInteger)a to:(NSInteger)b;
@end

@implementation Calculator

- (NSInteger)add:(NSInteger)a to:(NSInteger)b {
    NSInteger sum = a + b;
    [self logResult:sum];
    return sum;
}

- (void)logResult:(NSInteger)value {
    NSLog(@"Result: %ld", (long)value);
}

+ (Calculator *)sharedCalculator {
    return [[Calculator alloc] init];
}

@end

int main(int argc, const char * argv[]) {
    Calculator *calc = [Calculator sharedCalculator];
    NSInteger r = [calc add:3 to:4];
    NSLog(@"Final: %ld", (long)r);
    return 0;
}
"#;
        let (nodes, edges) = parse_objc("sample.m", source);
        assert!(nodes.iter().any(|node| {
            node.kind == "Class" && node.name == "Calculator" && node.language == "objc"
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == "add"
                && node.parent_name.as_deref() == Some("Calculator")
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function" && node.name == "main" && node.parent_name.is_none()
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.target == "#import <Foundation/Foundation.h>"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.m::Calculator.add"
                && edge.target == "sample.m::Calculator.logResult"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.m::main"
                && edge.target == "sample.m::Calculator.sharedCalculator"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.m::main"
                && edge.target == "sample.m::Calculator.add"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "sample.m::main" && edge.target == "NSLog"
        }));
    }

    #[test]
    fn parses_rust_items_imports_and_calls() {
        let source = br#"pub use dagayn_graph::{GraphStore};
use std::fs;

struct Foo {
    value: i32,
}

impl Foo {
    pub fn new() -> Self {
        Self { value: 1 }
    }

    fn load(&self) {
        fs::read("path");
        consume(helper);
        helper();
    }
}

fn consume(_f: fn()) {}
fn helper() {}
"#;
        let (nodes, edges) = parse_rust("src/lib.rs", source);
        let node_names = nodes
            .iter()
            .map(|node| {
                (
                    node.kind.as_str(),
                    node.name.as_str(),
                    node.parent_name.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert!(node_names.contains(&("File", "src/lib.rs", None)));
        assert!(node_names.contains(&("Class", "Foo", None)));
        assert!(node_names.contains(&("Function", "new", Some("Foo"))));
        assert!(node_names.contains(&("Function", "load", Some("Foo"))));
        assert!(node_names.contains(&("Function", "helper", None)));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.source == "src/lib.rs" && edge.target == "std::fs"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.source == "src/lib.rs"
                && edge.target == "pub dagayn_graph::{GraphStore}"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/lib.rs::Foo.load"
                && edge.target == "src/lib.rs::helper"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "src/lib.rs::Foo.load"
                && edge.target == "src/lib.rs::helper"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.source == "src/lib.rs::Foo.load"
                && edge.target == "path"
        }));
    }

    #[test]
    fn parses_python_items_imports_and_calls() {
        let source = br#"from models import User
import os

class Service(Base):
    def run(self, name: str) -> User:
        helper(name)
        os.getenv("ENV")

def helper(value: str) -> None:
    print(value)
"#;
        let (nodes, edges) = parse_python("app.py", source);
        let node_names = nodes
            .iter()
            .map(|node| {
                (
                    node.kind.as_str(),
                    node.name.as_str(),
                    node.parent_name.as_deref(),
                    node.params.as_deref(),
                    node.return_type.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert!(node_names.contains(&("File", "app.py", None, None, None)));
        assert!(node_names.contains(&("Class", "Service", None, None, None)));
        assert!(node_names.contains(&(
            "Function",
            "run",
            Some("Service"),
            Some("(self, name: str)"),
            Some("User")
        )));
        assert!(node_names.contains(&(
            "Function",
            "helper",
            None,
            Some("(value: str)"),
            Some("None")
        )));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.source == "app.py" && edge.target == "models"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.source == "app.py" && edge.target == "os"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS" && edge.source == "app.py::Service" && edge.target == "Base"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "app.py::Service.run"
                && edge.target == "app.py::helper"
        }));
    }

    #[test]
    fn parses_typescript_items_calls_tests_and_references() {
        let source = br#"import { Thing } from './thing';

interface Shape {
  id: string;
}

class Service extends Base {
  run(input: string): void {
    helper(input);
  }
}

function helper(value: string): void {
  console.log(value);
}

const indirect = { helper };
const callbacks = [helper];

describe('Service', () => {
  it('runs', () => {
    helper('x');
  });
});
"#;
        let (nodes, edges) = parse_javascript_like("service.test.ts", source, "typescript");
        let node_names = nodes
            .iter()
            .map(|node| {
                (
                    node.kind.as_str(),
                    node.name.as_str(),
                    node.parent_name.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert!(node_names.contains(&("Class", "Shape", None)));
        assert!(node_names.contains(&("Class", "Service", None)));
        assert!(node_names
            .iter()
            .any(|(_, name, parent)| *name == "run" && *parent == Some("Service")));
        assert!(node_names
            .iter()
            .any(|(_, name, parent)| *name == "helper" && parent.is_none()));
        assert!(node_names
            .iter()
            .any(|(kind, name, _)| *kind == "Test" && name.starts_with("it:runs@L")));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "./thing"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS"
                && edge.source == "service.test.ts::Service"
                && edge.target == "Base"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "service.test.ts::Service.run"
                && edge.target == "service.test.ts::helper"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "service.test.ts"
                && edge.target == "service.test.ts::helper"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "TESTED_BY"
                && edge.source == "service.test.ts::helper"
                && edge.target.contains("it:runs")
        }));
    }

    #[test]
    fn resolves_typescript_imported_call_targets() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-ts-import-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(repo_root.join("src")).unwrap();
        std::fs::write(
            repo_root.join("src/helper.ts"),
            b"export function helper() { return 1; }\n",
        )
        .unwrap();

        let source = br#"import { helper } from './helper';

export function run() {
  helper();
  const refs = [helper];
}
"#;
        let mut parser = RustOwnedParser::new();
        let (_nodes, edges) =
            parser.parse_file_in_repo(Some(&repo_root), "src/consumer.ts", source);
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/consumer.ts::run"
                && edge.target == "src/helper.ts::helper"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == "src/consumer.ts::run"
                && edge.target == "src/helper.ts::helper"
        }));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn resolves_typescript_tsconfig_alias_imports() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-ts-alias-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(repo_root.join("src/lib")).unwrap();
        std::fs::write(
            repo_root.join("tsconfig.json"),
            br#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@/*": ["src/*"],
    },
  },
}
"#,
        )
        .unwrap();
        std::fs::write(
            repo_root.join("src/lib/utils.ts"),
            b"export function cn(...args: string[]): string { return args.join(' '); }\n",
        )
        .unwrap();

        let source = br#"import { cn } from '@/lib/utils';

export function formatUser(name: string): string {
  return cn('user', name);
}
"#;
        let mut parser = RustOwnedParser::new();
        let (_nodes, edges) =
            parser.parse_file_in_repo(Some(&repo_root), "alias_importer.ts", source);
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.source == "alias_importer.ts"
                && edge.target == "src/lib/utils.ts"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "alias_importer.ts::formatUser"
                && edge.target == "src/lib/utils.ts::cn"
        }));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn resolves_typescript_barrel_reexports_to_origin() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-ts-barrel-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(repo_root.join("src/components")).unwrap();
        std::fs::write(
            repo_root.join("src/components/MarkdownMsg.ts"),
            b"export function MarkdownMsg() { return 'ok'; }\n",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("src/components/index.ts"),
            b"export { MarkdownMsg as Msg } from './MarkdownMsg';\n",
        )
        .unwrap();

        let source = br#"import { Msg } from './components';

export function render() {
  return Msg();
}
"#;
        let mut parser = RustOwnedParser::new();
        let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", source);
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/app.ts::render"
                && edge.target == "src/components/MarkdownMsg.ts::MarkdownMsg"
        }));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn resolves_typescript_star_barrel_reexports_to_origin() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-ts-star-barrel-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(repo_root.join("src/components")).unwrap();
        std::fs::write(
            repo_root.join("src/components/MarkdownMsg.ts"),
            b"export function MarkdownMsg() { return 'ok'; }\n",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("src/components/index.ts"),
            b"export * from './MarkdownMsg';\n",
        )
        .unwrap();

        let source = br#"import { MarkdownMsg } from './components';

export function render() {
  return MarkdownMsg();
}
"#;
        let mut parser = RustOwnedParser::new();
        let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", source);
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/app.ts::render"
                && edge.target == "src/components/MarkdownMsg.ts::MarkdownMsg"
        }));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn parses_tsx_jsx_component_calls() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-tsx-jsx-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::write(
            repo_root.join("MarkdownMsg.tsx"),
            b"export function MarkdownMsg() { return <div />; }\n",
        )
        .unwrap();

        let source = br#"import MarkdownMsg from './MarkdownMsg';

export function BookWorkspace() {
  return <section><MarkdownMsg text={value} /></section>;
}
"#;
        let mut parser = RustOwnedParser::new();
        let (_nodes, edges) =
            parser.parse_file_in_repo(Some(&repo_root), "BookWorkspace.tsx", source);
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "BookWorkspace.tsx::BookWorkspace"
                && edge.target == "MarkdownMsg.tsx::MarkdownMsg"
        }));
        assert!(!edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && (edge.target == "section" || edge.target == "div" || edge.target == "span")
        }));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn parses_tsx_namespace_component_calls() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-tsx-namespace-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::write(
            repo_root.join("MarkdownMsg.tsx"),
            b"export function MarkdownMsg() { return <div />; }\n",
        )
        .unwrap();

        let source = br#"import * as UI from './MarkdownMsg';

export function BookWorkspace() {
  return <UI.Messages.MarkdownMsg text={value} />;
}
"#;
        let mut parser = RustOwnedParser::new();
        let (_nodes, edges) =
            parser.parse_file_in_repo(Some(&repo_root), "BookWorkspace.tsx", source);
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "BookWorkspace.tsx::BookWorkspace"
                && edge.target == "MarkdownMsg.tsx::MarkdownMsg"
        }));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn parses_jsx_component_calls() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-jsx-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::write(
            repo_root.join("MarkdownMsg.jsx"),
            b"export function MarkdownMsg() { return <div />; }\n",
        )
        .unwrap();

        let source = br#"import { MarkdownMsg } from './MarkdownMsg';

export function BookWorkspace() {
  return <MarkdownMsg text={value} />;
}
"#;
        let mut parser = RustOwnedParser::new();
        let (_nodes, edges) =
            parser.parse_file_in_repo(Some(&repo_root), "BookWorkspace.jsx", source);
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "BookWorkspace.jsx::BookWorkspace"
                && edge.target == "MarkdownMsg.jsx::MarkdownMsg"
        }));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn parses_javascript_cross_artifact_edges() {
        let source = br#"child_process.spawn("./bin/tool", ["--flag"]);

function runDynamic(cmd) {
  child_process.exec(cmd);
}
"#;
        let (nodes, edges) = parse_javascript_like("bridge.js", source, "javascript");
        assert!(nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "runDynamic"));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.source == "bridge.js"
                && edge.target == "./bin/tool"
                && edge.extra["evidence_source"] == "child_process.spawn"
                && edge.extra["confidence_tier"] == "HIGH"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CROSS_ARTIFACT"
                && edge.source == "bridge.js::runDynamic"
                && edge.target == "<dynamic:child_process.exec@bridge.js:4>"
                && edge.extra["confidence_tier"] == "LOW"
        }));
    }

    #[test]
    fn parses_rust_owned_files_as_one_compact_batch() {
        let mut repo_root = std::env::temp_dir();
        repo_root.push(format!(
            "dagayn-parser-batch-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(repo_root.join("docs")).unwrap();
        std::fs::write(
            repo_root.join("docs/README.md"),
            b"# Guide\n\nSee `build_graph`.\n",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("main.tf"),
            br#"variable "region" {
  default = "us-east-1"
}
"#,
        )
        .unwrap();

        let payload = parse_rust_owned_files_compact_json(
            &repo_root,
            &["docs/README.md".to_string(), "main.tf".to_string()],
        );
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["errors"].as_array().unwrap().len(), 0);
        let batch = parsed["batch"].as_array().unwrap();
        assert_eq!(batch.len(), 2);
        assert!(batch.iter().any(|item| item[0] == "docs/README.md"));
        assert!(batch.iter().any(|item| item[0] == "main.tf"));

        let _ = std::fs::remove_dir_all(&repo_root);
    }
}
