use super::*;

#[test]
fn resolves_markdown_artifact_refs() {
    let path = temp_db("markdown-refs");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let target = NodeInput {
        kind: "Class".to_string(),
        name: "BridgePattern".to_string(),
        file_path: "parser.py".to_string(),
        line_start: 1,
        line_end: 10,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let edge = EdgeInput {
        kind: "CROSS_ARTIFACT".to_string(),
        source: "docs/spec.md::section".to_string(),
        target: "<unresolved:BridgePattern>".to_string(),
        file_path: "docs/spec.md".to_string(),
        line: 5,
        extra: json!({
            "relationship_role": "describes_symbol",
            "bridge_kind": "documentation",
            "evidence_kind": "markdown_code_span",
            "evidence_source": "code_span",
            "source_language": "markdown",
            "target_language": "unknown",
            "confidence": 0.2,
            "confidence_tier": "LOW",
            "original_symbol_name": "BridgePattern",
        }),
    };

    store
        .store_file_batch(&[(
            "parser.py".to_string(),
            vec![target],
            vec![edge],
            "hash".to_string(),
            0,
        )])
        .unwrap();

    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (1, 0, 0, 0)
    );
    let row = store
        .conn
        .query_row(
            "SELECT target_qualified, confidence, confidence_tier, extra \
                 FROM edges WHERE kind = 'CROSS_ARTIFACT'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0, "parser.py::BridgePattern");
    assert_eq!(row.1, 0.4);
    assert_eq!(row.2, "MEDIUM");
    let extra: Value = serde_json::from_str(&row.3).unwrap();
    assert!(extra.get("unresolved_target_name").is_none());
    assert_eq!(extra["original_symbol_name"], "BridgePattern");
    assert_eq!(extra["target_language"], "python");
    assert_eq!(extra["confidence"], 0.4);
    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (0, 0, 0, 0)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn prunes_unresolved_markdown_code_span_refs() {
    let path = temp_db("markdown-code-span-prune");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let edge = EdgeInput {
        kind: "CROSS_ARTIFACT".to_string(),
        source: "docs/spec.md::section".to_string(),
        target: "<unresolved:OrdinaryConcept>".to_string(),
        file_path: "docs/spec.md".to_string(),
        line: 5,
        extra: json!({
            "relationship_role": "describes_symbol",
            "bridge_kind": "documentation",
            "evidence_kind": "markdown_code_span",
            "evidence_source": "code_span",
            "target_language": "unknown",
            "confidence": 0.2,
            "confidence_tier": "LOW",
            "original_symbol_name": "OrdinaryConcept",
        }),
    };

    store
        .store_file_batch(&[(
            "docs/spec.md".to_string(),
            vec![],
            vec![edge],
            "hash".to_string(),
            0,
        )])
        .unwrap();

    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (0, 1, 0, 0)
    );
    let count: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE kind = 'CROSS_ARTIFACT'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn markdown_resolver_skips_terraform_handler_bridges() {
    let path = temp_db("markdown-skip-terraform-handler");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let decoy = NodeInput {
        kind: "Class".to_string(),
        name: "hello.main".to_string(),
        file_path: "app/decoy.py".to_string(),
        line_start: 1,
        line_end: 10,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let edge = EdgeInput {
        kind: "CROSS_ARTIFACT".to_string(),
        source: "infra/main.tf::resource.aws_lambda_function.auth".to_string(),
        target: "<unresolved:hello.main>".to_string(),
        file_path: "infra/main.tf".to_string(),
        line: 4,
        extra: json!({
            "relationship_role": "maps_entrypoint",
            "bridge_kind": "manifest_link",
            "evidence_kind": "config",
            "evidence_source": "handler",
            "source_language": "terraform",
            "target_language": "unknown",
            "confidence": 0.8,
            "confidence_tier": "HIGH",
            "original_symbol_name": "hello.main",
        }),
    };

    store
        .store_file_batch(&[(
            "infra/main.tf".to_string(),
            vec![decoy],
            vec![edge],
            "hash".to_string(),
            0,
        )])
        .unwrap();

    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (0, 0, 0, 0)
    );
    let row = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CROSS_ARTIFACT'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(row, "<unresolved:hello.main>");
    let _ = std::fs::remove_file(path);
}

#[test]
fn markdown_resolver_retiers_resolved_code_span_bridges_left_at_high() {
    let path = temp_db("markdown-code-span-retier");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let target = NodeInput {
        kind: "Class".to_string(),
        name: "BridgePattern".to_string(),
        file_path: "parser.py".to_string(),
        line_start: 1,
        line_end: 10,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let bridge = |line: i64, extra: Value| EdgeInput {
        kind: "CROSS_ARTIFACT".to_string(),
        source: "docs/spec.md::section".to_string(),
        target: "parser.py::BridgePattern".to_string(),
        file_path: "docs/spec.md".to_string(),
        line,
        extra,
    };
    // Resolved before implicit code spans were capped at MEDIUM.
    let legacy_code_span = bridge(
        5,
        json!({
            "relationship_role": "describes_symbol",
            "bridge_kind": "documentation",
            "evidence_kind": "markdown_code_span",
            "evidence_source": "code_span",
            "source_language": "markdown",
            "target_language": "python",
            "confidence": 0.8,
            "confidence_tier": "HIGH",
            "original_symbol_name": "BridgePattern",
        }),
    );
    // An explicit (non-code-span) bridge legitimately stays HIGH.
    let explicit = bridge(
        9,
        json!({
            "relationship_role": "describes_symbol",
            "bridge_kind": "documentation",
            "evidence_kind": "markdown_link",
            "evidence_source": "link",
            "source_language": "markdown",
            "target_language": "python",
            "confidence": 0.8,
            "confidence_tier": "HIGH",
            "original_symbol_name": "BridgePattern",
        }),
    );

    store
        .store_file_batch(&[(
            "parser.py".to_string(),
            vec![target],
            vec![legacy_code_span, explicit],
            "hash".to_string(),
            0,
        )])
        .unwrap();

    let rows = |store: &GraphStore| {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT line, confidence, confidence_tier, extra FROM edges \
                 WHERE kind = 'CROSS_ARTIFACT' ORDER BY line",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
    };
    let before = rows(&store);
    assert_eq!((before[0].1, before[0].2.as_str()), (0.8, "HIGH"));

    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (0, 0, 1, 0)
    );
    let after = rows(&store);
    assert_eq!(
        (after[0].0, after[0].1, after[0].2.as_str()),
        (5, 0.4, "MEDIUM")
    );
    let extra: Value = serde_json::from_str(&after[0].3).unwrap();
    assert_eq!(extra["confidence"], 0.4);
    assert_eq!(extra["confidence_tier"], "MEDIUM");
    assert_eq!(
        (after[1].0, after[1].1, after[1].2.as_str()),
        (9, 0.8, "HIGH")
    );

    // Idempotent once the stored tier matches.
    assert_eq!(
        store.resolve_markdown_artifact_refs().unwrap(),
        (0, 0, 0, 0)
    );
    let _ = std::fs::remove_file(path);
}
