use crate::*;

use serde_json::{Value, json};

use std::path::PathBuf;

mod bare_names;
mod endpoints;
mod external_calls;
mod impl_members;
mod reexports;
mod returned;
mod scip_overlay;
mod terraform;
mod tested_by;

fn temp_db(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "dagayn-postprocess-{}-{}.db",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn function_node(name: &str, file_path: &str) -> NodeInput {
    NodeInput {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end: 2,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    }
}

fn class_node(name: &str, file_path: &str) -> NodeInput {
    NodeInput {
        kind: "Class".to_string(),
        ..function_node(name, file_path)
    }
}

fn method_node(name: &str, file_path: &str, owner: &str) -> NodeInput {
    NodeInput {
        parent_name: Some(owner.to_string()),
        ..function_node(name, file_path)
    }
}

fn file_node(file_path: &str) -> NodeInput {
    NodeInput {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end: 1,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    }
}

fn edge(kind: &str, source: &str, target: &str, file_path: &str, line: i64) -> EdgeInput {
    EdgeInput {
        kind: kind.to_string(),
        source: source.to_string(),
        target: target.to_string(),
        file_path: file_path.to_string(),
        line,
        extra: json!({}),
    }
}

fn test_node(name: &str, file_path: &str) -> NodeInput {
    NodeInput {
        kind: "Test".to_string(),
        is_test: true,
        ..function_node(name, file_path)
    }
}

fn namespaced_file_node(file_path: &str, namespace: &str) -> NodeInput {
    let mut node = file_node(file_path);
    node.extra = json!({"namespaces": [namespace]});
    node
}
