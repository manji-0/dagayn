//! Compiled grammar provisioning for the Rust parser.
//!
//! This crate is intentionally small while Phase 1 focuses on the graph writer.
//! Phase 3 will move pinned Markdown and Terraform grammar compilation here.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrammarStatus {
    Pending,
}

pub fn status() -> GrammarStatus {
    GrammarStatus::Pending
}
