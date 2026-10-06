//! `review_tool(mode="changes")` findings: what a reviewer must check before
//! merging that the diff itself does not show (docs/plans/REVIEW-TOOL-TARGET.md).
//!
//! Each finding is one checkable claim with a `kind`, the place to look
//! (`qualified_name` and/or `file`), the graph facts behind it (`evidence`),
//! and an `action`. A change that needs nothing beyond the diff yields none.
//!
//! `dangling_reference` and `unchanged_caller` read the base side of the
//! change ([`crate::base_symbols`]); the other finders read the current graph.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use dagayn_graph::{GraphEdge, GraphNode, GraphStore, is_reportable_bridge};
use serde_json::{Map, Value, json};

use crate::base_symbols::{self, Reference, SymbolDelta};
use crate::changes::Analysis;
use crate::query::cross_artifact_role;

/// Symbols a grouped finding lists by name; the rest are counted.
const MAX_LISTED: usize = 10;
/// Caller hops `untested_change` walks looking for a test.
const CALLER_TEST_DEPTH: usize = 2;

/// Doc-to-code roles that state a contract (`implemented_by`), and the
/// code-to-doc ones (`implements_contract`, ...).
const CONTRACT_ROLES_FROM_DOC: &[&str] = &["implemented_by"];
const CONTRACT_ROLES_TO_DOC: &[&str] = &["implements_contract", "has_runbook", "explained_by"];
/// Bridge roles every change of the source side would trip without saying
/// anything: a manifest naming its default source file.
const IMPLIED_BRIDGE_ROLES: &[&str] = &["builds_from_source"];

/// Languages whose changed functions count as production code.
const CODE_LANGUAGES: &[&str] = &[
    "python",
    "rust",
    "typescript",
    "tsx",
    "javascript",
    "go",
    "java",
    "kotlin",
    "scala",
    "c",
    "cpp",
    "csharp",
    "objc",
    "swift",
    "ruby",
    "php",
    "perl",
    "lua",
    "dart",
    "elixir",
    "julia",
    "r",
    "zig",
    "gdscript",
    "bash",
    "vue",
];

/// What the finders read.
pub(crate) struct Inputs<'a> {
    pub store: &'a GraphStore,
    pub root: &'a Path,
    /// Repo-relative changed files.
    pub changed_files: &'a [String],
    /// Nodes whose code the change actually alters (not only comments or
    /// layout).
    pub changed_nodes: &'a [GraphNode],
}

impl Inputs<'_> {
    fn relative(&self, file_path: &str) -> String {
        let path = Path::new(file_path);
        path.strip_prefix(self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }

    fn changed_file_set(&self) -> HashSet<&str> {
        self.changed_files.iter().map(String::as_str).collect()
    }
}

/// Findings kept per kind; [`ChangeFindings::omitted`] counts the rest.
pub(crate) const MAX_FINDINGS_PER_KIND: usize = 10;

/// Every finder's output for one change set.
pub(crate) struct ChangeFindings {
    /// Most actionable kind first, at most [`MAX_FINDINGS_PER_KIND`] each.
    pub findings: Vec<Value>,
    /// Kind -> findings left out by the cap.
    pub omitted: Map<String, Value>,
    /// Kind -> all findings of that kind, in finder order, kinds with none
    /// left out.
    pub counts: Vec<(String, usize)>,
    pub delta: SymbolDelta,
}

/// Runs every finder over a change set: the base-side delta and its
/// references, then the graph finders over the changed functions whose code
/// (not only comments or layout) changed. `changed_files` are repo-relative.
pub(crate) fn change_findings(
    store: &GraphStore,
    root: &Path,
    analysis: &Analysis,
    changed_files: &[String],
    base: &str,
) -> Option<ChangeFindings> {
    let delta = base_symbols::symbol_delta(root, base, changed_files);
    let references = base_symbols::references_to(store, &delta.reference_targets(), changed_files);
    let mut nodes = Vec::new();
    for function in analysis
        .get("changed_functions")
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(qn) = function["qualified_name"].as_str()
            && !delta.unchanged_bodies.contains(qn)
            && let Some(node) = store.get_node(qn).ok()?
        {
            nodes.push(node);
        }
    }
    let inputs = Inputs {
        store,
        root,
        changed_files,
        changed_nodes: &nodes,
    };
    let kinds = [
        dangling_references(&delta, &references),
        unchanged_callers(&delta, &references),
        contract_docs(&inputs)?,
        bridges(&inputs)?,
        untested_changes(&inputs)?,
        tests_to_run(&inputs)?,
    ];
    let mut found = ChangeFindings {
        findings: Vec::new(),
        omitted: Map::new(),
        counts: Vec::new(),
        delta: SymbolDelta::default(),
    };
    for mut list in kinds {
        let Some(kind) = list
            .first()
            .and_then(|f| f["kind"].as_str())
            .map(str::to_string)
        else {
            continue;
        };
        found.counts.push((kind.clone(), list.len()));
        if list.len() > MAX_FINDINGS_PER_KIND {
            found
                .omitted
                .insert(kind, json!(list.len() - MAX_FINDINGS_PER_KIND));
            list.truncate(MAX_FINDINGS_PER_KIND);
        }
        found.findings.extend(list);
    }
    found.delta = delta;
    Some(found)
}

