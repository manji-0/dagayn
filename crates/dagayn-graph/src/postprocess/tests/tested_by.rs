use super::*;

fn tested_by_rows(store: &GraphStore) -> Vec<(String, String, i64, String)> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT source_qualified, target_qualified, line, confidence_tier FROM edges \
             WHERE kind = 'TESTED_BY' ORDER BY line, source_qualified",
        )
        .unwrap();
    stmt.query_map([], |row| <(_, _, _, _)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
}

/// A test file importing `a.py`: `test_run` calls `helper` (resolvable),
/// `missing` (not), and both `helper` / `other` on line 4, as parsers
/// emit them: bare `CALLS` plus the mirrored bare `TESTED_BY`.
fn store_test_file(store: &mut GraphStore, test_edges: &[EdgeInput]) {
    store
        .store_file_nodes_edges(
            "a.py",
            &[
                file_node("a.py"),
                function_node("helper", "a.py"),
                function_node("other", "a.py"),
            ],
            &[],
            "",
            0,
        )
        .expect("store a");
    let mut edges = vec![edge(
        "IMPORTS_FROM",
        "tests/test_b.py",
        "a.py",
        "tests/test_b.py",
        1,
    )];
    edges.extend_from_slice(test_edges);
    store
        .store_file_nodes_edges(
            "tests/test_b.py",
            &[
                file_node("tests/test_b.py"),
                test_node("test_run", "tests/test_b.py"),
            ],
            &edges,
            "",
            0,
        )
        .expect("store test");
}

#[test]
fn resolving_bare_calls_moves_tested_by_to_the_resolved_target() {
    let path = temp_db("tested-by-sync");
    let mut store = GraphStore::open(&path).expect("open");
    let test = "tests/test_b.py::test_run";
    let file = "tests/test_b.py";
    store_test_file(
        &mut store,
        &[
            edge("CALLS", test, "helper", file, 2),
            edge("TESTED_BY", "helper", test, file, 2),
            edge("CALLS", test, "missing", file, 3),
            edge("TESTED_BY", "missing", test, file, 3),
            edge("CALLS", test, "other", file, 4),
            edge("TESTED_BY", "other", test, file, 4),
        ],
    );
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 2);
    assert_eq!(
        tested_by_rows(&store),
        vec![
            (
                "a.py::helper".to_string(),
                test.to_string(),
                2,
                "HIGH".to_string()
            ),
            // `missing` stays unresolved: its TESTED_BY names no node.
            (
                "a.py::other".to_string(),
                test.to_string(),
                4,
                "HIGH".to_string()
            ),
        ]
    );
    // Idempotent: a second run changes nothing.
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 0);
    assert_eq!(tested_by_rows(&store).len(), 2);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tested_by_follows_calls_resolved_by_an_earlier_run() {
    // Graphs built before the sync: the CALLS edge is already resolved,
    // the TESTED_BY edge is still bare. A rewrite that would duplicate an
    // edge drops the bare copy, and a TESTED_BY of a symbol that is not a
    // node (`fetch`, `ext.py::Remote.fetch` of a file not in the graph)
    // is dropped.
    let path = temp_db("tested-by-stale");
    let mut store = GraphStore::open(&path).expect("open");
    let test = "tests/test_b.py::test_run";
    let file = "tests/test_b.py";
    store_test_file(
        &mut store,
        &[
            edge("CALLS", test, "a.py::helper", file, 2),
            edge("TESTED_BY", "helper", test, file, 2),
            edge("CALLS", test, "a.py::other", file, 3),
            edge("CALLS", test, "other", file, 3),
            edge("TESTED_BY", "a.py::other", test, file, 3),
            edge("TESTED_BY", "other", test, file, 3),
            edge("CALLS", test, "a.py::helper", file, 5),
            edge("TESTED_BY", "a.py::helper", test, file, 5),
            edge("TESTED_BY", "helper", test, file, 5),
            edge("CALLS", test, "ext.py::Remote.fetch", file, 6),
            edge("CALLS", test, "fetch", file, 6),
            edge("TESTED_BY", "ext.py::Remote.fetch", test, file, 6),
            edge("TESTED_BY", "fetch", test, file, 6),
        ],
    );
    store.resolve_bare_call_targets().unwrap();
    let rows = tested_by_rows(&store)
        .into_iter()
        .map(|(source, _, line, _)| (source, line))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            ("a.py::helper".to_string(), 2),
            ("a.py::other".to_string(), 3),
            ("a.py::helper".to_string(), 5),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn tested_by_comes_back_when_a_later_run_resolves_the_call() {
    // The first run cannot resolve `later` and drops its bare TESTED_BY;
    // once `a.py` declares it, the next run resolves the call and the test
    // covers it again.
    let path = temp_db("tested-by-later");
    let mut store = GraphStore::open(&path).expect("open");
    let test = "tests/test_b.py::test_run";
    let file = "tests/test_b.py";
    store_test_file(
        &mut store,
        &[
            edge("CALLS", test, "later", file, 2),
            edge("TESTED_BY", "later", test, file, 2),
        ],
    );
    store.resolve_bare_call_targets().unwrap();
    assert!(tested_by_rows(&store).is_empty());
    store
        .store_file_nodes_edges(
            "a.py",
            &[
                file_node("a.py"),
                function_node("helper", "a.py"),
                function_node("later", "a.py"),
            ],
            &[],
            "",
            0,
        )
        .expect("store a");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    assert_eq!(
        tested_by_rows(&store),
        vec![(
            "a.py::later".to_string(),
            test.to_string(),
            2,
            "HIGH".to_string()
        )]
    );
    let _ = std::fs::remove_file(path);
}
