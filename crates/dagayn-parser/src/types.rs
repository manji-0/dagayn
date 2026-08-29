use std::borrow::Borrow;
use std::ffi::OsStr;
use std::fmt;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

/// Shared path for every node and edge produced from one file.
///
/// Clone is an `Arc` bump. Parse entry points intern once with [`FilePath::new`]
/// and pass the same handle down; the SQLite / Python boundary still stores a
/// plain string.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FilePath(Arc<str>);

impl FilePath {
    pub fn new(path: &str) -> Self {
        Self(Arc::from(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        Arc::ptr_eq(&this.0, &other.0)
    }
}

impl Deref for FilePath {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for FilePath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<OsStr> for FilePath {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(self.as_str())
    }
}

impl AsRef<Path> for FilePath {
    fn as_ref(&self) -> &Path {
        Path::new(self.as_str())
    }
}

impl Borrow<str> for FilePath {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FilePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<str> for FilePath {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for FilePath {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for FilePath {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<FilePath> for str {
    fn eq(&self, other: &FilePath) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<FilePath> for &str {
    fn eq(&self, other: &FilePath) -> bool {
        *self == other.as_str()
    }
}

impl PartialEq<FilePath> for String {
    fn eq(&self, other: &FilePath) -> bool {
        self == other.as_str()
    }
}

impl Serialize for FilePath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Closed node vocabulary used while parsing. The SQLite / Python boundary
/// still stores these as the `as_str()` labels; the enum is the in-memory
/// form so a kind cannot be an arbitrary string.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NodeKind {
    File,
    Function,
    Class,
    Type,
    Test,
    DocSection,
    DocBody,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Function => "Function",
            Self::Class => "Class",
            Self::Type => "Type",
            Self::Test => "Test",
            Self::DocSection => "DocSection",
            Self::DocBody => "DocBody",
        }
    }
}

impl PartialEq<str> for NodeKind {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for NodeKind {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl Serialize for NodeKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Closed edge vocabulary used while parsing. Persisted labels stay the
/// `as_str()` forms (`CALLS`, `IMPLEMENTS`, …).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EdgeKind {
    Calls,
    Contains,
    ImportsFrom,
    TestedBy,
    CrossArtifact,
    References,
    Inherits,
    Implements,
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
            Self::Implements => "IMPLEMENTS",
            Self::DependsOn => "DEPENDS_ON",
        }
    }
}

impl PartialEq<str> for EdgeKind {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for EdgeKind {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl Serialize for EdgeKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Serialize)]
pub struct ParsedNode {
    pub kind: NodeKind,
    pub name: String,
    pub file_path: FilePath,
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
    pub kind: EdgeKind,
    pub source: String,
    pub target: String,
    pub file_path: FilePath,
    pub line: i64,
    pub extra: Value,
}

#[cfg(test)]
mod tests {
    use super::{EdgeKind, FilePath, NodeKind};

    #[test]
    fn file_path_clones_share_one_allocation() {
        let path = FilePath::new("src/app.py");
        let clone = path.clone();
        assert!(FilePath::ptr_eq(&path, &clone));
        assert!(path == "src/app.py");
    }

    #[test]
    fn node_kind_labels_match_schema() {
        assert_eq!(NodeKind::File.as_str(), "File");
        assert_eq!(NodeKind::Type.as_str(), "Type");
        assert_eq!(NodeKind::DocBody.as_str(), "DocBody");
        assert!(NodeKind::Function == "Function");
    }

    #[test]
    fn edge_kind_labels_match_schema() {
        assert_eq!(EdgeKind::Calls.as_str(), "CALLS");
        assert_eq!(EdgeKind::Implements.as_str(), "IMPLEMENTS");
        assert!(EdgeKind::ImportsFrom == "IMPORTS_FROM");
    }
}
