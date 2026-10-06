//! `dagayn.refactor.dead_code.find_dead_code`: functions and classes with no
//! callers, tests, importers, references, or subclasses that are not entry
//! points, framework hooks, structural types, or value containers.

use std::collections::{HashMap, HashSet};

use dagayn_graph::{
    GraphEdge, GraphNode, GraphStore, has_framework_decorator, is_conventional_entry_point,
    is_reportable_bridge,
};
use serde_json::{Value, json};

use crate::coverage::splitlines;
use crate::query::{cross_artifact_role, sanitize};

const FRAMEWORK_BASE_CLASSES: &[&str] = &[
    "Base",
    "DeclarativeBase",
    "Model",
    "BaseModel",
    "BaseSettings",
    "db.Model",
    "TableBase",
    "Stack",
    "NestedStack",
    "Construct",
    "Resource",
];
const CDK_CLASS_SUFFIXES: &[&str] = &["Stack", "Construct", "Pipeline", "Resources", "Layer"];
const STRUCTURAL_ROLES: &[&str] = &[
    "interface",
    "trait",
    "abstract_class",
    "abstract_type",
    "implementation",
];
const VALUE_CONTAINER_ROLES: &[&str] = &["struct", "enum", "record"];
const SCOPE_CONTAINER_ROLES: &[&str] = &["object", "namespace", "ambient_module"];
const CONFIG_LANGUAGES: &[&str] = &["terraform", "hcl", "json", "yaml", "toml"];

/// Test files and test data: fixtures are samples written to be parsed,
/// not called.
fn test_file_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"([\\/]__tests__[\\/]|\.(spec|test|cy)\.[cm]?[jt]sx?$|[\\/]test_[^/\\]*\.py$|[\\/]e2e[_-]?tests?[\\/]|[\\/]test[_-]utils?[\\/]|(^|[\\/])(fixtures|testdata)[\\/])",
        )
        .expect("regex")
    })
}

fn type_ident_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"[A-Z][A-Za-z0-9_]*").expect("regex"))
}

fn extra_str<'a>(node_extra: &'a Value, key: &str) -> Option<&'a str> {
    node_extra.get(key).and_then(Value::as_str)
}

fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(crate::architecture::truthy)
}

/// `_path_segments`.
fn path_segments(file_path: &str) -> Vec<String> {
    let normalized = file_path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    parts[..parts.len().saturating_sub(1)]
        .iter()
        .filter(|p| p.chars().count() >= 4 && !matches!(**p, "home" | "src" | "lib" | "app"))
        .map(|p| p.to_string())
        .collect()
}

/// The decorators when they are a list (`isinstance(.., (list, tuple))`).
fn decorator_list(node: &GraphNode) -> Option<Vec<&str>> {
    match node.extra.get("decorators") {
        Some(Value::Array(items)) if !items.is_empty() => {
            Some(items.iter().filter_map(Value::as_str).collect())
        }
        _ => None,
    }
}

fn has_value_container_metadata(extra: &Value) -> bool {
    if extra_str(extra, "container_role") == Some("data_container") {
        return true;
    }
    if extra.get("value_semantics") == Some(&Value::Bool(true)) {
        return true;
    }
    if extra_str(extra, "type_role").is_some_and(|r| VALUE_CONTAINER_ROLES.contains(&r)) {
        return true;
    }
    matches!(extra.get("derive_traits"), Some(Value::Array(traits))
        if traits.iter().any(|t| matches!(t.as_str(), Some("Serialize" | "Deserialize"))))
}

fn is_structural_type(node: &GraphNode) -> bool {
    if node.kind != "Class" {
        return false;
    }
    if extra_str(&node.extra, "type_role").is_some_and(|r| STRUCTURAL_ROLES.contains(&r)) {
        return true;
    }
    if truthy(node.extra.get("is_contract")) || truthy(node.extra.get("is_abstract")) {
        return true;
    }
    node.language == "python"
        && (node.name.ends_with("Protocol")
            || node.file_path.replace('\\', "/").ends_with("/_protocol.py"))
}

