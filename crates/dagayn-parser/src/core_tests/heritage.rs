//! Inheritance edges across languages: bases are named without their type
//! arguments, and a type argument is never a base.

use super::*;

fn heritage(file_path: &str, source: &str) -> Vec<(String, String, String)> {
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file(file_path, source.as_bytes());
    let mut found: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == "INHERITS" || edge.kind == "IMPLEMENTS")
        .map(|edge| {
            let source = edge.source.rsplit("::").next().unwrap().to_string();
            (source, edge.kind.as_str().to_string(), edge.target.clone())
        })
        .collect();
    found.sort();
    found
}

fn edges(expected: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    let mut expected: Vec<_> = expected
        .iter()
        .map(|(s, k, t)| (s.to_string(), k.to_string(), t.to_string()))
        .collect();
    expected.sort();
    expected
}

#[test]
fn kotlin_bases_skip_type_arguments() {
    assert_eq!(
        heritage("a.kt", "class A : B<String>(), C<Int>, pkg.D\n"),
        edges(&[
            ("A", "INHERITS", "B"),
            ("A", "IMPLEMENTS", "C"),
            ("A", "IMPLEMENTS", "pkg.D"),
        ])
    );
}

#[test]
fn swift_bases_skip_type_arguments() {
    assert_eq!(
        heritage("a.swift", "class A: B<String>, C {}\nstruct S: Q<Int> {}\n"),
        edges(&[
            ("A", "INHERITS", "B"),
            ("A", "INHERITS", "C"),
            ("S", "INHERITS", "Q"),
        ])
    );
}

#[test]
fn dart_bases_mixins_and_interfaces() {
    assert_eq!(
        heritage(
            "a.dart",
            "class A extends B<String> with M<int> implements C<D>, E {}\n"
        ),
        edges(&[
            ("A", "INHERITS", "B"),
            ("A", "INHERITS", "M"),
            ("A", "IMPLEMENTS", "C"),
            ("A", "IMPLEMENTS", "E"),
        ])
    );
}

#[test]
fn java_bases_drop_generic_arguments_across_lines() {
    assert_eq!(
        heritage(
            "A.java",
            "interface R extends JpaRepository<User,\n        Integer> {}\nclass A extends pkg.B<String> implements C<D>, E {}\n"
        ),
        edges(&[
            ("A", "IMPLEMENTS", "C"),
            ("A", "IMPLEMENTS", "E"),
            ("A", "INHERITS", "pkg.B"),
            ("R", "INHERITS", "JpaRepository"),
        ])
    );
}