/// A node that is a test, by flag, kind, or where it lives.
pub(crate) fn is_test_node(node: &GraphNode, rel: &str) -> bool {
    node.is_test || node.kind == "Test" || is_test_path(rel)
}

fn is_test_path(rel: &str) -> bool {
    let rel = rel.replace('\\', "/");
    let name = rel.rsplit('/').next().unwrap_or(&rel);
    rel.split('/')
        .any(|part| matches!(part, "tests" | "test" | "__tests__" | "spec" | "testdata"))
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name == "conftest.py"
}

/// Code whose lack of tests is worth saying: not tests, build scripts,
/// examples, benches, fixtures, or generated files.
pub(crate) fn is_production_code(node: &GraphNode, rel: &str) -> bool {
    if is_test_node(node, rel) || !CODE_LANGUAGES.contains(&node.language.as_str()) {
        return false;
    }
    let rel = rel.replace('\\', "/");
    let name = rel.rsplit('/').next().unwrap_or(&rel);
    !(name == "build.rs"
        || name == "setup.py"
        || name == "conftest.py"
        || name.contains("_pb2")
        || name.contains(".generated.")
        || rel.split('/').any(|part| {
            matches!(
                part,
                "examples" | "example" | "benches" | "bench" | "fixtures" | "generated" | "vendor"
            )
        }))
}

/// Direct tests of `qn` (TESTED_BY edges), as graph nodes.
fn direct_tests(store: &GraphStore, qn: &str) -> Option<Vec<GraphNode>> {
    let rows = store.get_transitive_tests(qn, 0).ok()?;
    let mut tests = Vec::new();
    for row in rows {
        if let Some(test_qn) = row["qualified_name"].as_str()
            && let Some(node) = store.get_node(test_qn).ok()?
        {
            tests.push(node);
        }
    }
    Some(tests)
}

/// `tests_to_run`: the direct tests of changed production code, and changed
/// tests themselves, one finding per test file with one command for them.
pub(crate) fn tests_to_run(inputs: &Inputs) -> Option<Vec<Value>> {
    let mut covers: BTreeMap<String, (GraphNode, Vec<String>)> = BTreeMap::new();
    for node in inputs.changed_nodes {
        let rel = inputs.relative(&node.file_path);
        if node.kind == "File" {
            continue;
        }
        if is_test_node(node, &rel) {
            // A helper in a test file is not a test to run.
            if node.is_test || node.kind == "Test" {
                covers
                    .entry(node.qualified_name.clone())
                    .or_insert_with(|| (node.clone(), Vec::new()));
            }
            continue;
        }
        for test in direct_tests(inputs.store, &node.qualified_name)? {
            covers
                .entry(test.qualified_name.clone())
                .or_insert_with(|| (test.clone(), Vec::new()))
                .1
                .push(node.qualified_name.clone());
        }
    }
    // One group per test file; Rust unit tests (in `src/`) one per crate,
    // since one `cargo test -p` runs them all.
    let mut by_file: BTreeMap<String, (String, Vec<CoveringTest>)> = BTreeMap::new();
    for (test, covered) in covers.into_values() {
        let rel = inputs.relative(&test.file_path);
        let key = rust_unit_crate_dir(inputs.root, &test, &rel).unwrap_or_else(|| rel.clone());
        by_file
            .entry(key)
            .or_insert_with(|| (rel, Vec::new()))
            .1
            .push((test, covered));
    }
    let mut findings = Vec::new();
    for (rel, (first_file, tests)) in by_file {
        let nodes: Vec<&GraphNode> = tests.iter().map(|(test, _)| test).collect();
        let command = group_command(inputs.root, &nodes, &first_file);
        let mut covered: Vec<&str> = tests
            .iter()
            .flat_map(|(_, covered)| covered.iter().map(String::as_str))
            .collect();
        covered.sort_unstable();
        covered.dedup();
        let claim = if covered.is_empty() {
            format!("{} test(s) here changed.", tests.len())
        } else {
            format!(
                "{} test(s) here directly test {} changed function(s).",
                tests.len(),
                covered.len()
            )
        };
        findings.push(json!({
            "kind": "tests_to_run",
            "file": rel,
            "claim": claim,
            "command": command,
            "targets": listed(nodes.iter().map(|test| test.qualified_name.as_str())),
            "test_count": tests.len(),
            "covers": listed(covered.iter().copied()),
            "action": "Run these tests.",
        }));
    }
    Some(findings)
}