/// `_survives_dead_code_node_filters`.
fn survives(
    node: &GraphNode,
    type_refs: &HashSet<String>,
    class_bases: &HashMap<String, Vec<String>>,
) -> bool {
    if CONFIG_LANGUAGES.contains(&node.language.as_str()) || node.language == "markdown" {
        return false;
    }
    if node.is_test || test_file_re().is_match(&node.file_path) {
        return false;
    }
    if node.language == "rust"
        && node
            .parent_name
            .as_deref()
            .is_some_and(|p| p.split("::").any(|s| s == "tests"))
    {
        return false;
    }
    if [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .any(|s| node.file_path.ends_with(s))
    {
        return false;
    }
    if truthy(node.extra.get("ambient")) {
        return false;
    }
    if node.name.starts_with("__") && node.name.ends_with("__") {
        return false;
    }
    if node.name == "constructor" && node.parent_name.as_deref().is_some_and(|p| !p.is_empty()) {
        return false;
    }
    if is_conventional_entry_point(node) {
        return false;
    }
    if node.kind == "Class" && (type_refs.contains(&node.name) || has_framework_decorator(node)) {
        return false;
    }
    if is_structural_type(node) {
        return false;
    }
    if node.kind == "Class"
        && extra_str(&node.extra, "type_role").is_some_and(|r| SCOPE_CONTAINER_ROLES.contains(&r))
    {
        return false;
    }
    if node.kind == "Class" && has_value_container_metadata(&node.extra) {
        return false;
    }
    let parent = node.parent_name.as_deref().filter(|p| !p.is_empty());
    let check_qn = if node.kind == "Class" {
        Some(node.qualified_name.as_str())
    } else if parent.is_some() {
        Some(
            node.qualified_name
                .rsplit_once('.')
                .map_or(node.qualified_name.as_str(), |(head, _)| head),
        )
    } else {
        None
    };
    let framework_class = check_qn
        .and_then(|qn| class_bases.get(qn))
        .is_some_and(|bases| {
            bases
                .iter()
                .any(|b| FRAMEWORK_BASE_CLASSES.contains(&b.as_str()))
        });
    if node.kind == "Class"
        && (framework_class || CDK_CLASS_SUFFIXES.iter().any(|s| node.name.ends_with(s)))
    {
        return false;
    }
    if node.kind == "Function" && framework_class {
        return false;
    }
    if node.kind == "Function"
        && parent.is_some_and(|p| CDK_CLASS_SUFFIXES.iter().any(|s| p.ends_with(s)))
    {
        return false;
    }
    if let Some(decorators) = decorator_list(node) {
        if matches!(node.kind.as_str(), "Function" | "Test")
            && decorators.iter().any(|d| {
                matches!(
                    *d,
                    "property" | "abstractmethod" | "classmethod" | "staticmethod"
                ) || d.ends_with(".abstractmethod")
                    || d.starts_with("HostListener")
            })
        {
            return false;
        }
        if node.kind == "Class" && decorators.iter().any(|d| d.contains("dataclass")) {
            return false;
        }
    }
    true
}

/// `_is_plausible_caller`.
fn plausible_caller(
    edge_file: &str,
    node: &GraphNode,
    importer_files: &HashMap<String, HashSet<String>>,
    name_counts: &HashMap<String, i64>,
) -> bool {
    let node_file = node.file_path.as_str();
    if edge_file == node_file {
        return true;
    }
    if !node.name.is_empty() && name_counts.get(&node.name).copied().unwrap_or(0) == 1 {
        return true;
    }
    let path_of = |target: &str| target.split("::").next().unwrap_or(target).to_string();
    let matches = |path: &str| -> bool {
        if let Some(dir) = path.strip_suffix("/__init__.py")
            && node_file.starts_with(&format!("{dir}/"))
        {
            return true;
        }
        path.starts_with(node_file) || node_file.starts_with(&format!("{path}/"))
    };
    for target in importer_files.get(edge_file).into_iter().flatten() {
        if matches(&path_of(target)) {
            return true;
        }
        for second in importer_files.get(target).into_iter().flatten() {
            if matches(&path_of(second)) {
                return true;
            }
        }
        if !target.starts_with('/')
            && path_segments(node_file)
                .iter()
                .any(|seg| target.contains(seg.as_str()))
        {
            return true;
        }
    }
    false
}

/// `_is_public_api_candidate`.
fn public_api_candidate(node: &GraphNode, lines: &[String]) -> bool {
    external_api_candidate(&node.language, node.line_start, lines)
}

/// `_is_source_public_api_candidate` or `_is_bridge_export_candidate` for a
/// symbol of `language` at `line_number`.
pub(crate) fn external_api_candidate(language: &str, line_number: i64, lines: &[String]) -> bool {
    let ls = line_number;
    let line = if ls <= 0 || ls as usize > lines.len() {
        ""
    } else {
        lines[ls as usize - 1].trim()
    };
    if !line.is_empty() {
        const MARKERS: &[&str] = &[
            "pub ",
            "pub(",
            "public ",
            "export ",
            "export default ",
            "export async ",
            "export function ",
            "export class ",
            "export interface ",
            "export const ",
            "export let ",
            "export var ",
        ];
        if MARKERS.iter().any(|m| line.starts_with(m)) {
            return true;
        }
        if matches!(
            language,
            "typescript" | "tsx" | "javascript" | "vue" | "svelte"
        ) {
            return format!(" {line} ").contains(" export ") || line.starts_with("exports.");
        }
    }
    // `_is_bridge_export_candidate`: inside a `#[pymethods]` impl.
    if language != "rust" || ls <= 0 || lines.is_empty() {
        return false;
    }
    enclosing_block(lines, ls, |idx| {
        lines[idx].trim_start().starts_with("impl ")
            && lines[idx.saturating_sub(5)..=idx]
                .join("\n")
                .contains("#[pymethods]")
    })
}

/// Whether an open block whose header line `is_header` accepts encloses
/// line `line_number`, scanning upwards as Python does.
pub(crate) fn enclosing_block(
    lines: &[String],
    line_number: i64,
    is_header: impl Fn(usize) -> bool,
) -> bool {
    let target = ((line_number - 1) as usize).min(lines.len() - 1);
    for idx in (0..=target).rev() {
        if !is_header(idx) {
            continue;
        }
        let depth: i64 = lines[idx..=target]
            .iter()
            .map(|l| l.matches('{').count() as i64 - l.matches('}').count() as i64)
            .sum();
        if depth > 0 {
            return true;
        }
    }
    false
}

/// `_load_source_lines`: strict UTF-8, or nothing.
pub(crate) fn source_lines(store: &GraphStore, file_path: &str) -> Vec<String> {
    store
        .resolve_file_path(file_path)
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| splitlines(&text).into_iter().map(str::to_string).collect())
        .unwrap_or_default()
}

