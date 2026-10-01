//! The Rust standard library (`std`, `core`, `alloc`).

/// Crates of the Rust standard library a path can start with.
const RUST_STD_CRATES: &[&str] = &["std", "core", "alloc"];

/// Names the Rust prelude brings into every module: types and traits called
/// through a path (`Vec::new`, `String::from`), and the functions and enum
/// variants called bare (`Some(x)`, `drop(x)`).
const RUST_PRELUDE_TYPES: &[&str] = &[
    "Box",
    "String",
    "Vec",
    "Option",
    "Result",
    "ToString",
    "ToOwned",
    "Iterator",
    "IntoIterator",
    "Default",
    "Clone",
    "From",
    "Into",
    "TryFrom",
    "TryInto",
    "FromIterator",
    "AsRef",
    "AsMut",
    "PartialEq",
    "Eq",
    "PartialOrd",
    "Ord",
    "Extend",
    "Drop",
];
const RUST_PRELUDE_FUNCTIONS: &[&str] = &["Some", "None", "Ok", "Err", "drop"];

/// Primitive types, whose associated functions (`u32::from`,
/// `str::from_utf8`) are defined by the standard library.
const RUST_PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
    "i128", "isize", "f32", "f64",
];

/// Macros exported by `std` (and `core`), in scope without a `use`.
const RUST_STD_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "cfg",
    "column",
    "compile_error",
    "concat",
    "dbg",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "env",
    "eprint",
    "eprintln",
    "file",
    "format",
    "format_args",
    "include",
    "include_bytes",
    "include_str",
    "line",
    "matches",
    "module_path",
    "option_env",
    "panic",
    "print",
    "println",
    "stringify",
    "thread_local",
    "todo",
    "unimplemented",
    "unreachable",
    "vec",
    "write",
    "writeln",
];

/// The standard-library crate a Rust path names: its first segment when that
/// is `std`, `core`, or `alloc`.
pub(crate) fn rust_std_crate(segments: &[String]) -> Option<&'static str> {
    let first = segments.first()?;
    RUST_STD_CRATES
        .iter()
        .find(|name| **name == first.as_str())
        .copied()
}

pub(crate) fn is_rust_prelude_type(name: &str) -> bool {
    RUST_PRELUDE_TYPES.contains(&name) || RUST_PRIMITIVES.contains(&name)
}

pub(crate) fn is_rust_prelude_function(name: &str) -> bool {
    RUST_PRELUDE_FUNCTIONS.contains(&name)
}

pub(crate) fn is_rust_std_macro(name: &str) -> bool {
    RUST_STD_MACROS.contains(&name)
}