/// A test and the changed functions it directly tests.
type CoveringTest = (GraphNode, Vec<String>);

/// The crate directory of a Rust unit test (one not under the crate's
/// `tests/`), which `cargo test -p` runs with the rest of the crate's.
fn rust_unit_crate_dir(root: &Path, test: &GraphNode, rel: &str) -> Option<String> {
    if test.language != "rust" {
        return None;
    }
    let (_, crate_dir) = cargo_package(root, rel)?;
    let in_crate = Path::new(rel).strip_prefix(&crate_dir).ok()?;
    (!in_crate.starts_with("tests")).then_some(crate_dir)
}

/// The first [`MAX_LISTED`] names.
fn listed<'a>(names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    names.take(MAX_LISTED).collect()
}

/// `untested_change`: changed production functions with no direct test and
/// no test among their callers up to [`CALLER_TEST_DEPTH`] hops, one finding
/// per file.
pub(crate) fn untested_changes(inputs: &Inputs) -> Option<Vec<Value>> {
    let mut by_file: BTreeMap<String, Vec<&GraphNode>> = BTreeMap::new();
    for node in inputs.changed_nodes {
        let rel = inputs.relative(&node.file_path);
        if node.kind != "Function" || !is_production_code(node, &rel) {
            continue;
        }
        if reached_by_test(inputs, &node.qualified_name)? {
            continue;
        }
        by_file.entry(rel).or_default().push(node);
    }
    Some(
        by_file
            .into_iter()
            .map(|(rel, nodes)| {
                let single = (nodes.len() == 1).then(|| nodes[0].qualified_name.clone());
                json!({
                    "kind": "untested_change",
                    "qualified_name": single,
                    "file": rel,
                    "line": nodes[0].line_start,
                    "claim": format!(
                        "{} changed function(s) here have no test reaching them, directly or through their callers.",
                        nodes.len()
                    ),
                    "targets": listed(nodes.iter().map(|node| node.qualified_name.as_str())),
                    "function_count": nodes.len(),
                    "evidence": [{"edge": "TESTED_BY", "count": 0}, {"edge": "CALLS", "caller_depth": CALLER_TEST_DEPTH, "tests_found": 0}],
                    "action": "Add a test, or confirm an existing one covers them.",
                })
            })
            .collect(),
    )
}