/// `_cross_artifact_symbol_name`.
fn cross_artifact_symbol(edge: &GraphEdge) -> Option<String> {
    if edge.kind != "CROSS_ARTIFACT" {
        return None;
    }
    if let Some(sym) = edge
        .extra
        .get("original_symbol_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(sym.to_string());
    }
    edge.target_qualified
        .strip_prefix("<unresolved:")
        .and_then(|rest| rest.strip_suffix('>'))
        .map(str::to_string)
}

fn is_external(edge: &GraphEdge) -> bool {
    edge.extra.get("external") == Some(&Value::Bool(true))
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

/// `find_dead_code(kind, file_pattern)`.
/// The symbols nothing in the repository uses: the graph's candidates that
/// survive [`crate::dead_code_verify::verify`].
pub(crate) fn find_dead_code(
    store: &GraphStore,
    kind: Option<&str>,
    file_pattern: Option<&str>,
) -> Option<Vec<Value>> {
    Some(dead_code_report(store, kind, file_pattern)?.dead)
}

/// [`find_dead_code`] with what it left out and how the check went.
pub(crate) fn dead_code_report(
    store: &GraphStore,
    kind: Option<&str>,
    file_pattern: Option<&str>,
) -> Option<crate::dead_code_verify::Verified> {
    let (candidates, sources) = graph_candidates(store, kind, file_pattern)?;
    crate::dead_code_verify::verify(store, candidates, &sources)
}

/// The graph's candidates alone, before the check that keeps only symbols
/// nothing in the repository uses: for testing the graph heuristics, never
/// for reporting.
pub(crate) fn graph_candidate_records(
    store: &GraphStore,
    kind: Option<&str>,
    file_pattern: Option<&str>,
) -> Option<Vec<Value>> {
    let (candidates, _) = graph_candidates(store, kind, file_pattern)?;
    Some(candidates.into_iter().map(|(_, record)| record).collect())
}

/// The graph's view: symbols without callers, tests, importers, references,
/// or subclasses, with their records, and the source lines read for them.
#[allow(clippy::type_complexity)]
fn graph_candidates(
    store: &GraphStore,
    kind: Option<&str>,
    file_pattern: Option<&str>,
) -> Option<(Vec<(GraphNode, Value)>, HashMap<String, Vec<String>>)> {
    let kinds = match kind.filter(|k| !k.is_empty()) {
        Some(kind) => vec![kind.to_string()],
        None => strings(&["Function", "Class"]),
    };
    let candidates = store
        .get_nodes_by_kind(&kinds, file_pattern.filter(|p| !p.is_empty()))
        .ok()?;
    let mut type_refs: HashSet<String> = HashSet::new();
    for f in store
        .get_nodes_by_kind(&strings(&["Function", "Test"]), None)
        .ok()?
    {
        for text in [&f.params, &f.return_type].into_iter().flatten() {
            type_refs.extend(
                type_ident_re()
                    .find_iter(text)
                    .map(|m| m.as_str().to_string()),
            );
        }
    }
    let mut class_bases: HashMap<String, Vec<String>> = HashMap::new();
    let mut inherits_targets: HashMap<String, Vec<String>> = HashMap::new();
    for edge in store.get_edges_by_kind("INHERITS", false).ok()? {
        let target = edge.target_qualified.clone();
        let base = target.rsplit("::").next().unwrap_or(&target).to_string();
        class_bases
            .entry(edge.source_qualified.clone())
            .or_default()
            .push(base);
        inherits_targets
            .entry(edge.source_qualified)
            .or_default()
            .push(target);
    }
    let mut importer_files: HashMap<String, HashSet<String>> = HashMap::new();
    for edge in store.get_edges_by_kind("IMPORTS_FROM", false).ok()? {
        importer_files
            .entry(edge.file_path)
            .or_default()
            .insert(edge.target_qualified);
    }
    let name_counts = store
        .count_nodes_by_name(&strings(&["Function", "Class"]), false)
        .ok()?;

    let surviving: Vec<GraphNode> = candidates
        .into_iter()
        .filter(|n| survives(n, &type_refs, &class_bases))
        .collect();
    let mut incoming_qns = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for node in &surviving {
        incoming_qns.push(node.qualified_name.clone());
        if !names.contains(&node.name) {
            names.push(node.name.clone());
        }
        if let Some(parent) = node.parent_name.as_deref().filter(|p| !p.is_empty()) {
            incoming_qns.push(format!("{parent}::{}", node.name));
        }
    }
    let incoming_by_qn = store.get_edges_by_targets(&incoming_qns, &[]).ok()?;
    let tested_by = store
        .get_edges_by_sources(&incoming_qns, &strings(&["TESTED_BY"]))
        .ok()?;
    let mut bare_calls: HashMap<String, Vec<GraphEdge>> = HashMap::new();
    let mut bare_inherits: HashMap<String, Vec<GraphEdge>> = HashMap::new();
    for (name, edges) in store
        .get_edges_by_targets(&names, &strings(&["CALLS", "INHERITS"]))
        .ok()?
    {
        for edge in edges {
            let slot = if edge.kind == "CALLS" {
                &mut bare_calls
            } else {
                &mut bare_inherits
            };
            slot.entry(name.clone()).or_default().push(edge);
        }
    }
    // `_bare_calls_from_tests`.
    let mut sources: Vec<String> = bare_calls
        .values()
        .flatten()
        .map(|e| e.source_qualified.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    sources.sort();
    let tests: HashSet<String> = if sources.is_empty() {
        HashSet::new()
    } else {
        store
            .get_nodes_by_qualified_names(&sources)
            .ok()?
            .into_iter()
            .filter(|(_, n)| n.is_test)
            .map(|(qn, _)| qn)
            .collect()
    };
    let bare_tested: HashMap<String, Vec<GraphEdge>> = bare_calls
        .iter()
        .filter_map(|(name, edges)| {
            let kept: Vec<GraphEdge> = edges
                .iter()
                .filter(|e| {
                    tests.contains(&e.source_qualified)
                        && !is_external(e)
                        && !truthy(e.extra.get("test_api"))
                })
                .cloned()
                .collect();
            (!kept.is_empty()).then(|| (name.clone(), kept))
        })
        .collect();
    let suffix_calls = store
        .get_edges_by_target_names(&names, "CALLS", true)
        .ok()?;
    let mut base_qns: HashSet<String> = HashSet::new();
    for node in &surviving {
        if node.kind == "Function" && node.parent_name.as_deref().is_some_and(|p| !p.is_empty()) {
            let parent_qn = node
                .qualified_name
                .rsplit_once('.')
                .map_or(node.qualified_name.as_str(), |(h, _)| h);
            for base in inherits_targets.get(parent_qn).into_iter().flatten() {
                base_qns.insert(format!("{base}.{}", node.name));
                base_qns.insert(format!("{}::{base}.{}", node.file_path, node.name));
            }
        }
    }
    let base_nodes = if base_qns.is_empty() {
        HashMap::new()
    } else {
        store
            .get_nodes_by_qualified_names(&base_qns.into_iter().collect::<Vec<_>>())
            .ok()?
    };
    let mut unresolved_entrypoints: HashMap<String, Vec<GraphEdge>> = HashMap::new();
    for edge in store.get_edges_by_kind("CROSS_ARTIFACT", true).ok()? {
        if cross_artifact_role(&edge) != Some("maps_entrypoint") {
            continue;
        }
        let Some(sym) = cross_artifact_symbol(&edge) else {
            continue;
        };
        let key = sym.rsplit('.').next().unwrap_or(&sym).to_string();
        unresolved_entrypoints.entry(key).or_default().push(edge);
    }

    let empty: Vec<GraphEdge> = Vec::new();
    let mut cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut dead = Vec::new();
    for node in &surviving {
        let parent = node.parent_name.as_deref().filter(|p| !p.is_empty());
        // A method overriding an abstract base method is reachable through it.
        if node.kind == "Function" && parent.is_some() {
            let parent_qn = node
                .qualified_name
                .rsplit_once('.')
                .map_or(node.qualified_name.as_str(), |(h, _)| h);
            let abstract_base = inherits_targets
                .get(parent_qn)
                .into_iter()
                .flatten()
                .any(|base| {
                    let base_node =
                        base_nodes
                            .get(&format!("{base}.{}", node.name))
                            .or_else(|| {
                                base_nodes.get(&format!("{}::{base}.{}", node.file_path, node.name))
                            });
                    base_node
                        .and_then(decorator_list)
                        .is_some_and(|d| d.iter().any(|x| x.contains("abstractmethod")))
                });
            if abstract_base {
                continue;
            }
        }
        let class_qn = parent.map(|p| format!("{p}::{}", node.name));
        let mut incoming: Vec<&GraphEdge> = incoming_by_qn
            .get(&node.qualified_name)
            .unwrap_or(&empty)
            .iter()
            .collect();
        let calls = |edges: &[&GraphEdge]| edges.iter().any(|e| e.kind == "CALLS");
        if !calls(&incoming)
            && let Some(class_qn) = &class_qn
        {
            incoming.extend(incoming_by_qn.get(class_qn).unwrap_or(&empty));
        }
        if !calls(&incoming) {
            let bare = bare_calls
                .get(&node.name)
                .unwrap_or(&empty)
                .iter()
                .chain(suffix_calls.get(&node.name).unwrap_or(&empty));
            incoming.extend(bare.filter(|e| {
                !is_external(e)
                    && plausible_caller(&e.file_path, node, &importer_files, &name_counts)
            }));
        }
        let mut tested: Vec<&GraphEdge> = tested_by
            .get(&node.qualified_name)
            .unwrap_or(&empty)
            .iter()
            .collect();
        if let Some(class_qn) = &class_qn {
            tested.extend(tested_by.get(class_qn).unwrap_or(&empty));
        }
        if tested.is_empty() {
            tested.extend(
                bare_tested
                    .get(&node.name)
                    .unwrap_or(&empty)
                    .iter()
                    .filter(|e| {
                        plausible_caller(&e.file_path, node, &importer_files, &name_counts)
                    }),
            );
        }
        if node.kind == "Class" && !incoming.iter().any(|e| e.kind == "INHERITS") {
            incoming.extend(bare_inherits.get(&node.name).unwrap_or(&empty));
        }
        let count = |kind: &str| incoming.iter().filter(|e| e.kind == kind).count() as i64;
        let mut has_callers = count("CALLS") > 0;
        let has_tests = !tested.is_empty();
        let has_importers = count("IMPORTS_FROM") > 0;
        let mut has_references = count("REFERENCES") > 0;
        let has_subclasses = count("INHERITS") > 0;
        let mut reference_count = count("REFERENCES");
        let reportable = incoming
            .iter()
            .any(|e| e.kind == "CROSS_ARTIFACT" && is_reportable_bridge(e));
        let entrypoint = unresolved_entrypoints
            .get(&node.name)
            .unwrap_or(&empty)
            .iter()
            .any(|e| {
                e.target_qualified.starts_with("<unresolved:")
                    && cross_artifact_role(e) == Some("maps_entrypoint")
                    && cross_artifact_symbol(e).is_some_and(|sym| {
                        let attr = if sym.contains('.') {
                            sym.rsplit('.').next().unwrap_or("").to_string()
                        } else {
                            sym
                        };
                        attr == node.name
                    })
            });
        if reportable {
            has_references = true;
            reference_count += incoming
                .iter()
                .filter(|e| e.kind == "CROSS_ARTIFACT" && is_reportable_bridge(e))
                .count() as i64;
        }
        let any = |callers: bool| {
            callers || has_tests || has_importers || has_references || has_subclasses || entrypoint
        };
        if node.kind == "Class"
            && !any(has_callers)
            && store
                .count_edges_by_target_name_prefix(&format!("{}.", node.name), "CALLS")
                .ok()?
                > 0
        {
            has_callers = true;
        }
        if any(has_callers) {
            continue;
        }
        // `_has_callers_via_base_method`.
        if node.kind == "Function" && parent.is_some() {
            let suffix = format!(".{}", node.name);
            if let Some(class_qn) = node.qualified_name.strip_suffix(&suffix) {
                for base in class_bases.get(class_qn).into_iter().flatten() {
                    for base_node in store
                        .get_nodes_by_parent_and_name(
                            base,
                            &node.name,
                            &strings(&["Function", "Test"]),
                        )
                        .ok()?
                    {
                        if store
                            .has_edge_to_target(&base_node.qualified_name, "CALLS")
                            .ok()?
                        {
                            has_callers = true;
                        }
                    }
                    if has_callers {
                        break;
                    }
                }
            }
        }
        if any(has_callers) {
            continue;
        }
        let lines = cache
            .entry(node.file_path.clone())
            .or_insert_with(|| source_lines(store, &node.file_path));
        let source_available = !lines.is_empty();
        let public = public_api_candidate(node, lines);
        let definitions = name_counts.get(&node.name).copied().unwrap_or(0);
        let (callers, tests_n, importers, subclasses) = (
            count("CALLS"),
            tested.len() as i64,
            count("IMPORTS_FROM"),
            count("INHERITS"),
        );
        let mut reasons = Vec::new();
        for (zero, code) in [
            (callers == 0, "no_callers"),
            (tests_n == 0, "no_test_references"),
            (importers == 0, "no_importers"),
            (reference_count == 0, "no_references"),
        ] {
            if zero {
                reasons.push(code);
            }
        }
        if node.kind == "Class" && subclasses == 0 {
            reasons.push("no_subclasses");
        }
        if public {
            reasons.push("public_api_candidate");
        }
        if entrypoint {
            reasons.push("reachable_via_cross_artifact");
        }
        if definitions > 1 {
            reasons.push("ambiguous_symbol_name");
        }
        if !source_available {
            reasons.push("source_unavailable");
        }
        let mut caveats = vec![
            "Static analysis can miss runtime dispatch, plugin registration, reflection, and dynamic imports.",
        ];
        if public {
            caveats.push("Public API symbols may be consumed outside the indexed graph; verify downstream users before deleting.");
        }
        if entrypoint {
            caveats.push("An unresolved manifest or Terraform entrypoint bridge references this symbol; verify runtime wiring before deleting.");
        }
        let low = public
            || has_value_container_metadata(&node.extra)
            || entrypoint
            || definitions > 1
            || !source_available;
        dead.push((
            node.clone(),
            json!({
                "name": sanitize(&node.name),
                "qualified_name": sanitize(&node.qualified_name),
                "kind": node.kind,
                "file": node.file_path,
                "line": node.line_start,
                "language": node.language,
                "confidence": if low { "low" } else { "medium" },
                "public_api_candidate": public,
                "reason_codes": reasons,
                "evidence": {
                    "caller_count": callers,
                    "test_ref_count": tests_n,
                    "importer_count": importers,
                    "reference_count": reference_count,
                    "subclass_count": subclasses,
                    "name_definition_count": definitions,
                    "source_available": source_available,
                    "reachable_via_cross_artifact": entrypoint,
                },
                "caveats": caveats,
            }),
        ));
    }
    Some((dead, cache))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_and_test_data_are_test_files() {
        for path in [
            "tests/fixtures/sample_cross_language.py",
            "pkg/testdata/input.go",
            "web/src/__tests__/a.ts",
            "tests/test_util.py",
        ] {
            assert!(test_file_re().is_match(path), "{path}");
        }
        for path in ["dagayn/fixture_loader.py", "src/fixtures.rs", "pkg/app.py"] {
            assert!(!test_file_re().is_match(path), "{path}");
        }
    }
}
