//! Scope dependencies on a built repository: a bare target matched by name
//! stays in the source's language family (docs/SAP-METRICS.md#type-name-fallback-resolution).

use std::collections::BTreeSet;
use std::path::PathBuf;

use dagayn_build::{BuildOptions, PostprocessLevel, db_path_for_build, full_build};

use super::*;

struct Repo(PathBuf);

impl Repo {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "dagayn-architecture-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".git")).expect("create repo");
        Self(root.canonicalize().expect("canonical repo"))
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    fn snapshot(&self) -> Snapshot {
        let mut store = GraphStore::open(db_path_for_build(&self.0).expect("db")).expect("store");
        full_build(
            &self.0,
            &mut store,
            &BuildOptions {
                recurse_submodules: false,
                postprocess: PostprocessLevel::Full,
            },
        )
        .expect("build");
        Snapshot::read(&store).expect("snapshot")
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_bare_target_matches_only_names_of_its_own_language_family() {
    let repo = Repo::new("bare-family");
    // Rust `impl From` names the standard trait; the only repository node
    // called `From` is a TypeScript method.
    repo.write(
        "rust/src/ty.rs",
        "pub struct Prim;\n\nimpl From<i64> for Prim {\n    fn from(_v: i64) -> Self {\n        Prim\n    }\n}\n",
    );
    repo.write("rust/src/base.rs", "pub struct Base;\n");
    repo.write(
        "web/field.ts",
        "export class Field {\n  From(): number {\n    return 1;\n  }\n}\n",
    );
    repo.write(
        "web/shape.ts",
        "export class Shape {\n  area(): number {\n    return 0;\n  }\n}\n",
    );
    // `Base` is only a Rust struct; `Mixin` and `Shape` are Python classes
    // of another directory, reached without an import.
    repo.write(
        "py/handler.py",
        "class Handler(Base):\n    pass\n\n\nclass Plugin(Mixin):\n    pass\n\n\nclass Square(Shape):\n    pass\n",
    );
    repo.write("py/lib/mixin.py", "class Mixin:\n    pass\n");
    repo.write("py/shapes/shape.py", "class Shape:\n    pass\n");
    let snapshot = repo.snapshot();
    let view = View::review();
    let pairs: BTreeSet<(String, String)> = snapshot.dependencies(&view).into_iter().collect();
    let pair = |source: &str, target: &str| (source.to_string(), target.to_string());
    assert!(!pairs.contains(&pair("rust/src", "web")), "{pairs:?}");
    assert!(!pairs.contains(&pair("py", "rust/src")), "{pairs:?}");
    assert!(!pairs.contains(&pair("py", "web")), "{pairs:?}");
    // Within Python, a name one scope holds still resolves; one a
    // TypeScript class shares stays ambiguous, as it was.
    assert!(pairs.contains(&pair("py", "py/lib")), "{pairs:?}");
    assert!(!pairs.contains(&pair("py", "py/shapes")), "{pairs:?}");
    let sap = sap_metrics(&snapshot, &view, "package", None);
    let web = sap
        .iter()
        .find(|metric| metric["scope_key"] == "web")
        .unwrap_or_else(|| panic!("{sap:?}"));
    assert_eq!(web["ca"], 0, "{web}");
}
