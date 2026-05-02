//! Rust parser crate.
//!
//! The public crate root stays stable and lightweight while the parser
//! implementation lives behind an internal module.

mod core;

pub use core::*;
