use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_terraform() -> *const ();
}

/// The tree-sitter [`LanguageFn`] for this grammar.
pub static LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_terraform) };

/// The source of the Terraform tree-sitter grammar description.
pub const GRAMMAR: &str = include_str!("../../grammar.js");

/// The folds query for this language.
pub const FOLDS_QUERY: &str = include_str!("../../queries/folds.scm");

/// The highlights query for this language.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../../queries/highlights.scm");

/// The tags query for this language.
pub const TAGS_QUERY: &str = include_str!("../../queries/tags.scm");

/// The node types for this language.
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");
