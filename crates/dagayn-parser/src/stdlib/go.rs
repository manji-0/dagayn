//! The Go standard library: the packages shipped with the toolchain (`fmt`,
//! `net/http`, `encoding/json`) and the predeclared functions (`len`,
//! `append`), which `go doc` files under the pseudo-package `builtin`.

/// First elements of the standard library's import paths. A module path
/// needs no dot either (`myapp/internal`), so a dotless path alone does not
/// make a package standard: its root must also be one of these.
const GO_STD_ROOTS: &[&str] = &[
    "archive",
    "bufio",
    "builtin",
    "bytes",
    "cmp",
    "compress",
    "container",
    "context",
    "crypto",
    "database",
    "debug",
    "embed",
    "encoding",
    "errors",
    "expvar",
    "flag",
    "fmt",
    "go",
    "hash",
    "html",
    "image",
    "index",
    "io",
    "iter",
    "log",
    "maps",
    "math",
    "mime",
    "net",
    "os",
    "path",
    "plugin",
    "reflect",
    "regexp",
    "runtime",
    "slices",
    "sort",
    "strconv",
    "strings",
    "structs",
    "sync",
    "syscall",
    "testing",
    "text",
    "time",
    "unicode",
    "unique",
    "unsafe",
    "weak",
];

/// Functions the language predeclares in the universe scope.
const GO_BUILTIN_FUNCTIONS: &[&str] = &[
    "append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "len", "make", "max",
    "min", "new", "panic", "print", "println", "real", "recover",
];

/// Whether an import path names a standard-library package: `fmt`,
/// `net/http`, `math/rand/v2`, but not `github.com/x/y`, `myapp/internal`,
/// or cgo's pseudo-package `C`.
pub(crate) fn is_go_std_import(path: &str) -> bool {
    let root = path.split('/').next().unwrap_or_default();
    !root.contains('.') && GO_STD_ROOTS.contains(&root)
}

/// The name a package is referred to by when imported without one: the last
/// element of its path, skipping a major-version suffix (`math/rand/v2` is
/// `rand`).
pub(crate) fn go_default_import_name(path: &str) -> &str {
    let mut elements = path.rsplit('/');
    let last = elements.next().unwrap_or(path);
    let is_version = last
        .strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    match (is_version, elements.next()) {
        (true, Some(previous)) => previous,
        _ => last,
    }
}

pub(crate) fn is_go_builtin_function(name: &str) -> bool {
    GO_BUILTIN_FUNCTIONS.contains(&name)
}
