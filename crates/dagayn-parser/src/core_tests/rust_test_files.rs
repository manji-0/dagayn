//! Rust files compiled only under test: the targets of `#[cfg(test)] mod`
//! declarations (with or without `#[path]`) and the files below them are
//! test code, whatever their names.

use super::*;

fn repo(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let mut root = std::env::temp_dir();
    root.push(format!("dagayn-rust-test-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

const BODY: &str = "pub struct Fixture;\nfn helper() -> u8 { crate::work() }\n#[test]\nfn runs() { helper(); }\n";

/// `(name, kind, is_test)` of every node the parser records for `file`.
fn nodes_of(root: &std::path::Path, file: &str) -> Vec<(String, String, bool)> {
    let mut parser = RustOwnedParser::new();
    let source = std::fs::read(root.join(file)).unwrap();
    let (nodes, _) = parser.parse_file_in_repo(Some(root), file, &source);
    nodes
        .into_iter()
        .map(|node| (node.name, node.kind.as_str().to_string(), node.is_test))
        .collect()
}

#[test]
fn files_declared_under_cfg_test_are_test_code() {
    let root = repo(
        "declared",
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            (
                "src/lib.rs",
                "pub fn work() -> u8 { 1 }\nmod contests;\n#[cfg(test)]\nmod foo_tests;\n#[cfg(all(test, feature = \"x\"))]\nmod tests;\n#[cfg(test)]\n#[path = \"support/x.rs\"]\nmod t;\n#[cfg(not(test))]\nmod not_tests;\n#[cfg(any(test, feature = \"x\"))]\nmod either;\n",
            ),
            ("src/foo_tests.rs", BODY),
            ("src/tests/mod.rs", "mod child;\npub struct Shared;\n"),
            ("src/tests/child.rs", BODY),
            ("src/support/x.rs", BODY),
            ("src/contests.rs", "pub struct Contest;\npub fn rank() -> u8 { 1 }\n"),
            ("src/not_tests.rs", "pub fn live() {}\n"),
            ("src/either.rs", "pub fn maybe() {}\n"),
        ],
    );
    for file in [
        "src/foo_tests.rs",
        "src/tests/mod.rs",
        "src/tests/child.rs",
        "src/support/x.rs",
    ] {
        let nodes = nodes_of(&root, file);
        assert!(nodes.len() > 1, "{file}: {nodes:?}");
        assert!(
            nodes.iter().all(|(_, _, is_test)| *is_test),
            "{file}: {nodes:?}"
        );
        // Functions are tests, as in a `tests/` directory; types keep
        // their kind.
        if file != "src/tests/mod.rs" {
            assert!(nodes.contains(&("helper".into(), "Test".into(), true)), "{file}");
            assert!(nodes.contains(&("Fixture".into(), "Class".into(), true)), "{file}");
        }
    }
    for file in ["src/contests.rs", "src/not_tests.rs", "src/either.rs", "src/lib.rs"] {
        let nodes = nodes_of(&root, file);
        assert!(
            nodes.iter().all(|(_, _, is_test)| !*is_test),
            "{file}: {nodes:?}"
        );
    }
}

#[test]
fn a_file_the_binary_also_compiles_stays_production() {
    let root = repo(
        "shared",
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", "#[cfg(test)]\nmod common;\n"),
            ("src/main.rs", "mod common;\nfn main() {}\n"),
            ("src/common.rs", "pub fn shared() {}\n"),
        ],
    );
    let nodes = nodes_of(&root, "src/common.rs");
    assert!(nodes.iter().all(|(_, _, is_test)| !*is_test), "{nodes:?}");
}

#[test]
fn every_node_of_a_tests_directory_file_is_test_code() {
    let root = repo(
        "integration",
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", "pub fn work() -> u8 { 1 }\n"),
            ("tests/it.rs", "use app::work;\nstruct Harness;\nfn helper() -> u8 { work() }\n#[test]\nfn runs() { helper(); }\n"),
        ],
    );
    let nodes = nodes_of(&root, "tests/it.rs");
    assert!(nodes.iter().all(|(_, _, is_test)| *is_test), "{nodes:?}");
    assert!(nodes.contains(&("Harness".into(), "Class".into(), true)));
}
