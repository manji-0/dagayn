use crate::*;

use std::path::PathBuf;

/// A repository on disk, parsed with the real parsers into a fresh graph.
struct Repo {
    root: PathBuf,
    db: PathBuf,
    store: GraphStore,
}

impl Repo {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("dagayn-tf-bridges-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let db = root.with_extension("db");
        let _ = std::fs::remove_file(&db);
        let mut store = GraphStore::open(&db).expect("open");
        store
            .set_metadata("repo_root", root.to_string_lossy().as_ref())
            .unwrap();
        let mut repo = Self { root, db, store };
        for (path, text) in files {
            repo.write(path, text);
        }
        repo
    }

    /// Write `path` and store what the parser makes of it.
    fn write(&mut self, path: &str, text: &str) {
        let file = self.root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        let (nodes, edges) = dagayn_parser::parse_rust_owned_file(path, text.as_bytes());
        let nodes: Vec<NodeInput> = nodes
            .into_iter()
            .map(|node| NodeInput {
                kind: node.kind.as_str().to_string(),
                name: node.name,
                file_path: node.file_path.as_str().to_string(),
                line_start: node.line_start,
                line_end: node.line_end,
                language: node.language,
                parent_name: node.parent_name,
                params: node.params,
                return_type: node.return_type,
                modifiers: node.modifiers,
                is_test: node.is_test,
                extra: node.extra,
            })
            .collect();
        let edges: Vec<EdgeInput> = edges
            .into_iter()
            .map(|edge| EdgeInput {
                kind: edge.kind.as_str().to_string(),
                source: edge.source,
                target: edge.target,
                file_path: edge.file_path.as_str().to_string(),
                line: edge.line,
                extra: edge.extra,
            })
            .collect();
        self.store
            .store_file_nodes_edges(path, &nodes, &edges, "hash", 0)
            .expect("store");
    }

    /// `(source, target, evidence_source, confidence_tier)` of every
    /// Terraform bridge after the step ran.
    fn bridges(&mut self) -> Vec<(String, String, String, String)> {
        self.store
            .resolve_terraform_artifact_refs()
            .expect("resolve");
        let mut stmt = self
            .store
            .conn
            .prepare(
                "SELECT source_qualified, target_qualified, \
                 json_extract(extra, '$.evidence_source'), confidence_tier FROM edges \
                 WHERE kind = 'CROSS_ARTIFACT' \
                 AND json_extract(extra, '$.source_language') = 'terraform' \
                 ORDER BY source_qualified, target_qualified",
            )
            .unwrap();
        stmt.query_map([], |row| <(String, String, String, String)>::try_from(row))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    }

    fn bridge_extra(&self, source: &str, target: &str) -> Value {
        let raw: String = self
            .store
            .conn
            .query_row(
                "SELECT extra FROM edges WHERE kind = 'CROSS_ARTIFACT' \
                 AND source_qualified = ? AND target_qualified = ?",
                [source, target],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str(&raw).unwrap()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_file(&self.db);
    }
}

fn row(source: &str, target: &str, evidence: &str, tier: &str) -> (String, String, String, String) {
    (
        source.to_string(),
        target.to_string(),
        evidence.to_string(),
        tier.to_string(),
    )
}

#[test]
fn resource_paths_and_a_unique_handler_become_bridges() {
    let mut repo = Repo::new(
        "resource",
        &[
            ("app/hello.py", "def main(event, context):\n    return 1\n"),
            (
                "infra/main.tf",
                "resource \"aws_lambda_function\" \"auth\" {\n  filename = \"${path.module}/../app/hello.py\"\n  handler  = \"hello.main\"\n}\n",
            ),
        ],
    );
    let lambda = "infra/main.tf::resource.aws_lambda_function.auth";
    assert_eq!(
        repo.bridges(),
        vec![
            row(lambda, "app/hello.py", "filename", "HIGH"),
            row(lambda, "app/hello.py::main", "handler", "HIGH"),
        ]
    );
    let extra = repo.bridge_extra(lambda, "app/hello.py");
    assert_eq!(extra["attr_lines"], json!([2, 2]));
    assert_eq!(extra["target_language"], "python");
    // Idempotent: a second run replaces the set with the same edges.
    assert_eq!(repo.bridges().len(), 2);
}
