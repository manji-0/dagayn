//! The units a repository declares and the dependencies between them: the
//! map `architecture_analysis_tool(mode="overview")` returns
//! (docs/plans/ARCHITECTURE-TOOL-TARGET.md#the-map).
//!
//! A unit is what a manifest or the language defines: a Cargo crate, an npm
//! package, a Go package, a Python import package, a Terraform module. Units
//! are found on disk from the directories of indexed files, so manifests
//! need not be indexed. A file belongs to the deepest unit whose directory
//! holds it; a code file in no unit falls back to its top-level directory.
//! Tests, examples, benches, fixtures, and vendored code are not units and
//! not endpoints of unit edges; tests are counted for the unit they test.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use dagayn_graph::{GraphEdge, GraphNode, is_reportable_bridge};
use serde_json::{Value, json};

use crate::findings::{CODE_LANGUAGES, is_production_code, is_test_node};

/// Symbols per unit that `surface` lists.
const SURFACE_SIZE: usize = 3;
/// Node kinds counted as a unit's symbols.
const SYMBOL_KINDS: &[&str] = &["Function", "Class", "Type"];
/// Directory names whose manifests describe test data, samples, or
/// vendored code rather than units of this repository.
const NON_UNIT_DIRS: &[&str] = &[
    "tests",
    "test",
    "__tests__",
    "testdata",
    "fixtures",
    "examples",
    "example",
    "benches",
    "bench",
    "vendor",
    "node_modules",
    "third_party",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Unit {
    pub name: String,
    pub kind: &'static str,
    /// Repo-relative directory, `""` for the repository root.
    pub path: String,
    /// Package names the manifest lists as runtime or build dependencies
    /// (Cargo `[dependencies]` / `[build-dependencies]`, npm `dependencies`
    /// / `peerDependencies`), or the module paths a Go package's `go.mod`
    /// requires.
    pub depends_on: Vec<String>,
    /// The path of the Go module a `go_package` or `go_module` belongs to.
    pub go_module: Option<String>,
}

/// The declared units and the unit of each file.
pub(crate) struct UnitIndex {
    pub units: Vec<Unit>,
    unit_of_file: HashMap<String, usize>,
}

impl UnitIndex {
    /// Units of the directories holding `files` (repo-relative).
    pub(crate) fn discover<'f>(root: &Path, files: impl IntoIterator<Item = &'f str>) -> Self {
        let mut units: Vec<Unit> = Vec::new();
        let mut by_dir: HashMap<String, Option<usize>> = HashMap::new();
        let mut unit_of_file = HashMap::new();
        let mut fallback: HashMap<String, usize> = HashMap::new();
        for file in files {
            let file = file.replace('\\', "/");
            let mut dir = parent(&file);
            let mut found = None;
            loop {
                let entry = by_dir
                    .entry(dir.to_string())
                    .or_insert_with(|| {
                        declared_unit(root, dir).map(|unit| {
                            units.push(unit);
                            units.len() - 1
                        })
                    })
                    .to_owned();
                if entry.is_some() {
                    found = entry;
                    break;
                }
                if dir.is_empty() {
                    break;
                }
                dir = parent(dir);
            }
            let index = match found {
                Some(index) => index,
                None => {
                    let top = file.split('/').next().unwrap_or("");
                    let top = if top == file { "" } else { top };
                    *fallback.entry(top.to_string()).or_insert_with(|| {
                        units.push(Unit {
                            name: if top.is_empty() {
                                "<root>".to_string()
                            } else {
                                top.to_string()
                            },
                            kind: "directory",
                            path: top.to_string(),
                            depends_on: Vec::new(),
                            go_module: None,
                        });
                        units.len() - 1
                    })
                }
            };
            unit_of_file.insert(file, index);
        }
        Self {
            units,
            unit_of_file,
        }
    }

    pub(crate) fn unit_of(&self, file: &str) -> Option<usize> {
        self.unit_of_file.get(file).copied()
    }

    /// Each unit's name, or `name (path)` for a name two units share (an
    /// npm package and a Python package both called `app`).
    pub(crate) fn labels(&self) -> Vec<String> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for unit in &self.units {
            *counts.entry(unit.name.as_str()).or_default() += 1;
        }
        self.units
            .iter()
            .map(|unit| {
                if counts[unit.name.as_str()] > 1 {
                    format!("{} ({})", unit.name, unit.path)
                } else {
                    unit.name.clone()
                }
            })
            .collect()
    }
}

