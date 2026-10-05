//! Tree-sitter parser constructors, one per grammar.
//!
//! Each grammar is behind a `lang-<language>` Cargo feature (all on by
//! default). With a feature off, `new_<language>_parser` still exists but
//! returns `None`, so the extractors compile unchanged. The graph path never
//! reaches them for that language: `parse_file_dispatch` checks
//! `RustOwnedPathKind::grammar_enabled` first and gives the file only its
//! File node (`file_only.rs`, as for PowerShell). Path ownership does not
//! change, so a disabled language is still Rust-owned and no other parser
//! takes the file over. A graph built that way keeps File nodes only for the
//! language until a build with the feature on re-parses it
//! (`dagayn build --force-full-build`, or a changed file). The per-language
//! `parse_<language>` helpers bypass that check and return what the extractor
//! recovers without a grammar (a File node; Markdown and Terraform also scan
//! text).

macro_rules! grammar_parser {
    ($constructor:ident, $feature:literal, $language:ident) => {
        #[cfg(feature = $feature)]
        pub(super) fn $constructor() -> Option<tree_sitter::Parser> {
            let mut parser = tree_sitter::Parser::new();
            if parser.set_language(&dagayn_grammars::$language()).is_ok() {
                Some(parser)
            } else {
                None
            }
        }

        #[cfg(not(feature = $feature))]
        pub(super) fn $constructor() -> Option<tree_sitter::Parser> {
            None
        }
    };
}

grammar_parser!(new_terraform_parser, "lang-terraform", terraform_language);
grammar_parser!(new_markdown_parser, "lang-markdown", markdown_language);
grammar_parser!(new_rust_parser, "lang-rust", rust_language);
grammar_parser!(new_python_parser, "lang-python", python_language);
grammar_parser!(
    new_javascript_parser,
    "lang-javascript",
    javascript_language
);
grammar_parser!(
    new_typescript_parser,
    "lang-typescript",
    typescript_language
);
grammar_parser!(new_tsx_parser, "lang-tsx", tsx_language);
grammar_parser!(new_bash_parser, "lang-bash", bash_language);
grammar_parser!(new_go_parser, "lang-go", go_language);
grammar_parser!(new_java_parser, "lang-java", java_language);
grammar_parser!(new_ruby_parser, "lang-ruby", ruby_language);
grammar_parser!(new_csharp_parser, "lang-csharp", csharp_language);
grammar_parser!(new_php_parser, "lang-php", php_language);
grammar_parser!(new_kotlin_parser, "lang-kotlin", kotlin_language);
grammar_parser!(new_scala_parser, "lang-scala", scala_language);
grammar_parser!(new_dart_parser, "lang-dart", dart_language);
grammar_parser!(new_lua_parser, "lang-lua", lua_language);
grammar_parser!(new_c_parser, "lang-c", c_language);
grammar_parser!(new_cpp_parser, "lang-cpp", cpp_language);
grammar_parser!(new_objc_parser, "lang-objc", objc_language);
grammar_parser!(new_elixir_parser, "lang-elixir", elixir_language);
grammar_parser!(new_gdscript_parser, "lang-gdscript", gdscript_language);
grammar_parser!(new_r_parser, "lang-r", r_language);
grammar_parser!(new_julia_parser, "lang-julia", julia_language);
grammar_parser!(new_perl_parser, "lang-perl", perl_language);
grammar_parser!(new_vue_parser, "lang-vue", vue_language);
grammar_parser!(new_svelte_parser, "lang-svelte", svelte_language);
grammar_parser!(new_zig_parser, "lang-zig", zig_language);
grammar_parser!(new_swift_parser, "lang-swift", swift_language);
