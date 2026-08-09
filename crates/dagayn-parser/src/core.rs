//! Rust parser crate.
//!
//! The migration target is for this crate to own file discovery, language
//! detection, parser orchestration, Markdown, Terraform, and notebook
//! extraction. During Phase 1 it starts with parseable-file filtering so Python
//! can shrink back toward CLI/MCP interfaces.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

#[path = "bash.rs"]
mod bash;
#[path = "c_like.rs"]
mod c_like;
#[path = "csharp.rs"]
mod csharp;
#[path = "dart.rs"]
mod dart;
#[path = "discovery.rs"]
mod discovery;
#[path = "documentation_directives.rs"]
mod documentation_directives;
#[path = "elixir.rs"]
mod elixir;
#[path = "file_only.rs"]
mod file_only;
#[path = "gdscript.rs"]
mod gdscript;
#[path = "go.rs"]
mod go;
#[path = "java.rs"]
mod java;
#[path = "js_like.rs"]
mod js_like;
#[path = "js_modules.rs"]
mod js_modules;
#[path = "js_sfc.rs"]
mod js_sfc;
#[path = "julia.rs"]
mod julia;
#[path = "kotlin.rs"]
mod kotlin;
#[path = "lua.rs"]
mod lua;
#[path = "markdown.rs"]
mod markdown;
#[path = "ownership.rs"]
mod ownership;
#[path = "parsers.rs"]
mod parsers;
#[path = "perl.rs"]
mod perl;
#[path = "php.rs"]
mod php;
#[path = "python.rs"]
mod python;
#[path = "r.rs"]
mod r;
#[path = "ruby.rs"]
mod ruby;
#[path = "rust_lang.rs"]
mod rust_lang;
#[path = "scala.rs"]
mod scala;
#[path = "solidity.rs"]
mod solidity;
#[path = "swift.rs"]
mod swift;
#[path = "terraform.rs"]
mod terraform;
#[path = "terraform_bridges.rs"]
mod terraform_bridges;
#[path = "terraform_collect.rs"]
mod terraform_collect;
#[path = "types.rs"]
mod types;
#[path = "util.rs"]
mod util;

pub use discovery::{
    collect_parseable_files, detect_language, filter_incremental_candidates, filter_parseable_files,
};
pub use js_sfc::{parse_svelte, parse_vue};
pub use types::{ParsedEdge, ParsedNode};

use ownership::{rust_owned_path_kind, rust_owned_path_kind_for_source, RustOwnedPathKind};
use parsers::*;
use util::{contains_ascii_ignore_case, node_text, sha256_hex, starts_with_ascii_ignore_case};

