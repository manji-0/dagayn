use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    File,
    Function,
    Class,
    Test,
    DocSection,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Function => "Function",
            Self::Class => "Class",
            Self::Test => "Test",
            Self::DocSection => "DocSection",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdgeKind {
    Calls,
    Contains,
    ImportsFrom,
    TestedBy,
    CrossArtifact,
    References,
    Inherits,
    DependsOn,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Calls => "CALLS",
            Self::Contains => "CONTAINS",
            Self::ImportsFrom => "IMPORTS_FROM",
            Self::TestedBy => "TESTED_BY",
            Self::CrossArtifact => "CROSS_ARTIFACT",
            Self::References => "REFERENCES",
            Self::Inherits => "INHERITS",
            Self::DependsOn => "DEPENDS_ON",
        }
    }
}

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
