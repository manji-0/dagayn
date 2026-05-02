use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ParsedNode {
    pub kind: String,
    pub name: String,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub language: String,
    pub parent_name: Option<String>,
    pub params: Option<String>,
    pub return_type: Option<String>,
    pub modifiers: Option<String>,
    pub is_test: bool,
    pub extra: Value,
}

#[derive(Debug, Serialize)]
pub struct ParsedEdge {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub file_path: String,
    pub line: i64,
    pub extra: Value,
}
