//! Compiled grammar provisioning for the Rust parser.
//!
//! This crate compiles the same pinned grammar sources that the Python parser
//! path provisions through `dagayn.vendor_grammars`.

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_markdown() -> *const ();
    fn tree_sitter_terraform() -> *const ();
    fn tree_sitter_rust() -> *const ();
}

pub const MARKDOWN_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_markdown) };
pub const TERRAFORM_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_terraform) };
pub const RUST_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_rust) };

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrammarStatus {
    Ready,
}

pub fn status() -> GrammarStatus {
    GrammarStatus::Ready
}

pub fn markdown_language() -> tree_sitter::Language {
    MARKDOWN_LANGUAGE.into()
}

pub fn terraform_language() -> tree_sitter::Language {
    TERRAFORM_LANGUAGE.into()
}

pub fn rust_language() -> tree_sitter::Language {
    RUST_LANGUAGE.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_markdown_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&markdown_language())
            .expect("load pinned Markdown grammar");
        let tree = parser.parse("# Heading\n", None).expect("parse Markdown");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_terraform_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&terraform_language())
            .expect("load pinned Terraform grammar");
        let tree = parser
            .parse("resource \"aws_s3_bucket\" \"main\" {}\n", None)
            .expect("parse Terraform");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_rust_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&rust_language())
            .expect("load pinned Rust grammar");
        let tree = parser.parse("fn main() {}\n", None).expect("parse Rust");
        assert!(!tree.root_node().has_error());
    }
}
