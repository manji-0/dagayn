use super::*;

#[test]
fn reexported_targets_follow_the_package_imports() {
    // `from pkg import NodeInfo` names `pkg/__init__.py::NodeInfo`, which
    // `__init__.py` imports from `pkg/types.py`; `Store` comes from a star
    // import of `pkg/store.py`, renamed on the way (`from .core import
    // Store as Engine` in `pkg/store.py`).
    let path = temp_db("reexports");
    let mut store = GraphStore::open(&path).expect("open");
    let names =
        |pairs: &[(&str, &str)]| json!(pairs.iter().map(|(n, a)| [n, a]).collect::<Vec<_>>());
    for (file, nodes, edges) in [
        (
            "pkg/types.py",
            vec![
                file_node("pkg/types.py"),
                class_node("NodeInfo", "pkg/types.py"),
                method_node("load", "pkg/types.py", "NodeInfo"),
            ],
            vec![],
        ),
        (
            "pkg/core.py",
            vec![file_node("pkg/core.py"), class_node("Store", "pkg/core.py")],
            vec![],
        ),
        (
            "pkg/store.py",
            vec![file_node("pkg/store.py")],
            vec![EdgeInput {
                extra: json!({"module": ".core", "names": names(&[("Store", "Engine")])}),
                ..edge(
                    "IMPORTS_FROM",
                    "pkg/store.py",
                    "pkg/core.py",
                    "pkg/store.py",
                    1,
                )
            }],
        ),
        (
            "pkg/__init__.py",
            vec![file_node("pkg/__init__.py")],
            vec![
                EdgeInput {
                    extra: json!({"module": ".types", "names": names(&[("NodeInfo", "NodeInfo")])}),
                    ..edge(
                        "IMPORTS_FROM",
                        "pkg/__init__.py",
                        "pkg/types.py",
                        "pkg/__init__.py",
                        1,
                    )
                },
                EdgeInput {
                    extra: json!({"module": ".store", "names": []}),
                    ..edge(
                        "IMPORTS_FROM",
                        "pkg/__init__.py",
                        "pkg/store.py",
                        "pkg/__init__.py",
                        2,
                    )
                },
            ],
        ),
        (
            "app.py",
            vec![file_node("app.py"), function_node("main", "app.py")],
            vec![
                edge(
                    "CALLS",
                    "app.py::main",
                    "pkg/__init__.py::NodeInfo",
                    "app.py",
                    3,
                ),
                edge(
                    "CALLS",
                    "app.py::main",
                    "pkg/__init__.py::NodeInfo.load",
                    "app.py",
                    4,
                ),
                edge(
                    "CALLS",
                    "app.py::main",
                    "pkg/__init__.py::Engine",
                    "app.py",
                    5,
                ),
                edge(
                    "CALLS",
                    "app.py::main",
                    "pkg/__init__.py::Missing",
                    "app.py",
                    6,
                ),
            ],
        ),
    ] {
        store
            .store_file_nodes_edges(file, &nodes, &edges, "", 0)
            .expect("store");
    }
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 3);
    let mut stmt = store
        .conn
        .prepare(
            "SELECT line, target_qualified, confidence_tier FROM edges \
             WHERE kind = 'CALLS' ORDER BY line",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| <(i64, String, String)>::try_from(row))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (3, "pkg/types.py::NodeInfo".to_string(), "HIGH".to_string()),
            (
                4,
                "pkg/types.py::NodeInfo.load".to_string(),
                "HIGH".to_string()
            ),
            (5, "pkg/core.py::Store".to_string(), "HIGH".to_string()),
            (
                6,
                "pkg/__init__.py::Missing".to_string(),
                "EXTRACTED".to_string()
            ),
        ]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn reexported_targets_reach_a_class_the_module_getattr_defines() {
    // `from pkg import Store` with `pkg/__init__.py` returning a class its
    // module `__getattr__` defines (PEP 562).
    let path = temp_db("reexports-getattr");
    let mut store = GraphStore::open(&path).expect("open");
    store
        .store_file_nodes_edges(
            "pkg/__init__.py",
            &[
                file_node("pkg/__init__.py"),
                function_node("__getattr__", "pkg/__init__.py"),
                method_node("Store", "pkg/__init__.py", "__getattr__"),
            ],
            &[],
            "",
            0,
        )
        .expect("store pkg");
    store
        .store_file_nodes_edges(
            "app.py",
            &[file_node("app.py"), function_node("main", "app.py")],
            &[edge(
                "CALLS",
                "app.py::main",
                "pkg/__init__.py::Store",
                "app.py",
                2,
            )],
            "",
            0,
        )
        .expect("store app");
    assert_eq!(store.resolve_bare_call_targets().unwrap(), 1);
    let target: String = store
        .conn
        .query_row(
            "SELECT target_qualified FROM edges WHERE kind = 'CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, "pkg/__init__.py::__getattr__.Store");
    let _ = std::fs::remove_file(path);
}
