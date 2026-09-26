use super::*;

#[test]
fn sync_fts_for_file_paths_does_not_drop_other_files() {
    let path = temp_db("fts-incr");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let alpha = NodeInput {
        kind: "Function".to_string(),
        name: "alpha_widget".to_string(),
        file_path: "src/a.py".to_string(),
        line_start: 1,
        line_end: 2,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let beta = NodeInput {
        kind: "Function".to_string(),
        name: "beta_gadget".to_string(),
        file_path: "src/b.py".to_string(),
        line_start: 1,
        line_end: 2,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    store
        .store_file_nodes_edges("src/a.py", &[alpha], &[], "hash-a", 0)
        .unwrap();
    store
        .store_file_nodes_edges("src/b.py", &[beta], &[], "hash-b", 0)
        .unwrap();
    let rebuilt = store.rebuild_fts_index().unwrap();
    assert!(rebuilt >= 2);

    let synced = store
        .sync_fts_for_file_paths(&["src/a.py".to_string()])
        .unwrap();
    assert!(synced >= 1);

    let fts_count: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM nodes_fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fts_count, rebuilt);

    let beta_hits: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM nodes_fts WHERE name MATCH 'beta_gadget'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(beta_hits, 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn rebuilds_fts_index() {
    let path = temp_db("fts");
    let mut store = GraphStore::open(&path).expect("open graph store");
    let file = NodeInput {
        kind: "File".to_string(),
        name: "app.py".to_string(),
        file_path: "app.py".to_string(),
        line_start: 1,
        line_end: 1,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };
    let func = NodeInput {
        kind: "Function".to_string(),
        name: "calculate_total".to_string(),
        file_path: "app.py".to_string(),
        line_start: 3,
        line_end: 5,
        language: "python".to_string(),
        parent_name: None,
        params: Some("()".to_string()),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    };

    store
        .store_file_nodes_edges("app.py", &[file, func], &[], "hash", 0)
        .unwrap();
    store
        .conn
        .execute("DROP TABLE IF EXISTS nodes_fts", [])
        .unwrap();

    assert_eq!(store.rebuild_fts_index().unwrap(), 2);
    let hit: String = store
        .conn
        .query_row(
            "SELECT name FROM nodes_fts WHERE name MATCH 'calculate*'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hit, "calculate_total");
    let _ = std::fs::remove_file(path);
}

#[test]
fn rebuilds_fts_index_segments_japanese_source() {
    let path = temp_db("fts-japanese");
    let source_root = {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "dagayn-rust-fts-japanese-src-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    };
    std::fs::write(
        source_root.join("design.md"),
        "# 日本語検索\n\nGraphStoreで自然言語検索を行う。\n",
    )
    .unwrap();

    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .set_metadata("repo_root", source_root.to_string_lossy().as_ref())
        .unwrap();
    let doc = NodeInput {
        kind: "DocSection".to_string(),
        name: "japanese-search".to_string(),
        file_path: "design.md".to_string(),
        line_start: 1,
        line_end: 1,
        language: "markdown".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"display_name": "日本語検索"}),
    };

    store
        .store_file_nodes_edges("design.md", &[doc], &[], "hash", 0)
        .unwrap();

    assert_eq!(store.rebuild_fts_index().unwrap(), 1);
    let doc_text: String = store
        .conn
        .query_row(
            "SELECT doc_text FROM nodes_fts WHERE name = 'japanese-search'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(doc_text.contains("GraphStore"));
    assert!(doc_text.contains("自然"));
    assert!(doc_text.contains("言語"));
    assert_eq!(
        store.get_metadata("fts_segmenter").unwrap().as_deref(),
        Some("lindera")
    );

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(source_root);
}

#[test]
fn rebuilds_fts_index_includes_structured_code_reference_text() {
    let path = temp_db("fts-structured-code-reference");
    let source_root = {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "dagayn-rust-fts-structured-src-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    };
    std::fs::write(
        source_root.join("service.py"),
        "def handle_failure(retry_budget):\n    retry_budget_exhausted = retry_budget <= 0\n    return retry_budget_exhausted\n",
    )
    .unwrap();

    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .set_metadata("repo_root", source_root.to_string_lossy().as_ref())
        .unwrap();
    let node = NodeInput {
        kind: "Function".to_string(),
        name: "handle_failure".to_string(),
        file_path: "service.py".to_string(),
        line_start: 1,
        line_end: 3,
        language: "python".to_string(),
        parent_name: None,
        params: Some("(retry_budget)".to_string()),
        return_type: Some("bool".to_string()),
        modifiers: None,
        is_test: false,
        extra: json!({"display_name": "Retry failure handler"}),
    };

    store
        .store_file_nodes_edges("service.py", &[node], &[], "hash", 0)
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE nodes SET signature = ? WHERE qualified_name = ?",
            params![
                "def handle_failure(retry_budget) -> bool",
                "service.py::handle_failure"
            ],
        )
        .unwrap();

    assert_eq!(store.rebuild_fts_index().unwrap(), 1);
    let doc_text: String = store
        .conn
        .query_row(
            "SELECT doc_text FROM nodes_fts WHERE name = 'handle_failure'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(doc_text.contains("kind: Function"));
    assert!(doc_text.contains("qualified: service.py::handle_failure"));
    assert!(doc_text.contains("signature: def handle_failure"));
    assert!(doc_text.contains("retry_budget_exhausted"));

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(source_root);
}

fn japanese_search_fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/japanese_search")
}

#[derive(Debug, Deserialize)]
struct JapaneseSearchCorpus {
    rebuild_budget_ms: f64,
    query_p95_ms: f64,
    min_indexed_nodes: i64,
    gates: Vec<JapaneseSearchGate>,
}

#[derive(Debug, Deserialize)]
struct JapaneseSearchGate {
    query: String,
    k: usize,
    #[serde(default)]
    match_mode: Option<String>,
    #[serde(default)]
    file_contains: Option<String>,
    #[serde(default)]
    name_any: Option<Vec<String>>,
}

fn collect_fixture_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    fn visit(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, files);
            } else {
                files.push(path);
            }
        }
    }
    visit(root, &mut files);
    files.sort();
    files
}

