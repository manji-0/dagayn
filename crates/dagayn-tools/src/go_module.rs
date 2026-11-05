//! Where the Go tools look for a Go target: its module, less the
//! directories `go list ./...` of that module skips (#325).
//!
//! The go command builds the packages under the directory of the module's
//! `go.mod`, except directories (and files) whose names start with `_` or
//! `.`, directories named `testdata`, and directories under another
//! `go.mod`, which are another module. Rows there (chi's `_examples/`) are
//! not callers or tests of the module as its tools build it, so replies list
//! them apart from the answer.

use std::collections::HashMap;

use dagayn_graph::{GraphNode, GraphStore};
use serde_json::Value;

/// Why the Go tools building the target's module skip a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outside {
    /// Under another `go.mod`: the directory of that `go.mod`.
    OtherModule(String),
    /// Under (or itself) a name starting with `_` or `.`, or a `testdata`
    /// directory: the path up to and including that name.
    Ignored(String),
}

impl Outside {
    /// The reply's `reason` for a row.
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Outside::OtherModule(_) => "other_module",
            Outside::Ignored(path) => match path.rsplit('/').next().unwrap_or(path) {
                "testdata" => "testdata",
                name if name.starts_with('.') => "dot_name",
                _ => "underscore_name",
            },
        }
    }

    /// The place, as a summary names it: `_examples/` (a directory) or
    /// `_gen.go` (a file); the root module is `./`.
    pub(crate) fn place(&self) -> String {
        match self {
            Outside::OtherModule(dir) if dir.is_empty() => "./".to_string(),
            Outside::OtherModule(dir) => format!("{dir}/"),
            Outside::Ignored(path) if path.ends_with(".go") => path.clone(),
            Outside::Ignored(path) => format!("{path}/"),
        }
    }

    /// Why the Go tools skip the place, as a summary says it.
    pub(crate) fn why(&self) -> &'static str {
        match self.reason() {
            "other_module" => "its own module",
            "testdata" => "testdata",
            "dot_name" => "name starts with .",
            _ => "name starts with _",
        }
    }
}

/// The module of a Go target and the module of each file asked about.
pub(crate) struct GoScope<'a> {
    store: &'a GraphStore,
    /// The directory of the target's `go.mod` (`""` at the root).
    module_dir: String,
    modules: HashMap<String, Option<String>>,
}

impl<'a> GoScope<'a> {
    /// The scope of `node`, a Go node in a module the go command builds;
    /// `None` for another language, a file in no module, a graph parsed
    /// before File nodes recorded `go_module_dir`, or a target that is
    /// itself where the Go tools do not look (asking about an example's
    /// function keeps its neighbours in the answer).
    pub(crate) fn of(store: &'a GraphStore, node: &GraphNode) -> Option<Self> {
        if node.language != "go" {
            return None;
        }
        Self::of_file(store, &node.file_path)
    }

    /// The scope of the Go file `file`.
    pub(crate) fn of_file(store: &'a GraphStore, file: &str) -> Option<Self> {
        if !file.ends_with(".go") {
            return None;
        }
        let module_dir = file_module_dir(store, file)?;
        let mut scope = Self {
            store,
            module_dir,
            modules: HashMap::new(),
        };
        scope.outside(file).is_none().then_some(scope)
    }

    /// Why the Go tools building this module skip `file`, if they do. A
    /// file that is not Go code is never skipped.
    pub(crate) fn outside(&mut self, file: &str) -> Option<Outside> {
        if !file.ends_with(".go") {
            return None;
        }
        let store = self.store;
        let module = self
            .modules
            .entry(file.to_string())
            .or_insert_with(|| file_module_dir(store, file))
            .clone();
        if let Some(module) = module
            && module != self.module_dir
        {
            return Some(Outside::OtherModule(module));
        }
        let below = if self.module_dir.is_empty() {
            file
        } else {
            file.strip_prefix(self.module_dir.as_str())?
                .strip_prefix('/')?
        };
        let mut end = file.len() - below.len();
        for (index, part) in below.split('/').enumerate() {
            end += part.len() + usize::from(index > 0);
            let last = end == file.len();
            if part.starts_with('_') || part.starts_with('.') || (!last && part == "testdata") {
                return Some(Outside::Ignored(file[..end].to_string()));
            }
        }
        None
    }
}

/// The `go_module_dir` the File node of `file` records.
fn file_module_dir(store: &GraphStore, file: &str) -> Option<String> {
    store
        .get_node(file)
        .ok()
        .flatten()?
        .extra
        .get("go_module_dir")
        .and_then(Value::as_str)
        .map(str::to_string)
}
