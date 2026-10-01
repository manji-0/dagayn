//! The Zig standard library: the modules every compilation can import by
//! name, `@import("std")` and `@import("builtin")` (the build's target and
//! options).

const ZIG_STD_MODULES: &[&str] = &["std", "builtin"];

/// The standard module an `@import` names: `std` for `@import("std")`, not
/// a file (`"util.zig"`) or a module the build script adds (`"known"`).
pub(crate) fn zig_std_module(import: &str) -> Option<&'static str> {
    ZIG_STD_MODULES
        .iter()
        .find(|module| **module == import)
        .copied()
}