/// The label of the declared unit of every code file (tests included: a
/// crate's `tests/` belong to the crate), for metrics computed per unit.
pub(crate) fn unit_scopes(root: &Path, nodes: &[GraphNode]) -> HashMap<String, String> {
    let mut files: Vec<&str> = nodes
        .iter()
        .filter(|node| node.kind == "File" && CODE_LANGUAGES.contains(&node.language.as_str()))
        .map(|node| node.file_path.as_str())
        .collect();
    files.sort_unstable();
    let index = UnitIndex::discover(root, files.iter().copied());
    let labels = index.labels();
    files
        .into_iter()
        .filter_map(|file| Some((file.to_string(), labels[index.unit_of(file)?].clone())))
        .collect()
}

fn parent(path: &str) -> &str {
    path.rfind('/').map_or("", |index| &path[..index])
}

/// The unit `dir` declares, if any; directories under test data, samples,
/// or vendored code declare none.
fn declared_unit(root: &Path, dir: &str) -> Option<Unit> {
    if dir.split('/').any(|part| NON_UNIT_DIRS.contains(&part)) {
        return None;
    }
    let base = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    let leaf = dir.rsplit('/').next().unwrap_or(dir);
    let unit = |name: String, kind| Unit {
        name,
        kind,
        path: dir.to_string(),
        depends_on: Vec::new(),
        go_module: None,
    };
    if let Ok(text) = std::fs::read_to_string(base.join("Cargo.toml"))
        && let Ok(manifest) = text.parse::<toml::Table>()
        && let Some(name) = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
    {
        let mut depends_on = Vec::new();
        for section in ["dependencies", "build-dependencies"] {
            for (key, spec) in manifest
                .get(section)
                .and_then(toml::Value::as_table)
                .into_iter()
                .flatten()
            {
                // `alias = { package = "real-name", ... }`
                let real = spec
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(key);
                depends_on.push(real.to_string());
            }
        }
        return Some(Unit {
            depends_on,
            ..unit(name.to_string(), "cargo_crate")
        });
    }
    if let Ok(text) = std::fs::read_to_string(base.join("package.json"))
        && let Ok(manifest) = serde_json::from_str::<Value>(&text)
        && let Some(name) = manifest.get("name").and_then(Value::as_str)
    {
        let depends_on = ["dependencies", "peerDependencies"]
            .iter()
            .filter_map(|section| manifest.get(section).and_then(Value::as_object))
            .flat_map(|deps| deps.keys().cloned())
            .collect();
        return Some(Unit {
            depends_on,
            ..unit(name.to_string(), "npm_package")
        });
    }
    if let Some(unit) = go_unit(root, dir) {
        return Some(unit);
    }
    // The outermost directory of a chain of `__init__.py`: the import
    // package. Inner packages are part of it.
    if !dir.is_empty()
        && base.join("__init__.py").is_file()
        && !root.join(parent(dir)).join("__init__.py").is_file()
    {
        return Some(unit(leaf.to_string(), "python_package"));
    }
    if !dir.is_empty()
        && std::fs::read_dir(&base).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry.path().extension().is_some_and(|ext| ext == "tf") && entry.path().is_file()
            })
        })
    {
        return Some(unit(dir.to_string(), "terraform_module"));
    }
    None
}

/// The Go unit `dir` declares: the package of a directory holding a `.go`
/// file other than a test, under the nearest `go.mod`, named by its import
/// path (module path, then the directory below the `go.mod`); or, for a
/// `go.mod` directory with no such file, the module. A directory holding
/// only tests declares nothing, so its tests count for the enclosing unit.
fn go_unit(root: &Path, dir: &str) -> Option<Unit> {
    let has_package = std::fs::read_dir(root.join(dir)).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".go") && !name.ends_with("_test.go") && entry.path().is_file()
        })
    });
    let mut module_dir = dir;
    loop {
        if let Ok(text) = std::fs::read_to_string(root.join(module_dir).join("go.mod")) {
            let (module, requires) = parse_go_mod(&text)?;
            if module_dir != dir && !has_package {
                return None;
            }
            let below = dir[module_dir.len()..].trim_start_matches('/');
            return Some(Unit {
                name: if below.is_empty() {
                    module.clone()
                } else {
                    format!("{module}/{below}")
                },
                kind: if has_package {
                    "go_package"
                } else {
                    "go_module"
                },
                path: dir.to_string(),
                depends_on: requires,
                go_module: Some(module),
            });
        }
        if module_dir.is_empty() || !has_package {
            return None;
        }
        module_dir = parent(module_dir);
    }
}

