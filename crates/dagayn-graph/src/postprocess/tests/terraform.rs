use super::*;

fn terraform_node(kind: &str, name: &str, file_path: &str) -> NodeInput {
    NodeInput {
        kind: kind.to_string(),
        language: "terraform".to_string(),
        ..function_node(name, file_path)
    }
}

fn terraform_file_node(file_path: &str) -> NodeInput {
    NodeInput {
        language: "terraform".to_string(),
        ..file_node(file_path)
    }
}

fn reference_edge(source: &str, target: &str, file_path: &str, line: i64) -> EdgeInput {
    EdgeInput {
        kind: "REFERENCES".to_string(),
        source: source.to_string(),
        target: target.to_string(),
        file_path: file_path.to_string(),
        line,
        extra: json!({}),
    }
}

#[test]
fn resolves_terraform_references_within_module_directory() {
    let path = temp_db("terraform-module-refs");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "infra/variables.tf",
            &[
                terraform_file_node("infra/variables.tf"),
                terraform_node("Function", "var.region", "infra/variables.tf"),
                terraform_node("Function", "var.dup", "infra/variables.tf"),
            ],
            &[],
            "",
            0,
        )
        .expect("store variables");
    store
        .store_file_nodes_edges(
            "infra/override.tf",
            &[
                terraform_file_node("infra/override.tf"),
                terraform_node("Function", "var.dup", "infra/override.tf"),
            ],
            &[],
            "",
            0,
        )
        .expect("store override");
    store
        .store_file_nodes_edges(
            "infra/modules/net/variables.tf",
            &[
                terraform_file_node("infra/modules/net/variables.tf"),
                terraform_node("Function", "var.cidr", "infra/modules/net/variables.tf"),
            ],
            &[],
            "",
            0,
        )
        .expect("store child module");
    let source = "infra/main.tf::resource.aws_vpc.main";
    store
        .store_file_nodes_edges(
            "infra/main.tf",
            &[
                terraform_file_node("infra/main.tf"),
                terraform_node("Class", "resource.aws_vpc.main", "infra/main.tf"),
            ],
            &[
                reference_edge(source, "var.region", "infra/main.tf", 1),
                // Declared twice in the module: ambiguous, stays bare.
                reference_edge(source, "var.dup", "infra/main.tf", 2),
                // Only declared in a child module directory: out of scope.
                reference_edge(source, "var.cidr", "infra/main.tf", 3),
                reference_edge(source, "var.missing", "infra/main.tf", 4),
            ],
            "",
            0,
        )
        .expect("store main");
    // A non-Terraform file with the same bare target is left alone.
    store
        .store_file_nodes_edges(
            "infra/app.py",
            &[
                file_node("infra/app.py"),
                function_node("main", "infra/app.py"),
            ],
            &[reference_edge(
                "infra/app.py::main",
                "var.region",
                "infra/app.py",
                1,
            )],
            "",
            0,
        )
        .expect("store python");

    assert_eq!(store.resolve_terraform_module_references().unwrap(), 1);
    let rows = {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT file_path, line, target_qualified, confidence_tier FROM edges \
                 WHERE kind='REFERENCES' ORDER BY file_path, line",
            )
            .unwrap();
        stmt.query_map([], |row| <(String, i64, String, String)>::try_from(row))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    let targets = rows
        .iter()
        .map(|(file, line, target, _)| (file.as_str(), *line, target.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        vec![
            ("infra/app.py", 1, "var.region"),
            ("infra/main.tf", 1, "infra/variables.tf::var.region"),
            ("infra/main.tf", 2, "var.dup"),
            ("infra/main.tf", 3, "var.cidr"),
            ("infra/main.tf", 4, "var.missing"),
        ]
    );
    assert_eq!(rows[1].3, "HIGH");
    // The resolved edge survives endpoint demotion; the bare ones do not.
    store.demote_unresolved_endpoint_edges().unwrap();
    let tier: String = store
        .conn
        .query_row(
            "SELECT confidence_tier FROM edges WHERE file_path='infra/main.tf' AND line=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tier, "HIGH");
    let _ = std::fs::remove_file(path);
}