#[cfg(test)]
pub(crate) use discovery::{build_globset, load_ignore_patterns, should_ignore, walk_files};
#[cfg(test)]
use js_like::parse_javascript_like;
pub fn grammar_status() -> dagayn_grammars::GrammarStatus {
    dagayn_grammars::status()
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
    javascript_export_cache: js_modules::JavaScriptExportCache,
    javascript_module_cache: js_modules::JavaScriptModuleCache,
    javascript_tsconfig_cache: js_modules::JavaScriptTsconfigCache,
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
            javascript_export_cache: Default::default(),
            javascript_module_cache: Default::default(),
            javascript_tsconfig_cache: Default::default(),
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
            RustOwnedPathKind::Markdown => markdown::parse_markdown_with_parser(
                file_path,
                source,
                parser_slot(&mut self.markdown_parser, new_markdown_parser),
            ),
            RustOwnedPathKind::Terraform => terraform::parse_terraform_with_parser(
                file_path,
                source,
                parser_slot(&mut self.terraform_parser, new_terraform_parser),
            ),
            RustOwnedPathKind::Rust => rust_lang::parse_rust_with_parser(
                file_path,
                source,
                parser_slot(&mut self.rust_parser, new_rust_parser),
            ),
            RustOwnedPathKind::Python => python::parse_python_with_parser(
                file_path,
                source,
                parser_slot(&mut self.python_parser, new_python_parser),
                repo_root,
            ),
            RustOwnedPathKind::Notebook => python::parse_notebook_with_parser(
                file_path,
                source,
                parser_slot(&mut self.python_parser, new_python_parser),
                repo_root,
            ),
            RustOwnedPathKind::JavaScript => js_like::parse_javascript_like_with_parser(
                file_path,
                source,
                "javascript",
                parser_slot(&mut self.javascript_parser, new_javascript_parser),
                repo_root,
                js_modules::JavaScriptCaches {
                    export: Some(&self.javascript_export_cache),
                    module: Some(&self.javascript_module_cache),
                    tsconfig: Some(&self.javascript_tsconfig_cache),
                },
            ),
            RustOwnedPathKind::TypeScript => js_like::parse_javascript_like_with_parser(
                file_path,
                source,
                "typescript",
                parser_slot(&mut self.typescript_parser, new_typescript_parser),
                repo_root,
                js_modules::JavaScriptCaches {
                    export: Some(&self.javascript_export_cache),
                    module: Some(&self.javascript_module_cache),
                    tsconfig: Some(&self.javascript_tsconfig_cache),
                },
            ),
            RustOwnedPathKind::Tsx => js_like::parse_javascript_like_with_parser(
                file_path,
                source,
                "tsx",
                parser_slot(&mut self.tsx_parser, new_tsx_parser),
                repo_root,
                js_modules::JavaScriptCaches {
                    export: Some(&self.javascript_export_cache),
                    module: Some(&self.javascript_module_cache),
                    tsconfig: Some(&self.javascript_tsconfig_cache),
                },
            ),
            RustOwnedPathKind::Bash => bash::parse_bash_with_parser(
                file_path,
                source,
                parser_slot(&mut self.bash_parser, new_bash_parser),
                repo_root,
            ),
            RustOwnedPathKind::Go => go::parse_go_with_parser(
                file_path,
                source,
                parser_slot(&mut self.go_parser, new_go_parser),
            ),
            RustOwnedPathKind::Java => java::parse_java_with_parser(
                file_path,
                source,
                parser_slot(&mut self.java_parser, new_java_parser),
                repo_root,
            ),
            RustOwnedPathKind::Ruby => ruby::parse_ruby_with_parser(
                file_path,
                source,
                parser_slot(&mut self.ruby_parser, new_ruby_parser),
            ),
            RustOwnedPathKind::CSharp => csharp::parse_csharp_with_parser(
                file_path,
                source,
                parser_slot(&mut self.csharp_parser, new_csharp_parser),
            ),
            RustOwnedPathKind::Php => php::parse_php_with_parser(
                file_path,
                source,
                parser_slot(&mut self.php_parser, new_php_parser),
            ),
            RustOwnedPathKind::Kotlin => kotlin::parse_kotlin_with_parser(
                file_path,
                source,
                parser_slot(&mut self.kotlin_parser, new_kotlin_parser),
            ),
            RustOwnedPathKind::Scala => scala::parse_scala_with_parser(
                file_path,
                source,
                parser_slot(&mut self.scala_parser, new_scala_parser),
            ),
            RustOwnedPathKind::Solidity => solidity::parse_solidity_with_parser(
                file_path,
                source,
                parser_slot(&mut self.solidity_parser, new_solidity_parser),
            ),
            RustOwnedPathKind::Dart => dart::parse_dart_with_parser(
                file_path,
                source,
                parser_slot(&mut self.dart_parser, new_dart_parser),
            ),
            RustOwnedPathKind::Lua => lua::parse_lua_with_parser(
                file_path,
                source,
                parser_slot(&mut self.lua_parser, new_lua_parser),
            ),
            RustOwnedPathKind::Luau => lua::parse_luau_with_parser(
                file_path,
                source,
                parser_slot(&mut self.luau_parser, new_luau_parser),
            ),
            RustOwnedPathKind::C => c_like::parse_c_with_parser(
                file_path,
                source,
                parser_slot(&mut self.c_parser, new_c_parser),
            ),
            RustOwnedPathKind::Cpp => c_like::parse_cpp_with_parser(
                file_path,
                source,
                parser_slot(&mut self.cpp_parser, new_cpp_parser),
            ),
            RustOwnedPathKind::ObjC => c_like::parse_objc_with_parser(
                file_path,
                source,
                parser_slot(&mut self.objc_parser, new_objc_parser),
            ),
            RustOwnedPathKind::Elixir => elixir::parse_elixir_with_parser(
                file_path,
                source,
                parser_slot(&mut self.elixir_parser, new_elixir_parser),
            ),
            RustOwnedPathKind::Gdscript => gdscript::parse_gdscript_with_parser(
                file_path,
                source,
                parser_slot(&mut self.gdscript_parser, new_gdscript_parser),
            ),
            RustOwnedPathKind::R => r::parse_r_with_parser(
                file_path,
                source,
                parser_slot(&mut self.r_parser, new_r_parser),
            ),
            RustOwnedPathKind::Julia => julia::parse_julia_with_parser(
                file_path,
                source,
                parser_slot(&mut self.julia_parser, new_julia_parser),
            ),
            RustOwnedPathKind::Perl => perl::parse_perl_with_parser(
                file_path,
                source,
                parser_slot(&mut self.perl_parser, new_perl_parser),
            ),
            RustOwnedPathKind::Vue => {
                ensure_parser(&mut self.vue_parser, new_vue_parser);
                ensure_parser(&mut self.javascript_parser, new_javascript_parser);
                ensure_parser(&mut self.typescript_parser, new_typescript_parser);
                js_sfc::parse_vue_with_parsers(
                    file_path,
                    source,
                    self.vue_parser.as_mut(),
                    self.javascript_parser.as_mut(),
                    self.typescript_parser.as_mut(),
                    repo_root,
                    js_modules::JavaScriptCaches {
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
                js_sfc::parse_svelte_with_parsers(
                    file_path,
                    source,
                    self.svelte_parser.as_mut(),
                    self.javascript_parser.as_mut(),
                    self.typescript_parser.as_mut(),
                    repo_root,
                    js_modules::JavaScriptCaches {
                        export: Some(&self.javascript_export_cache),
                        module: Some(&self.javascript_module_cache),
                        tsconfig: Some(&self.javascript_tsconfig_cache),
                    },
                )
            }
            RustOwnedPathKind::Zig => {
                ensure_parser(&mut self.zig_parser, new_zig_parser);
                file_only::parse_zig_with_parser(file_path, source, self.zig_parser.as_mut())
            }
            RustOwnedPathKind::PowerShell => {
                ensure_parser(&mut self.powershell_parser, new_powershell_parser);
                file_only::parse_powershell_with_parser(
                    file_path,
                    source,
                    self.powershell_parser.as_mut(),
                )
            }
            RustOwnedPathKind::Swift => {
                ensure_parser(&mut self.swift_parser, new_swift_parser);
                swift::parse_swift_with_parser(file_path, source, self.swift_parser.as_mut())
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
    markdown::parse_markdown_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_markdown_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_markdown(file_path, source);
    parsed_compact_json(nodes, edges)
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum RustOwnedParseFileResult {
    Ok {
        file_path: String,
        nodes: Vec<Value>,
        edges: Vec<Value>,
        file_hash: String,
    },
    Error {
        file_path: String,
        reason_code: &'static str,
        error: String,
    },
    Unsupported {
        file_path: String,
        reason_code: &'static str,
        error: String,
    },
}

#[derive(Debug, Serialize)]
struct RustOwnedParseFilesCompactPayload {
    batch: Vec<Value>,
    errors: Vec<(String, String)>,
    results: Vec<RustOwnedParseFileResult>,
}

pub fn parse_rust_owned_files_compact_json(repo_root: &Path, file_paths: &[String]) -> String {
    let mut batch = Vec::new();
    let mut errors = Vec::new();
    let mut results = Vec::new();
    let mut parser = RustOwnedParser::new();
    for file_path in file_paths {
        let full_path = repo_root.join(file_path);
        let source = match std::fs::read(&full_path) {
            Ok(source) => source,
            Err(err) => {
                let error = err.to_string();
                errors.push((file_path.clone(), error.clone()));
                results.push(RustOwnedParseFileResult::Error {
                    file_path: file_path.clone(),
                    reason_code: "read_error",
                    error,
                });
                continue;
            }
        };
        if !rust_parser_owns_source(file_path, &source) {
            let error = "unsupported Rust parser path".to_string();
            errors.push((file_path.clone(), error.clone()));
            results.push(RustOwnedParseFileResult::Unsupported {
                file_path: file_path.clone(),
                reason_code: "unsupported_rust_parser_path",
                error,
            });
            continue;
        }
        let (nodes, edges) = parser.parse_file_in_repo(Some(repo_root), file_path, &source);
        let (nodes, edges) = parsed_compact_values(nodes, edges);
        let file_hash = sha256_hex(&source);
        batch.push(json!([file_path, &nodes, &edges, &file_hash]));
        results.push(RustOwnedParseFileResult::Ok {
            file_path: file_path.clone(),
            nodes,
            edges,
            file_hash,
        });
    }
    serde_json::to_string(&RustOwnedParseFilesCompactPayload {
        batch,
        errors,
        results,
    })
    .expect("serializing Rust-owned parser payload should not fail")
}

pub fn parse_rust_owned_file_compact_json(file_path: &str, source: &[u8]) -> String {
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file_in_repo(None, file_path, source);
    parsed_compact_json(nodes, edges)
}

pub fn parse_terraform(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_terraform_parser();
    terraform::parse_terraform_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_terraform_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_terraform(file_path, source);
    parsed_compact_json(nodes, edges)
}

pub fn parse_python(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_python_parser();
    python::parse_python_with_parser(file_path, source, parser.as_mut(), None)
}

pub fn parse_python_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_python(file_path, source);
    parsed_compact_json(nodes, edges)
}

pub fn parse_notebook(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_python_parser();
    python::parse_notebook_with_parser(file_path, source, parser.as_mut(), None)
}

pub fn parse_rust(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_rust_parser();
    rust_lang::parse_rust_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_rust_compact_json(file_path: &str, source: &[u8]) -> String {
    let (nodes, edges) = parse_rust(file_path, source);
    parsed_compact_json(nodes, edges)
}

pub fn parse_zig(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_zig_parser();
    file_only::parse_zig_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_powershell(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_powershell_parser();
    file_only::parse_powershell_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_swift(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_swift_parser();
    swift::parse_swift_with_parser(file_path, source, parser.as_mut())
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
            kind: crate::core::types::EdgeKind::TestedBy.as_str().to_string(),
            source: edge.target.clone(),
            target: edge.source.clone(),
            file_path: edge.file_path.clone(),
            line: edge.line,
            extra: json!({}),
        })
        .collect::<Vec<_>>();
    edges.extend(tested_by);
}

pub fn parse_bash(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_bash_parser();
    bash::parse_bash_with_parser(file_path, source, parser.as_mut(), None)
}

pub fn parse_go(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_go_parser();
    go::parse_go_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_java(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_java_parser();
    java::parse_java_with_parser(file_path, source, parser.as_mut(), None)
}

pub fn parse_ruby(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_ruby_parser();
    ruby::parse_ruby_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_csharp(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_csharp_parser();
    csharp::parse_csharp_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_php(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_php_parser();
    php::parse_php_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_kotlin(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_kotlin_parser();
    kotlin::parse_kotlin_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_scala(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_scala_parser();
    scala::parse_scala_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_solidity(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_solidity_parser();
    solidity::parse_solidity_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_dart(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_dart_parser();
    dart::parse_dart_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_lua(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_lua_parser();
    lua::parse_lua_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_luau(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_luau_parser();
    lua::parse_luau_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_elixir(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_elixir_parser();
    elixir::parse_elixir_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_gdscript(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_gdscript_parser();
    gdscript::parse_gdscript_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_r(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_r_parser();
    r::parse_r_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_julia(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_julia_parser();
    julia::parse_julia_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_perl(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_perl_parser();
    perl::parse_perl_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_c(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_c_parser();
    c_like::parse_c_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_cpp(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_cpp_parser();
    c_like::parse_cpp_with_parser(file_path, source, parser.as_mut())
}

pub fn parse_objc(file_path: &str, source: &[u8]) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let mut parser = new_objc_parser();
    c_like::parse_objc_with_parser(file_path, source, parser.as_mut())
}

pub(super) fn resolve_rust_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind.as_str(),
                "Function" | "Class" | "Type" | "Test" | "DocSection"
            )
        })
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

pub(super) fn qualify(file_path: &str, name: &str, parent_name: Option<&str>) -> String {
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

fn rust_node_with_leading_attributes(
    node: tree_sitter::Node<'_>,
) -> impl Iterator<Item = tree_sitter::Node<'_>> {
    let mut attrs = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(sibling) = current {
        if matches!(sibling.kind(), "attribute_item" | "inner_attribute_item") {
            attrs.push(sibling);
            current = sibling.prev_sibling();
            continue;
        }
        break;
    }
    attrs.reverse();
    attrs.into_iter().chain(std::iter::once(node))
}

fn has_rust_test_attribute(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    for candidate in rust_node_with_leading_attributes(node) {
        if matches!(candidate.kind(), "attribute_item" | "inner_attribute_item")
            && node_text(candidate, source).contains("test")
        {
            return true;
        }
        let mut cursor = candidate.walk();
        if candidate
            .children(&mut cursor)
            .filter(|child| matches!(child.kind(), "attribute_item" | "inner_attribute_item"))
            .any(|child| node_text(child, source).contains("test"))
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
        RustOwnedPathKind::JavaScript => {
            js_like::parse_javascript_like(file_path, source, "javascript")
        }
        RustOwnedPathKind::TypeScript => {
            js_like::parse_javascript_like(file_path, source, "typescript")
        }
        RustOwnedPathKind::Tsx => js_like::parse_javascript_like(file_path, source, "tsx"),
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

#[cfg(test)]
mod parser_core_tests {
    use super::{
        has_rust_test_attribute, new_rust_parser, rust_node_with_leading_attributes,
        RustOwnedParser,
    };
    use std::path::Path;

    #[test]
    fn test_parse_file_in_repo_dispatches_markdown_by_path() {
        let mut parser = RustOwnedParser::new();
        let (nodes, edges) = parser.parse_file_in_repo(
            Some(Path::new("/repo")),
            "docs/design.md",
            b"# Design\n\nBody\n",
        );

        assert!(nodes
            .iter()
            .any(|node| node.kind == "DocSection" && node.name == "design"));
        assert!(edges.iter().any(|edge| edge.kind == "CONTAINS"));
    }

    #[test]
    fn test_has_rust_test_attribute_reads_leading_attribute_sibling() {
        let source = b"#[test]\nfn test_example() {}\n";
        let mut parser = new_rust_parser().expect("rust grammar should load");
        let tree = parser.parse(source, None).expect("source should parse");
        let root = tree.root_node();
        let mut cursor = root.walk();
        let function = root
            .named_children(&mut cursor)
            .find(|node| node.kind() == "function_item")
            .expect("function item should exist");
        let candidates = rust_node_with_leading_attributes(function).collect::<Vec<_>>();

        assert_eq!(candidates[0].kind(), "attribute_item");
        assert_eq!(candidates[1].kind(), "function_item");
        assert!(has_rust_test_attribute(function, source));
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
