//! Calls written in `macro_rules!` bodies: once at the definition for the
//! paths the body spells out, and at each invocation of a macro the file
//! defines for the paths a matched metavariable completes.

use super::*;

fn calls(edges: &[ParsedEdge]) -> Vec<(&str, &str, i64)> {
    edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| (edge.source.as_str(), edge.target.as_str(), edge.line))
        .collect()
}

fn targets<'a>(edges: &'a [ParsedEdge], target: &str) -> Vec<&'a ParsedEdge> {
    edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.target == target)
        .collect()
}

const GRAMMAR_PARSER: &str = r#"macro_rules! grammar_parser {
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
    };
}

grammar_parser!(new_rust_parser, "lang-rust", rust_language);
grammar_parser!(
    new_go_parser,
    "lang-go",
    go_language
);

fn build() {
    grammar_parser!(new_c_parser, "lang-c", c_language);
}
"#;

#[test]
fn invocations_of_a_local_macro_call_what_its_metavariables_complete() {
    let (nodes, edges) = parse_rust("src/parsers.rs", GRAMMAR_PARSER.as_bytes());
    let calls = calls(&edges);
    for expected in [
        ("src/parsers.rs", "dagayn_grammars::rust_language", 15),
        ("src/parsers.rs", "dagayn_grammars::go_language", 16),
        ("src/parsers.rs::build", "dagayn_grammars::c_language", 23),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    let rust = targets(&edges, "dagayn_grammars::rust_language");
    assert_eq!(rust.len(), 1, "{rust:?}");
    assert_eq!(rust[0].extra["in_macro"], true);
    assert_eq!(rust[0].extra["macro_expansion"], "grammar_parser!");
    // The functions the macro generates are not nodes, and their
    // declarations are not calls.
    assert!(!nodes.iter().any(|node| node.name == "new_rust_parser"));
    assert!(
        !calls
            .iter()
            .any(|(_, target, _)| target.contains("new_rust_parser")),
        "{calls:?}"
    );
}

#[test]
fn paths_a_macro_body_spells_out_are_called_once_from_the_definition() {
    let (_, edges) = parse_rust("src/parsers.rs", GRAMMAR_PARSER.as_bytes());
    // `tree_sitter::Parser::new()` does not depend on the arguments: one
    // edge at the body's line, not one per invocation.
    let new = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.target.ends_with("Parser::new"))
        .collect::<Vec<_>>();
    assert_eq!(new.len(), 1, "{edges:#?}");
    assert_eq!(new[0].source, "src/parsers.rs");
    assert_eq!(new[0].line, 5);
    assert_eq!(new[0].extra["macro_body"], "grammar_parser!");
    // An attribute is not a call, and a method on a variable of the
    // expansion has no type to bind to.
    let calls = calls(&edges);
    for absent in ["cfg", "set_language", "is_ok"] {
        assert!(
            !calls.iter().any(|(_, target, _)| *target == absent),
            "{absent} in {calls:?}"
        );
    }
    // A path with a metavariable no invocation completes is not a call.
    assert!(
        !calls.iter().any(|(_, target, _)| target.contains('$')),
        "{calls:?}"
    );
}

#[test]
fn dollar_crate_names_the_defining_crate() {
    let source = br#"macro_rules! call_util {
    ($name:ident) => {
        $crate::util::setup();
        $crate::util::$name()
    };
}

fn run() {
    call_util!(teardown);
}
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    let calls = calls(&edges);
    for expected in [
        ("src/lib.rs", "crate::util::setup", 3),
        ("src/lib.rs::run", "crate::util::teardown", 9),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
}

#[test]
fn the_first_rule_that_matches_the_invocation_expands() {
    let source = br#"fn go() {}

macro_rules! dispatch {
    (@local $f:ident) => { $f() };
    ($m:ident :: $f:ident) => { $m::$f() };
    ($f:ident) => { other::$f() };
}

fn run() {
    dispatch!(@local go);
    dispatch!(net::fetch);
    dispatch!(stop);
}
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    let calls = calls(&edges);
    for expected in [
        ("src/lib.rs::run", "src/lib.rs::go", 10),
        ("src/lib.rs::run", "net::fetch", 11),
        ("src/lib.rs::run", "other::stop", 12),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    assert!(
        !calls
            .iter()
            .any(|(_, target, _)| matches!(*target, "other::go" | "other::net" | "stop")),
        "{calls:?}"
    );
}

#[test]
fn expression_arguments_are_not_expanded_twice() {
    let source = br#"fn compute(x: u32) -> u32 { x }
fn report(x: u32) {}

macro_rules! apply {
    ($value:expr, $f:ident) => { $f($value) };
}

fn run() {
    apply!(compute(1), report);
}
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    assert_eq!(
        targets(&edges, "src/lib.rs::compute").len(),
        1,
        "{edges:#?}"
    );
    assert_eq!(targets(&edges, "src/lib.rs::report").len(), 1, "{edges:#?}");
}

#[test]
fn macros_the_matcher_cannot_read_expand_to_nothing() {
    // A repetition in the pattern, and a name defined twice: which rule or
    // definition applies is not known, so no invocation adds a call.
    let source = br#"macro_rules! each {
    ($($f:ident),*) => { $( other::$f(); )* };
}

mod a {
    macro_rules! twice { ($f:ident) => { first::$f() }; }
}
mod b {
    macro_rules! twice { ($f:ident) => { second::$f() }; }
}

fn run() {
    each!(one, two);
    twice!(three);
}
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    let calls = calls(&edges);
    for absent in ["other::one", "other::two", "first::three", "second::three"] {
        assert!(
            !calls.iter().any(|(_, target, _)| *target == absent),
            "{absent} in {calls:?}"
        );
    }
}