fn parsed_to_node_input(node: dagayn_parser::ParsedNode) -> NodeInput {
    NodeInput {
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
    }
}

fn parsed_to_edge_input(edge: dagayn_parser::ParsedEdge) -> EdgeInput {
    EdgeInput {
        kind: edge.kind.as_str().to_string(),
        source: edge.source,
        target: edge.target,
        file_path: edge.file_path.as_str().to_string(),
        line: edge.line,
        extra: edge.extra,
    }
}

fn fts_hit_records(store: &GraphStore, query: &str) -> (Vec<(String, String)>, &'static str) {
    let (hits, mode) = store.fts_query(query, 10).unwrap();
    let ids = hits.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let by_id = store.get_nodes_by_ids(&ids).unwrap();
    let records = ids
        .iter()
        .filter_map(|id| {
            by_id
                .get(id)
                .map(|node| (node.name.clone(), node.file_path.clone()))
        })
        .collect();
    (records, mode)
}

fn gate_matches(records: &[(String, String)], gate: &JapaneseSearchGate) -> bool {
    records.iter().take(gate.k).any(|(name, file)| {
        let file_ok = gate
            .file_contains
            .as_ref()
            .is_none_or(|needle| file.contains(needle));
        let name_ok = gate
            .name_any
            .as_ref()
            .is_none_or(|allowed| allowed.iter().any(|candidate| candidate == name));
        file_ok && name_ok
    })
}

/// Ranking gates against the mixed Japanese search fixture.
///
/// The 7-node inline corpus hid DocBody crowding and shared-term collisions.
/// This fixture is parsed with the real markdown/Python/Terraform parsers.
#[test]
fn japanese_fts_quality_gates_hit_inflected_and_identifier_queries() {
    let fixture = japanese_search_fixture_root();
    assert!(
        fixture.is_dir(),
        "missing Japanese search fixture at {}",
        fixture.display()
    );
    let spec: JapaneseSearchCorpus = serde_json::from_str(
        &std::fs::read_to_string(fixture.join("queries.json")).expect("queries.json"),
    )
    .expect("corpus spec");

    let path = temp_db("fts-japanese-quality");
    let mut store = GraphStore::open(&path).expect("open graph store");
    store
        .set_metadata("repo_root", fixture.to_string_lossy().as_ref())
        .unwrap();

    for file in collect_fixture_files(&fixture) {
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if matches!(name, "README.md" | "queries.json") {
            continue;
        }
        let rel = file
            .strip_prefix(&fixture)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let source = std::fs::read(&file).unwrap();
        let (nodes, edges) = dagayn_parser::parse_rust_owned_file(&rel, &source);
        if nodes.is_empty() {
            continue;
        }
        let node_inputs = nodes
            .into_iter()
            .map(parsed_to_node_input)
            .collect::<Vec<_>>();
        let edge_inputs = edges
            .into_iter()
            .map(parsed_to_edge_input)
            .collect::<Vec<_>>();
        store
            .store_file_nodes_edges(&rel, &node_inputs, &edge_inputs, "hash", 0)
            .unwrap();
    }

    let rebuild_started = std::time::Instant::now();
    let indexed = store.rebuild_fts_index().unwrap();
    let rebuild_ms = rebuild_started.elapsed().as_secs_f64() * 1000.0;
    assert!(
        indexed >= spec.min_indexed_nodes,
        "indexed {indexed} nodes, expected at least {}",
        spec.min_indexed_nodes
    );
    assert!(
        rebuild_ms < spec.rebuild_budget_ms,
        "FTS rebuild took {rebuild_ms:.2}ms; budget is {}ms",
        spec.rebuild_budget_ms
    );

    for gate in &spec.gates {
        let (records, mode) = fts_hit_records(&store, &gate.query);
        assert!(
            gate_matches(&records, gate),
            "gate failed for {:?}: mode={mode} ranking={records:?}",
            gate.query
        );
        if let Some(want) = gate.match_mode.as_deref() {
            assert_eq!(
                mode, want,
                "match_mode for {:?}: ranking={records:?}",
                gate.query
            );
        }
    }

    let mut query_ms = Vec::new();
    for _ in 0..50 {
        let started = std::time::Instant::now();
        let _ = store.fts_query("自然言語検索する", 10).unwrap();
        query_ms.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    query_ms.sort_by(|left, right| left.partial_cmp(right).unwrap());
    let p95 = query_ms[(query_ms.len() * 95 / 100).min(query_ms.len() - 1)];
    assert!(
        p95 < spec.query_p95_ms,
        "inflected Japanese query p95 {p95:.3}ms exceeds {}ms",
        spec.query_p95_ms
    );

    let identifier_tokens: String = store
        .conn
        .query_row(
            "SELECT identifier_tokens FROM nodes_fts WHERE name = 'ユーザー取得'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        identifier_tokens.contains("ユーザー") || identifier_tokens.contains("ユー"),
        "CJK identifiers should land in identifier_tokens: {identifier_tokens}"
    );

    let _ = std::fs::remove_file(path);
}
