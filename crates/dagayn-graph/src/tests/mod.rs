use super::*;
use crate::helpers::write_tx;

use serde_json::json;
use std::path::PathBuf;

mod analysis;
mod bridges;
mod centrality;
mod communities;
mod flows;
mod impact;
mod schema;
mod search;
mod storage;
mod summaries;

fn temp_db(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("dagayn-rust-{}-{}.db", name, std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn flow_test_node(kind: &str, name: &str, file: &str) -> NodeInput {
    NodeInput {
        kind: kind.to_string(),
        name: name.to_string(),
        file_path: file.to_string(),
        line_start: 1,
        line_end: 10,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: Value::Object(Default::default()),
    }
}

fn flow_test_call(source: &str, target: &str, file: &str) -> EdgeInput {
    EdgeInput {
        kind: "CALLS".to_string(),
        source: source.to_string(),
        target: target.to_string(),
        file_path: file.to_string(),
        line: 2,
        extra: Value::Object(Default::default()),
    }
}
