//! Rust parser crate.
//!
//! The migration target is for this crate to own file discovery, language
//! detection, parser orchestration, Markdown, Terraform, and notebook
//! extraction. During Phase 1 it exists as the landing zone for parser slices
//! so Python can shrink back toward CLI/MCP interfaces.

pub fn grammar_status() -> dagayn_grammars::GrammarStatus {
    dagayn_grammars::status()
}
