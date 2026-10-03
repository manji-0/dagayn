//! Manifest bridge extraction over temporary repositories.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{EXTRACTOR_ID, ManifestBridgeResult, discover_manifest_bridges};

struct TempRepo(PathBuf);

impl TempRepo {
    fn new(name: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dagayn-manifest-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn write(&self, rel: &str, content: &str) -> &Self {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
        self
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn edges(result: &ManifestBridgeResult) -> Vec<(&str, &str, &Value)> {
    result
        .edges
        .iter()
        .map(|edge| (edge.source.as_str(), edge.target.as_str(), &edge.extra))
        .collect()
}

#[test]
fn maturin_pyproject_bridges_to_its_crate_root() {
    let repo = TempRepo::new("maturin");
    repo.write(
        "pyproject.toml",
        "[tool.maturin]\nmanifest-path = \"rust/Cargo.toml\"\nmodule-name = \"pkg._core\"\n",
    )
    .write("rust/Cargo.toml", "[package]\nname = \"pkg-core\"\n")
    .write("rust/src/lib.rs", "");
    let result = discover_manifest_bridges(repo.path(), None);
    let found = edges(&result);
    assert_eq!(found.len(), 2);
    assert_eq!(
        (found[0].0, found[0].1),
        ("pyproject.toml", "rust/Cargo.toml")
    );
    assert_eq!(found[0].2["confidence"], json!(1.0));
    assert_eq!(found[0].2["module_name"], json!("pkg._core"));
    assert_eq!(
        (found[1].0, found[1].1),
        ("rust/Cargo.toml", "rust/src/lib.rs")
    );
    assert_eq!(found[1].2["python_module"], json!("pkg._core"));
    assert_eq!(found[1].2["lib_name"], json!("pkg_core"));
    let node = &result.nodes[0];
    assert_eq!(node.file_path, "pyproject.toml");
    assert_eq!(node.line_end, 3);
    assert_eq!(node.extra["extractor"], json!(EXTRACTOR_ID));
}

#[test]
fn wasm_bindgen_crate_carries_js_package_names() {
    let repo = TempRepo::new("wasm");
    repo.write(
        "crate/Cargo.toml",
        "[package]\nname = \"wcrate\"\n[lib]\ncrate-type = [\"cdylib\"]\n[dependencies]\nwasm-bindgen = \"0.2\"\n",
    )
    .write("crate/src/lib.rs", "")
    .write(
        "web/package.json",
        r#"{"scripts": {"build": "wasm-pack build ../crate --scope acme"},
            "dependencies": {"local-wasm": "file:../crate/pkg"}}"#,
    );
    let result = discover_manifest_bridges(repo.path(), None);
    let [(source, _, extra)] = edges(&result)[..] else {
        panic!("one edge expected");
    };
    assert_eq!(source, "crate/Cargo.toml");
    assert_eq!(
        extra["js_packages"],
        json!(["wcrate", "@acme/wcrate", "local-wasm"])
    );
    assert_eq!(extra["wasm_out_dirs"], json!(["crate/pkg"]));
}

#[test]
fn openapi_generator_links_schema_package_and_consumer() {
    let repo = TempRepo::new("openapi");
    repo.write(
        "openapitools.json",
        r#"{"generator-cli": {"generators": {"ts": {"inputSpec": "api.yaml", "output": "client"}}}}"#,
    )
    .write("api.yaml", "openapi: 3.0.0\n")
    .write("client/package.json", r#"{"name": "@acme/client"}"#)
    .write(
        "app/package.json",
        r#"{"name": "app", "dependencies": {"@acme/client": "*"}}"#,
    );
    let result = discover_manifest_bridges(repo.path(), None);
    let found = edges(&result);
    assert_eq!(found.len(), 2);
    assert_eq!(
        (found[0].0, found[0].1),
        ("api.yaml", "client/package.json")
    );
    assert_eq!(found[0].2["generator_name"], json!("ts"));
    assert_eq!(
        (found[1].0, found[1].1),
        ("app/package.json", "client/package.json")
    );
    assert_eq!(found[1].2["consumer_package_name"], json!("app"));
}

#[test]
fn cmake_shared_library_lists_its_sources() {
    let repo = TempRepo::new("cmake");
    repo.write(
        "CMakeLists.txt",
        "set(SRCS src/a.c src/b.cpp)\nadd_library(fast-sum SHARED ${SRCS} missing.c)\nadd_library(st STATIC src/a.c)\n",
    )
    .write("src/a.c", "")
    .write("src/b.cpp", "");
    let result = discover_manifest_bridges(repo.path(), None);
    let [(source, target, extra)] = edges(&result)[..] else {
        panic!("one edge expected");
    };
    assert_eq!((source, target), ("CMakeLists.txt", "src/a.c"));
    assert_eq!(extra["lib_name"], json!("fast_sum"));
    assert_eq!(extra["target_language"], json!("cpp"));
    assert_eq!(extra["source_files"], json!(["src/a.c", "src/b.cpp"]));
}

#[test]
fn scope_skips_manifests_and_drops_out_of_scope_targets() {
    let repo = TempRepo::new("scope");
    repo.write(
        "pyproject.toml",
        "[tool.maturin]\nmanifest-path = \"rust/Cargo.toml\"\n",
    )
    .write("rust/Cargo.toml", "[package]\nname = \"x\"\n")
    .write("rust/src/lib.rs", "")
    .write(
        "ignored/Cargo.toml",
        "[package]\nname = \"y\"\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .write("ignored/src/lib.rs", "");
    let scope: HashSet<String> = ["pyproject.toml", "rust/Cargo.toml"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let result = discover_manifest_bridges(repo.path(), Some(&scope));
    // The crate root edge names `rust/src/lib.rs`, an existing file outside
    // scope; `ignored/Cargo.toml` is never read.
    let found = edges(&result);
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].0, found[0].1),
        ("pyproject.toml", "rust/Cargo.toml")
    );
}