/// A `go.mod`'s module path and the module paths it requires, from the
/// one-line and the block form of `require`.
fn parse_go_mod(text: &str) -> Option<(String, Vec<String>)> {
    let unquote = |path: &str| path.trim_matches(|c| c == '"' || c == '`').to_string();
    let mut module = None;
    let mut requires = Vec::new();
    let mut in_require = false;
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if in_require {
            if line.starts_with(')') {
                in_require = false;
            } else if let Some(path) = line.split_whitespace().next() {
                requires.push(unquote(path));
            }
            continue;
        }
        let mut words = line.split_whitespace();
        match words.next() {
            Some("module") => module = words.next().map(unquote),
            Some("require") => match words.next() {
                Some("(") => in_require = true,
                Some(path) => requires.push(unquote(path)),
                None => {}
            },
            Some("require(") => in_require = true,
            _ => {}
        }
    }
    Some((module?, requires))
}

/// The map: units with their sizes, the dependencies between them, and,
/// when `with_surface`, each unit's most used symbols.
pub(crate) fn unit_map(
    root: &Path,
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    with_surface: bool,
) -> (Vec<Value>, Vec<Value>) {
    // Code files only: Markdown and other artifacts are linked to code, not
    // part of a unit.
    let production: HashMap<&str, &GraphNode> = nodes
        .iter()
        .filter(|node| is_production_code(node, &node.file_path))
        .map(|node| (node.qualified_name.as_str(), node))
        .collect();
    let mut files: Vec<&str> = production
        .values()
        .map(|node| node.file_path.as_str())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    files.sort_unstable();
    let index = UnitIndex::discover(root, files.iter().copied());
    let unit_of_qn = |qn: &str| -> Option<usize> {
        match production.get(qn) {
            Some(node) => index.unit_of(&node.file_path),
            // `IMPORTS_FROM` targets a file path.
            None => index.unit_of(qn),
        }
    };
    // An import of a package of another Go module of this repository keeps
    // its import path as the target; it is a declared dependency when the
    // importing package's `go.mod` requires that module.
    let go_packages: HashMap<&str, usize> = index
        .units
        .iter()
        .enumerate()
        .filter(|(_, unit)| unit.kind == "go_package")
        .map(|(unit, entry)| (entry.name.as_str(), unit))
        .collect();
    let required_go_package = |from: usize, edge: &GraphEdge| -> Option<usize> {
        if edge.kind != "IMPORTS_FROM" {
            return None;
        }
        let to = *go_packages.get(edge.target_qualified.as_str())?;
        let module = index.units[to].go_module.as_ref()?;
        (index.units[from].go_module.as_ref() != Some(module)
            && index.units[from].depends_on.contains(module))
        .then_some(to)
    };
    let mut declared: HashSet<(usize, usize)> = HashSet::new();
    let count = index.units.len();
    let mut file_counts = vec![0usize; count];
    for file in &files {
        if let Some(unit) = index.unit_of(file) {
            file_counts[unit] += 1;
        }
    }
    let mut symbols = vec![0usize; count];
    for node in production.values() {
        if SYMBOL_KINDS.contains(&node.kind.as_str())
            && let Some(unit) = index.unit_of(&node.file_path)
        {
            symbols[unit] += 1;
        }
    }
    let tests_by_qn: HashSet<&str> = nodes
        .iter()
        .filter(|node| is_test_node(node, &node.file_path))
        .map(|node| node.qualified_name.as_str())
        .collect();
    let mut tests: Vec<HashSet<&str>> = vec![HashSet::new(); count];
    // (from, to) -> kind -> count
    let mut weights: BTreeMap<(usize, usize), BTreeMap<&'static str, usize>> = BTreeMap::new();
    // unit -> target -> distinct sources in other units
    let mut used: Vec<HashMap<&str, HashSet<&str>>> = vec![HashMap::new(); count];
    for edge in edges {
        if edge.kind == "TESTED_BY" {
            if tests_by_qn.contains(edge.target_qualified.as_str())
                && let Some(unit) = unit_of_qn(&edge.source_qualified)
            {
                tests[unit].insert(edge.target_qualified.as_str());
            }
            continue;
        }
        let Some(kind) = edge_class(edge) else {
            continue;
        };
        let Some(from) = unit_of_qn(&edge.source_qualified) else {
            continue;
        };
        let to = match unit_of_qn(&edge.target_qualified) {
            Some(to) => to,
            None => match required_go_package(from, edge) {
                Some(to) => {
                    declared.insert((from, to));
                    to
                }
                None => continue,
            },
        };
        if from == to {
            continue;
        }
        *weights
            .entry((from, to))
            .or_default()
            .entry(kind)
            .or_default() += 1;
        if production.contains_key(edge.target_qualified.as_str()) {
            used[to]
                .entry(edge.target_qualified.as_str())
                .or_default()
                .insert(edge.source_qualified.as_str());
        }
    }
    // Dependencies the manifests declare between units of this repository;
    // a macro or generated call the graph cannot see still shows up.
    let mut by_package: HashMap<(&str, &str), usize> = HashMap::new();
    for (unit, entry) in index.units.iter().enumerate() {
        by_package.insert((entry.kind, entry.name.as_str()), unit);
    }
    for (from, entry) in index.units.iter().enumerate() {
        if entry.go_module.is_some() {
            // A `require` names a module, not one of its packages.
            continue;
        }
        for dependency in &entry.depends_on {
            if let Some(&to) = by_package.get(&(entry.kind, dependency.as_str()))
                && to != from
            {
                declared.insert((from, to));
                weights.entry((from, to)).or_default();
            }
        }
    }
    let labels = index.labels();
    let label = |unit: usize| -> String { labels[unit].clone() };
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|a, b| {
        symbols[*b]
            .cmp(&symbols[*a])
            .then_with(|| index.units[*a].path.cmp(&index.units[*b].path))
    });
    let units = order
        .iter()
        .map(|&unit| {
            let entry = &index.units[unit];
            let mut out = json!({
                "name": label(unit),
                "kind": entry.kind,
                "path": entry.path,
                "files": file_counts[unit],
                "symbols": symbols[unit],
                "tests": tests[unit].len(),
            });
            if with_surface {
                let mut ranked: Vec<(&str, usize)> = used[unit]
                    .iter()
                    .map(|(qn, sources)| (*qn, sources.len()))
                    .collect();
                ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
                out["surface"] = json!(
                    ranked
                        .into_iter()
                        .take(SURFACE_SIZE)
                        .map(|(qn, users)| json!({"qualified_name": qn, "used_by": users}))
                        .collect::<Vec<_>>()
                );
            }
            out
        })
        .collect();
    let mut unit_edges: Vec<(usize, Value)> = weights
        .into_iter()
        .map(|((from, to), kinds)| {
            let total: usize = kinds.values().sum();
            let mut out = json!({
                "from": label(from),
                "to": label(to),
            });
            for (kind, value) in kinds {
                out[kind] = json!(value);
            }
            if declared.contains(&(from, to)) {
                out["declared"] = json!(true);
            }
            (total, out)
        })
        .collect();
    unit_edges.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            (a.1["from"].as_str(), a.1["to"].as_str())
                .cmp(&(b.1["from"].as_str(), b.1["to"].as_str()))
        })
    });
    (
        units,
        unit_edges.into_iter().map(|(_, edge)| edge).collect(),
    )
}