/// Whether a test calls `qn` directly, through TESTED_BY, or through
/// callers up to [`CALLER_TEST_DEPTH`] hops.
fn reached_by_test(inputs: &Inputs, qn: &str) -> Option<bool> {
    let mut frontier = vec![qn.to_string()];
    let mut seen: HashSet<String> = frontier.iter().cloned().collect();
    for depth in 0..=CALLER_TEST_DEPTH {
        for current in &frontier {
            if !direct_tests(inputs.store, current)?.is_empty() {
                return Some(true);
            }
        }
        if depth == CALLER_TEST_DEPTH {
            break;
        }
        let (_, incoming) = inputs.store.get_edges_by_endpoints(&frontier).ok()?;
        let mut next = Vec::new();
        for edge in incoming.values().flatten() {
            if edge.kind != "CALLS" || !seen.insert(edge.source_qualified.clone()) {
                continue;
            }
            if let Some(caller) = inputs.store.get_node(&edge.source_qualified).ok()? {
                if is_test_node(&caller, &inputs.relative(&caller.file_path)) {
                    return Some(true);
                }
                next.push(caller.qualified_name);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Some(false)
}

/// `contract_doc_not_updated`: authored contract docs linked to changed
/// code, where the doc is not part of the change.
pub(crate) fn contract_docs(inputs: &Inputs) -> Option<Vec<Value>> {
    let changed = inputs.changed_file_set();
    let qns: Vec<String> = inputs
        .changed_nodes
        .iter()
        .filter(|node| node.language != "markdown")
        .map(|node| node.qualified_name.clone())
        .collect();
    let (outgoing, incoming) = inputs.store.get_edges_by_endpoints(&qns).ok()?;
    let mut docs: BTreeMap<String, (String, i64, Vec<Value>)> = BTreeMap::new();
    let mut note = |doc_qn: &str, edge: &GraphEdge, code_qn: &str, role: &str| -> Option<()> {
        let doc = inputs.store.get_node(doc_qn).ok()?;
        let (file, line) = match &doc {
            Some(doc) => (inputs.relative(&doc.file_path), doc.line_start),
            None => (doc_qn.split("::").next().unwrap_or(doc_qn).to_string(), 0),
        };
        if changed.contains(file.as_str()) {
            return Some(());
        }
        docs.entry(doc_qn.to_string())
            .or_insert_with(|| (file, line, Vec::new()))
            .2
            .push(json!({"edge": edge.kind, "relationship_role": role, "code": code_qn, "confidence_tier": edge.confidence_tier.as_str()}));
        Some(())
    };
    for qn in &qns {
        for edge in incoming.get(qn).into_iter().flatten() {
            if let Some(role) =
                cross_artifact_role(edge).filter(|r| CONTRACT_ROLES_FROM_DOC.contains(r))
            {
                note(&edge.source_qualified, edge, qn, role)?;
            }
        }
        for edge in outgoing.get(qn).into_iter().flatten() {
            if let Some(role) =
                cross_artifact_role(edge).filter(|r| CONTRACT_ROLES_TO_DOC.contains(r))
            {
                note(&edge.target_qualified, edge, qn, role)?;
            }
        }
    }
    Some(
        docs.into_iter()
            .map(|(doc_qn, (file, line, evidence))| {
                json!({
                    "kind": "contract_doc_not_updated",
                    "qualified_name": doc_qn,
                    "file": file,
                    "line": line,
                    "claim": "An authored contract doc is linked to changed code and was not edited in this change.",
                    "evidence": evidence,
                    "action": "Read the section; update it or confirm the contract still holds.",
                })
            })
            .collect(),
    )
}

/// `bridge_touched`: the change edits the source side of a reportable
/// cross-artifact bridge (manifest, Terraform, FFI, build config) and not the
/// target side.
pub(crate) fn bridges(inputs: &Inputs) -> Option<Vec<Value>> {
    let changed = inputs.changed_file_set();
    let mut seeds: Vec<String> = inputs
        .changed_nodes
        .iter()
        .map(|node| node.qualified_name.clone())
        .collect();
    // File-level nodes carry manifest bridges (`pyproject.toml`). A code
    // file's own file-level bridge (an import of a native module) would fire
    // on any edit to it, so code files seed from their changed functions only.
    for file in inputs.changed_files {
        let is_code = inputs
            .store
            .get_node(file)
            .ok()?
            .is_some_and(|node| CODE_LANGUAGES.contains(&node.language.as_str()));
        if !is_code {
            seeds.push(file.clone());
        }
    }
    seeds.sort();
    seeds.dedup();
    let (outgoing, _) = inputs.store.get_edges_by_endpoints(&seeds).ok()?;
    // One finding per target file: a manifest naming both a file and a
    // symbol in it is one place to check.
    let mut targets: BTreeMap<String, (Vec<String>, Vec<Value>)> = BTreeMap::new();
    for edge in outgoing.values().flatten() {
        // Tests use a bridge; they do not define its contract.
        if edge.kind != "CROSS_ARTIFACT"
            || !is_reportable_bridge(edge)
            || is_test_path(&edge.file_path)
        {
            continue;
        }
        let role = cross_artifact_role(edge).unwrap_or("");
        if IMPLIED_BRIDGE_ROLES.contains(&role)
            || CONTRACT_ROLES_TO_DOC.contains(&role)
            || CONTRACT_ROLES_FROM_DOC.contains(&role)
        {
            continue;
        }
        let target = &edge.target_qualified;
        // A target that is neither a node nor a file (a `.wasm` suffix in a
        // string) names nothing to check.
        let file = match inputs.store.get_node(target).ok()? {
            Some(node) => inputs.relative(&node.file_path),
            None => {
                let file = target.split("::").next().unwrap_or(target).to_string();
                if !inputs.root.join(&file).is_file() {
                    continue;
                }
                file
            }
        };
        if changed.contains(file.as_str()) {
            continue;
        }
        let entry = targets.entry(file).or_default();
        if !entry.0.contains(target) {
            entry.0.push(target.clone());
        }
        entry.1.push(json!({
            "edge": "CROSS_ARTIFACT",
            "relationship_role": role,
            "source": edge.source_qualified,
            "file": edge.file_path,
            "line": edge.line,
            "confidence_tier": edge.confidence_tier.as_str(),
        }));
    }
    Some(
        targets
            .into_iter()
            .map(|(file, (mut symbols, evidence))| {
                symbols.sort();
                // The symbol when the bridge names one, else the file.
                let target = symbols
                    .iter()
                    .find(|symbol| symbol.contains("::"))
                    .cloned()
                    .unwrap_or_else(|| file.clone());
                json!({
                    "kind": "bridge_touched",
                    "qualified_name": target,
                    "file": file,
                    "targets": symbols,
                    "claim": "The change edits one side of a cross-artifact bridge; the other side is unchanged.",
                    "evidence": evidence,
                    "action": "Check the other side still matches.",
                })
            })
            .collect(),
    )
}

/// A shell command that runs `test`, when its language has a known runner.
fn test_command(root: &Path, test: &GraphNode, rel: &str) -> Value {
    let qn = &test.qualified_name;
    let local = qn.split_once("::").map_or(qn.as_str(), |(_, rest)| rest);
    match test.language.as_str() {
        "python" => json!(format!("pytest {rel}::{}", local.replace('.', "::"))),
        "rust" => {
            let Some((crate_name, crate_dir)) = cargo_package(root, rel) else {
                return Value::Null;
            };
            let filter = local.replace('.', "::");
            let in_crate = Path::new(rel)
                .strip_prefix(&crate_dir)
                .unwrap_or(Path::new(rel))
                .to_string_lossy()
                .replace('\\', "/");
            match in_crate
                .strip_prefix("tests/")
                .and_then(|rest| rest.strip_suffix(".rs"))
            {
                Some(target) if !target.contains('/') => {
                    json!(format!(
                        "cargo test -p {crate_name} --test {target} {filter}"
                    ))
                }
                _ => json!(format!("cargo test -p {crate_name} {filter}")),
            }
        }
        "typescript" | "tsx" | "javascript" => {
            let name = test_title(&test.name);
            match js_runner(root, rel) {
                Some(runner) => json!(format!("npx {runner} {rel} -t {}", shell_quote(&name))),
                None => json!(format!("npm test -- {rel}")),
            }
        }
        "go" => {
            let dir = Path::new(rel)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dir = if dir.is_empty() { ".".to_string() } else { dir };
            json!(format!("go test ./{dir} -run '^{}$'", test.name))
        }
        _ => Value::Null,
    }
}

/// One command for `tests`, all in `rel`: the test's own command when
/// there is one test, else the narrowest command that runs them together.
fn group_command(root: &Path, tests: &[&GraphNode], rel: &str) -> Value {
    if let [test] = tests {
        return test_command(root, test, rel);
    }
    let local = |test: &GraphNode| -> String {
        let qn = &test.qualified_name;
        qn.split_once("::")
            .map_or(qn.as_str(), |(_, rest)| rest)
            .replace('.', "::")
    };
    match tests[0].language.as_str() {
        "python" if tests.len() <= 5 => json!(format!(
            "pytest {}",
            tests
                .iter()
                .map(|test| format!("{rel}::{}", local(test)))
                .collect::<Vec<_>>()
                .join(" ")
        )),
        "python" => json!(format!("pytest {rel}")),
        "rust" => {
            let Some((crate_name, crate_dir)) = cargo_package(root, rel) else {
                return Value::Null;
            };
            let in_crate = Path::new(rel)
                .strip_prefix(&crate_dir)
                .unwrap_or(Path::new(rel))
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(target) = in_crate
                .strip_prefix("tests/")
                .and_then(|rest| rest.strip_suffix(".rs"))
                .filter(|target| !target.contains('/'))
            {
                return json!(format!("cargo test -p {crate_name} --test {target}"));
            }
            let filters: Vec<String> = tests.iter().map(|test| local(test)).collect();
            let prefix = common_module_prefix(&filters);
            if prefix.is_empty() {
                json!(format!("cargo test -p {crate_name}"))
            } else {
                json!(format!("cargo test -p {crate_name} {prefix}"))
            }
        }
        "typescript" | "tsx" | "javascript" => match js_runner(root, rel) {
            Some(runner) => json!(format!("npx {runner} {rel}")),
            None => json!(format!("npm test -- {rel}")),
        },
        "go" => {
            let dir = Path::new(rel)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|dir| !dir.is_empty())
                .unwrap_or_else(|| ".".to_string());
            let names: Vec<&str> = tests.iter().map(|test| test.name.as_str()).collect();
            json!(format!("go test ./{dir} -run '^({})$'", names.join("|")))
        }
        _ => Value::Null,
    }
}

/// The longest `a::b::` module path every filter starts with.
fn common_module_prefix(filters: &[String]) -> String {
    let Some(first) = filters.first() else {
        return String::new();
    };
    let mut parts: Vec<&str> = first.split("::").collect();
    parts.pop();
    for filter in &filters[1..] {
        let other: Vec<&str> = filter.split("::").collect();
        let shared = parts.iter().zip(&other).take_while(|(a, b)| a == b).count();
        parts.truncate(shared.min(other.len().saturating_sub(1)));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{}::", parts.join("::"))
    }
}

/// `test:greet@L3` -> `greet`; other names unchanged.
fn test_title(name: &str) -> String {
    let name = name.strip_prefix("test:").unwrap_or(name);
    match name.rsplit_once("@L") {
        Some((title, line)) if line.chars().all(|c| c.is_ascii_digit()) => title.to_string(),
        _ => name.to_string(),
    }
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The nearest `Cargo.toml` with a `[package]` name above `rel`, and the
/// crate's repo-relative directory.
fn cargo_package(root: &Path, rel: &str) -> Option<(String, String)> {
    let mut dir = Path::new(rel).parent();
    while let Some(current) = dir {
        let manifest = root.join(current).join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest)
            && let Some(name) = package_name(&text)
        {
            return Some((name, current.to_string_lossy().into_owned()));
        }
        dir = current.parent();
    }
    None
}

fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some(value) = line.strip_prefix("name")
            && let Some(value) = value.trim_start().strip_prefix('=')
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// `vitest` or `jest`, from the nearest `package.json` that names one.
fn js_runner(root: &Path, rel: &str) -> Option<&'static str> {
    let mut dir = Path::new(rel).parent();
    loop {
        let base = dir.map_or_else(|| root.to_path_buf(), |d| root.join(d));
        if let Ok(text) = std::fs::read_to_string(base.join("package.json")) {
            if text.contains("\"vitest\"") {
                return Some("vitest run");
            }
            if text.contains("\"jest\"") {
                return Some("jest");
            }
        }
        dir = dir?.parent();
    }
}

/// `dangling_reference`: one finding per removed symbol, file, or moved
/// file that something outside the change still points at.
pub(crate) fn dangling_references(delta: &SymbolDelta, references: &[Reference]) -> Vec<Value> {
    let targets = delta
        .removed
        .iter()
        .chain(&delta.removed_files)
        .chain(&delta.moved_files);
    let mut findings = Vec::new();
    for symbol in targets {
        let sites = sites_for(references, &symbol.qualified_name, |_| true);
        if sites.is_empty() {
            continue;
        }
        let renamed_to = delta
            .renamed_candidates
            .iter()
            .find(|candidate| candidate.from.qualified_name == symbol.qualified_name)
            .map(|candidate| candidate.to_qualified_name.clone());
        let claim = match (&symbol.current_file, symbol.kind.as_str()) {
            (Some(now), "File") => format!(
                "{} moved to {now}; references to the old path remain.",
                symbol.file_path
            ),
            (None, "File") => format!("{} was deleted; references to it remain.", symbol.file_path),
            _ => match &renamed_to {
                Some(to) => format!(
                    "{} was renamed to {to}; references to the old name remain.",
                    symbol.name
                ),
                None => format!("{} was removed; references to it remain.", symbol.name),
            },
        };
        findings.push(json!({
            "kind": "dangling_reference",
            "qualified_name": symbol.qualified_name,
            "file": symbol.file_path,
            "line": symbol.line_start,
            "claim": claim,
            "renamed_to": renamed_to,
            "sites": sites,
            "action": "Update or remove each referencing site.",
        }));
    }
    findings
}

/// `unchanged_caller`: a function whose parameters changed in a way its
/// callers cannot absorb (a new required parameter, or fewer parameters),
/// with callers outside the change.
pub(crate) fn unchanged_callers(delta: &SymbolDelta, references: &[Reference]) -> Vec<Value> {
    let mut findings = Vec::new();
    for change in &delta.signature_changed {
        if !change.params_changed {
            continue;
        }
        let before = param_shape(change.before.params.as_deref().unwrap_or(""));
        let after = param_shape(change.after.params.as_deref().unwrap_or(""));
        if after.required <= before.required && after.total >= before.total {
            continue;
        }
        let sites = sites_for(references, &change.symbol.qualified_name, |reference| {
            matches!(
                reference.edge_kind.as_str(),
                "CALLS" | "INHERITS" | "IMPLEMENTS"
            )
        });
        if sites.is_empty() {
            continue;
        }
        let symbol = &change.symbol;
        findings.push(json!({
            "kind": "unchanged_caller",
            "qualified_name": symbol.current_qualified_name.as_deref().unwrap_or(&symbol.qualified_name),
            "file": symbol.current_file.as_deref().unwrap_or(&symbol.file_path),
            "line": change.line_start,
            "claim": format!(
                "{} now takes {} ({} required, was {}); callers outside the change were not edited.",
                symbol.name,
                change.after.params.as_deref().unwrap_or("()"),
                after.required,
                before.required,
            ),
            "before": change.before.params,
            "after": change.after.params,
            "sites": sites,
            "action": "Check each caller still passes what the new signature needs.",
        }));
    }
    findings
}

fn sites_for(
    references: &[Reference],
    target: &str,
    keep: impl Fn(&Reference) -> bool,
) -> Vec<Value> {
    references
        .iter()
        .filter(|reference| reference.target == target && keep(reference))
        .map(|reference| {
            json!({
                "qualified_name": reference.source_qualified,
                "file": reference.file_path,
                "line": reference.line,
                "edge": reference.edge_kind,
                "matched_by": reference.matched_by.as_str(),
                "confidence": reference.matched_by.confidence(),
            })
        })
        .collect()
}

/// Parameter counts of a declaration's parameter list.
#[derive(Debug, PartialEq)]
struct ParamShape {
    total: usize,
    required: usize,
}

/// Counts the top-level parameters of `(a, b=1, *args)`-style text; a
/// parameter with a default, an optional marker, or a variadic form is not
/// required, and a receiver (`self`, `&self`, `cls`, `this`) is not counted.
fn param_shape(params: &str) -> ParamShape {
    let inner = params.trim();
    let inner = inner.strip_prefix('(').unwrap_or(inner);
    let inner = inner.strip_suffix(')').unwrap_or(inner);
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (at, c) in inner.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&inner[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    let mut shape = ParamShape {
        total: 0,
        required: 0,
    };
    for part in parts {
        let part = part.trim();
        let name = part.split(':').next().unwrap_or(part).trim();
        if part.is_empty()
            || matches!(
                name,
                "self" | "&self" | "&mut self" | "mut self" | "cls" | "this" | "/" | "*"
            )
        {
            continue;
        }
        shape.total += 1;
        let optional = part.contains('=')
            || name.ends_with('?')
            || name.starts_with('*')
            || name.starts_with("...")
            || part.ends_with("...");
        if !optional {
            shape.required += 1;
        }
    }
    shape
}

/// `text` with comments, whitespace, and trailing commas before a closing
/// bracket removed, so two spans compare equal when only comments or layout
/// differ. The same treatment on both sides keeps a `#` inside a string from
/// mattering.
pub(crate) fn normalize_code(text: &str, language: &str) -> String {
    let line_comment: &[&str] = match language {
        "python" | "ruby" | "perl" | "r" | "julia" | "bash" | "elixir" | "gdscript" | "toml"
        | "terraform" => &["#"],
        "lua" => &["--"],
        _ => &["//"],
    };
    let block = !matches!(
        language,
        "python" | "ruby" | "perl" | "r" | "bash" | "elixir" | "gdscript" | "lua"
    );
    let mut stripped = String::with_capacity(text.len());
    let mut in_block = false;
    for line in text.lines() {
        let mut rest = line;
        let mut kept = String::new();
        loop {
            if in_block {
                match rest.find("*/") {
                    Some(end) => {
                        rest = &rest[end + 2..];
                        in_block = false;
                    }
                    None => break,
                }
            }
            let line_at = line_comment.iter().filter_map(|m| rest.find(m)).min();
            let block_at = if block { rest.find("/*") } else { None };
            match (line_at, block_at) {
                (Some(l), Some(b)) if b < l => {
                    kept.push_str(&rest[..b]);
                    rest = &rest[b + 2..];
                    in_block = true;
                }
                (None, Some(b)) => {
                    kept.push_str(&rest[..b]);
                    rest = &rest[b + 2..];
                    in_block = true;
                }
                (Some(l), _) => {
                    kept.push_str(&rest[..l]);
                    break;
                }
                (None, None) => {
                    kept.push_str(rest);
                    break;
                }
            }
        }
        stripped.push_str(&kept);
    }
    let mut compact: String = stripped.chars().filter(|c| !c.is_whitespace()).collect();
    for (from, to) in [(",)", ")"), (",]", "]"), (",}", "}")] {
        while compact.contains(from) {
            compact = compact.replace(from, to);
        }
    }
    compact
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_layout_do_not_count_as_changes() {
        let before = "def f(a, b):\n    return a + b\n";
        let after = "def f(\n    a,\n    b,\n):\n    # add them\n    return a + b  # sum\n";
        assert_eq!(
            normalize_code(before, "python"),
            normalize_code(after, "python")
        );
        let rust_before = "fn add(a: i32) -> i32 {\n    a + 1\n}\n";
        let rust_after = "/// Adds one.\nfn add(a: i32) -> i32 {\n    /* inc */ a + 1 // done\n}\n";
        assert_eq!(
            normalize_code(rust_before, "rust"),
            normalize_code(rust_after, "rust")
        );
        assert_ne!(
            normalize_code("fn add(a: i32) -> i32 { a + 1 }", "rust"),
            normalize_code("fn add(a: i32) -> i32 { a + 2 }", "rust")
        );
    }

    #[test]
    fn parameter_shapes_count_required_parameters() {
        assert_eq!(
            param_shape("(x)"),
            ParamShape {
                total: 1,
                required: 1
            }
        );
        assert_eq!(
            param_shape("(x, factor=2)"),
            ParamShape {
                total: 2,
                required: 1
            }
        );
        assert_eq!(
            param_shape("(&self, a: HashMap<String, i32>, b: i32)"),
            ParamShape {
                total: 2,
                required: 2
            }
        );
        assert_eq!(
            param_shape("(name: string, greeting?: string, ...rest: string[])"),
            ParamShape {
                total: 3,
                required: 1
            }
        );
        assert_eq!(
            param_shape("()"),
            ParamShape {
                total: 0,
                required: 0
            }
        );
        assert_eq!(
            param_shape("(self, *args, **kwargs)"),
            ParamShape {
                total: 2,
                required: 0
            }
        );
    }

    #[test]
    fn test_paths_are_recognised() {
        assert!(is_test_path("tests/test_util.py"));
        assert!(is_test_path("web/greet.test.ts"));
        assert!(is_test_path("pkg/util_test.go"));
        assert!(!is_test_path("src/testing_utils.rs"));
        assert!(!is_test_path("pkg/app.py"));
    }

    #[test]
    fn module_prefixes_are_shared_paths() {
        let filters = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            common_module_prefix(&filters(&["core_tests::a", "core_tests::b"])),
            "core_tests::"
        );
        assert_eq!(common_module_prefix(&filters(&["a::x", "b::y"])), "");
        assert_eq!(common_module_prefix(&filters(&["solo"])), "");
    }

    #[test]
    fn titles_drop_the_test_prefix_and_line() {
        assert_eq!(test_title("test:greet@L3"), "greet");
        assert_eq!(test_title("test_helper"), "test_helper");
    }

    #[test]
    fn package_names_come_from_the_package_table() {
        let manifest =
            "[workspace]\nname = \"no\"\n\n[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
        assert_eq!(package_name(manifest).as_deref(), Some("demo"));
        assert_eq!(package_name("[workspace]\nmembers = []\n"), None);
    }
}
