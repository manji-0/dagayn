//! The standard libraries of the languages: calls and imports into them are
//! recorded as edges to the package (`subprocess`, `builtins`, `std`, `fmt`)
//! instead of bare names. Marked `external`, they never bind to a same-named
//! symbol of the repository, and keep the confidence their evidence gives
//! them instead of being demoted to `LOW` for having no node.
//!
//! Each language keeps its tables and predicates in its own module; the
//! extractors call [`mark_stdlib_edge`] with the package they resolved.

use serde_json::{Value, json};

pub(crate) mod bash;
pub(crate) mod c;
pub(crate) mod cpp;
pub(crate) mod csharp;
pub(crate) mod dart;
pub(crate) mod elixir;
pub(crate) mod gdscript;
pub(crate) mod go;
pub(crate) mod java;
pub(crate) mod javascript;
pub(crate) mod julia;
pub(crate) mod kotlin;
pub(crate) mod lua;
pub(crate) mod objc;
pub(crate) mod perl;
pub(crate) mod php;
pub(crate) mod python;
pub(crate) mod r;
pub(crate) mod ruby;
pub(crate) mod rust;
pub(crate) mod scala;
pub(crate) mod swift;
pub(crate) mod zig;

/// How sure the extractor is that an edge reaches the standard library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StdlibEvidence {
    /// Named through the standard library itself: an import of it, a path or
    /// import alias rooted at it (`subprocess.run`, `std::fs::read`,
    /// `fmt.Println`), or a member of a value it just constructed
    /// (`Path(p).read_text()`). `HIGH`.
    Certain,
    /// Inferred from a name alone: a builtin or prelude name the file does
    /// not shadow (`len`, `Some`, `println`), or a method of a variable
    /// bound to a standard-library value. `MEDIUM`.
    Likely,
}

impl StdlibEvidence {
    fn tier(self) -> (&'static str, f64) {
        match self {
            Self::Certain => ("HIGH", 0.9),
            Self::Likely => ("MEDIUM", 0.6),
        }
    }
}

/// Points a `CALLS` / `IMPORTS_FROM` / `REFERENCES` edge at a
/// standard-library `package`, keeping the name as written in
/// `external_symbol` (`subprocess.run`, `Vec::new`), with the confidence
/// `evidence` gives it.
pub(crate) fn mark_stdlib_edge(
    target: &mut String,
    extra: &mut Value,
    package: &str,
    evidence: StdlibEvidence,
) {
    mark_external_edge(target, extra, package, evidence);
    extra["stdlib"] = json!(true);
}

/// Points an edge at an external `package` that is not the standard
/// library (a third-party crate or module): the same shape as
/// [`mark_stdlib_edge`] without `stdlib`. The evidence reads the same way:
/// named through an import of the package is certain, a package the
/// repository only fails to contain is likely.
pub(crate) fn mark_external_edge(
    target: &mut String,
    extra: &mut Value,
    package: &str,
    evidence: StdlibEvidence,
) {
    let symbol = std::mem::replace(target, package.to_string());
    if !extra.is_object() {
        *extra = json!({});
    }
    let (tier, confidence) = evidence.tier();
    extra["external"] = json!(true);
    extra["external_package"] = json!(package);
    extra["confidence_tier"] = json!(tier);
    extra["confidence"] = json!(confidence);
    if !symbol.is_empty() && symbol != package {
        extra["external_symbol"] = json!(symbol);
    }
}