/// The map's name for an edge between units, or `None` for edges that are
/// not dependencies (containment, tests, documentation links).
fn edge_class(edge: &GraphEdge) -> Option<&'static str> {
    match edge.kind.as_str() {
        "CALLS" => Some("calls"),
        "IMPORTS_FROM" => Some("imports"),
        "REFERENCES" => Some("references"),
        "INHERITS" | "IMPLEMENTS" => Some("inherits"),
        "CROSS_ARTIFACT"
            if is_reportable_bridge(edge)
                && edge.extra.get("bridge_kind").and_then(Value::as_str)
                    != Some("documentation") =>
        {
            Some("bridges")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("dagayn-units-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn units_come_from_manifests_and_import_packages() {
        let root = scratch("discover");
        write(
            &root,
            "crates/core/Cargo.toml",
            "[package]\nname = \"core-lib\"\n",
        );
        write(&root, "crates/core/src/lib.rs", "");
        write(&root, "crates/core/src/util/mod.rs", "");
        write(
            &root,
            "crates/cli/Cargo.toml",
            "[package]\nname = \"cli\"\n[dependencies]\ncore = { package = \"core-lib\", path = \"../core\" }\nserde = \"1\"\n[dev-dependencies]\ntempfile = \"3\"\n",
        );
        write(&root, "crates/cli/src/main.rs", "");
        write(&root, "web/package.json", "{\"name\": \"@acme/web\"}");
        write(&root, "web/src/app.ts", "");
        write(
            &root,
            "svc/go.mod",
            "module example.com/svc\n\ngo 1.22\n\nrequire example.com/lib v0.1.0\n\nrequire (\n\texample.com/other v1.0.0 // indirect\n)\n",
        );
        write(&root, "svc/main.go", "");
        write(&root, "svc/internal/store/store.go", "");
        write(&root, "svc/internal/store/store_test.go", "");
        write(&root, "svc/internal/e2e/e2e_test.go", "");
        write(&root, "svc/internal/testdata/data.go", "");
        write(&root, "mods/go.mod", "module \"example.com/mods\"\n");
        write(&root, "mods/api/api.go", "");
        write(&root, "pkg/__init__.py", "");
        write(&root, "pkg/sub/__init__.py", "");
        write(&root, "pkg/sub/mod.py", "");
        write(&root, "infra/net/main.tf", "");
        write(
            &root,
            "tests/fixtures/demo/Cargo.toml",
            "[package]\nname = \"demo\"\n",
        );
        write(&root, "tests/fixtures/demo/src/lib.rs", "");
        write(&root, "scripts/run.py", "");
        write(&root, "setup.py", "");
        let files = [
            "crates/core/src/lib.rs",
            "crates/core/src/util/mod.rs",
            "crates/cli/src/main.rs",
            "web/src/app.ts",
            "svc/main.go",
            "svc/internal/store/store.go",
            "svc/internal/store/store_test.go",
            "svc/internal/e2e/e2e_test.go",
            "svc/internal/testdata/data.go",
            "mods/api/api.go",
            "pkg/sub/mod.py",
            "infra/net/main.tf",
            "tests/fixtures/demo/src/lib.rs",
            "scripts/run.py",
            "setup.py",
        ];
        let index = UnitIndex::discover(&root, files);
        let unit = |file: &str| {
            let unit = &index.units[index.unit_of(file).unwrap()];
            (unit.name.as_str(), unit.kind, unit.path.as_str())
        };
        assert_eq!(
            unit("crates/core/src/lib.rs"),
            ("core-lib", "cargo_crate", "crates/core")
        );
        assert_eq!(
            unit("crates/core/src/util/mod.rs"),
            ("core-lib", "cargo_crate", "crates/core")
        );
        let cli = &index.units[index.unit_of("crates/cli/src/main.rs").unwrap()];
        assert_eq!(
            cli.depends_on,
            vec!["core-lib".to_string(), "serde".to_string()]
        );
        assert_eq!(unit("web/src/app.ts"), ("@acme/web", "npm_package", "web"));
        assert_eq!(
            unit("svc/main.go"),
            ("example.com/svc", "go_package", "svc")
        );
        let store = &index.units[index.unit_of("svc/internal/store/store.go").unwrap()];
        assert_eq!(
            (store.name.as_str(), store.kind, store.path.as_str()),
            (
                "example.com/svc/internal/store",
                "go_package",
                "svc/internal/store"
            )
        );
        assert_eq!(store.go_module.as_deref(), Some("example.com/svc"));
        assert_eq!(
            store.depends_on,
            vec![
                "example.com/lib".to_string(),
                "example.com/other".to_string()
            ]
        );
        assert_eq!(
            unit("svc/internal/store/store_test.go"),
            (
                "example.com/svc/internal/store",
                "go_package",
                "svc/internal/store"
            )
        );
        // A directory of tests only, or of test data, is no package of its
        // own.
        assert_eq!(
            unit("svc/internal/e2e/e2e_test.go"),
            ("example.com/svc", "go_package", "svc")
        );
        assert_eq!(
            unit("svc/internal/testdata/data.go"),
            ("example.com/svc", "go_package", "svc")
        );
        assert_eq!(
            unit("mods/api/api.go"),
            ("example.com/mods/api", "go_package", "mods/api")
        );
        assert!(
            !index
                .units
                .iter()
                .any(|unit| unit.name == "example.com/mods"),
            "a go.mod directory that holds no indexed file is no unit"
        );
        assert_eq!(unit("pkg/sub/mod.py"), ("pkg", "python_package", "pkg"));
        assert_eq!(
            unit("infra/net/main.tf"),
            ("infra/net", "terraform_module", "infra/net")
        );
        // A manifest under test data is not a unit of this repository.
        assert_eq!(
            unit("tests/fixtures/demo/src/lib.rs"),
            ("tests", "directory", "tests")
        );
        assert_eq!(unit("scripts/run.py"), ("scripts", "directory", "scripts"));
        assert_eq!(unit("setup.py"), ("<root>", "directory", ""));
        let _ = std::fs::remove_dir_all(root);
    }
}