#[test]
fn attributes_in_macro_arguments_are_not_calls() {
    let source = br#"fn run() {
    make! {
        #![allow(unused)]
        #[derive(Debug)]
        struct Wrapper(u8);
    }
    tokio::select! {
        value = recv() => handle(value),
        _ = sleep() => {}
    }
    let rows = vec![vec![row(1)], vec![row(2)]];
}
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    let calls = calls(&edges);
    assert!(
        !calls
            .iter()
            .any(|(_, target, _)| matches!(*target, "derive" | "allow")),
        "{calls:?}"
    );
    // Calls in a brace-delimited macro's arguments, and in a nested
    // `vec![...]`.
    for expected in ["recv", "handle", "sleep", "row"] {
        assert!(
            calls.iter().any(|(_, target, _)| *target == expected),
            "{expected} not in {calls:?}"
        );
    }
}

#[test]
fn expanded_paths_resolve_into_other_crates_of_the_workspace() {
    let mut root = std::env::temp_dir();
    root.push(format!(
        "dagayn-rust-macros-workspace-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in [
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/grammars/Cargo.toml",
            "[package]\nname = \"dagayn-grammars\"\n",
        ),
        (
            "crates/grammars/src/lib.rs",
            "pub fn rust_language() {}\npub fn go_language() {}\npub fn c_language() {}\n",
        ),
        (
            "crates/parser/Cargo.toml",
            "[package]\nname = \"parser\"\n\n[dependencies]\ndagayn-grammars = { path = \"../grammars\" }\ntree-sitter = \"0.25\"\n",
        ),
        ("crates/parser/src/lib.rs", GRAMMAR_PARSER),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file_in_repo(
        Some(&root),
        "crates/parser/src/lib.rs",
        GRAMMAR_PARSER.as_bytes(),
    );
    let rust = targets(&edges, "rust_language");
    assert_eq!(rust.len(), 1, "{edges:#?}");
    assert_eq!(rust[0].extra["module_file"], "crates/grammars/src/lib.rs");
    assert_eq!(rust[0].extra["path"], "dagayn_grammars::rust_language");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_local_macro_invoked_in_another_macros_arguments_expands() {
    let source = br#"macro_rules! batch {
    ($collector:ident, $db:expr) => {
        async { $collector($db).await }
    };
}

async fn collect(db: u8) {
    futures::try_join!(batch!(collect_reports, db), batch!(collect_follows, db));
}
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    let calls = calls(&edges);
    for expected in [
        ("src/lib.rs::collect", "collect_reports", 8),
        ("src/lib.rs::collect", "collect_follows", 8),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
}

#[test]
fn every_single_token_fragment_and_nested_group_matches() {
    let source = br#"macro_rules! route {
    ($vis:vis $life:lifetime $n:literal $any:tt $body:block [$p:path] => $h:ident) => {
        $p::$h()
    };
}
macro_rules! odd {
    ($x:unknown_fragment) => { never::$x() };
}

fn run() {
    route!(pub(crate) 'a -1 + { body() } [api::v1] => list);
    route!('a 2 x {} [api::v2] => show);
    route!(pub 'a "s" (y) {} [api] => index);
    odd!(z);
}
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    let calls = calls(&edges);
    for expected in [
        ("src/lib.rs::run", "api::v1::list", 11),
        ("src/lib.rs::run", "api::v2::show", 12),
        ("src/lib.rs::run", "api::index", 13),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    assert!(
        !calls
            .iter()
            .any(|(_, target, _)| target.starts_with("never")),
        "{calls:?}"
    );
}

#[test]
fn paths_in_macro_arguments_start_at_their_first_segment() {
    // A primitive (`i64`) or a contextual keyword (`default`) starts a path;
    // `<T>::from` has no readable start and is no call of a bare `from`.
    let source = br#"fn from(x: u8) {}

fn run(x: bool) {
    assert_eq!(i64::from(x), 1);
    check!(default::all_widths());
    assert_eq!(<u8>::from(x), 1);
}
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    let calls = calls(&edges);
    assert!(
        edges
            .iter()
            .any(|edge| edge.extra["external_symbol"] == "i64::from"),
        "{calls:?}"
    );
    assert!(
        calls.contains(&("src/lib.rs::run", "default::all_widths", 5)),
        "{calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|(_, target, _)| matches!(*target, "from" | "src/lib.rs::from" | "all_widths")),
        "{calls:?}"
    );
}

#[test]
fn paths_a_body_cannot_name_are_not_calls() {
    // `Self` is the type an expansion's `impl` names, `<$t>::try_from` has
    // no readable start, and a pattern is no call.
    let source = br#"macro_rules! define_id {
    ($name:ident, $t:ty) => {
        impl $name {
            pub fn parse(value: &str) -> Self {
                let _ = <$t>::try_from(1);
                let Wrapped(inner) = value;
                match lookup(value) {
                    Found(Exact(id)) => Self(id),
                    _ => fallback(),
                }
            }
        }
    };
}

define_id!(AccountId, u8);
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    let calls = calls(&edges);
    for expected in ["lookup", "fallback"] {
        assert!(
            calls.iter().any(|(_, target, _)| *target == expected),
            "{expected} not in {calls:?}"
        );
    }
    for absent in ["Self", "try_from", "parse", "Wrapped", "Found", "Exact"] {
        assert!(
            !calls.iter().any(|(_, target, _)| *target == absent),
            "{absent} in {calls:?}"
        );
    }
}
