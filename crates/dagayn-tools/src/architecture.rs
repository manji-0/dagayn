//! `dagayn.architecture` and `dagayn.sap`: the dependency graph between
//! scopes and its ADP, SDP, and SAP measures, for any granularity, artifact
//! scope, and dependency profile.

use std::collections::{HashMap, HashSet};

use dagayn_graph::{GraphEdge, GraphNode, GraphStore, is_reportable_bridge};
use serde_json::{Value, json};

use crate::answerability::round4;

const DOC_SUFFIXES: &[&str] = &[".md", ".markdown", ".mdown", ".mkdn"];
const MAX_ADP_CYCLES: usize = 5000;
/// DFS steps after which ADP enumeration leaves the answer to Python.
const MAX_ADP_STEPS: usize = 2_000_000;

/// `ArtifactScope`.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Artifact {
    Code,
    Docs,
    All,
}

impl Artifact {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "code" => Self::Code,
            "docs" => Self::Docs,
            "all" => Self::All,
            _ => return None,
        })
    }

    /// `node_matches_artifact_scope`.
    pub(crate) fn includes(self, node: &GraphNode) -> bool {
        match self {
            Self::All => true,
            Self::Docs => is_documentation_node(node),
            Self::Code => !is_documentation_node(node),
        }
    }
}

/// `DependencyProfile` and its edge kinds.
#[derive(Clone, Copy)]
pub(crate) enum Profile {
    StrictStatic,
    Implementation,
    InfraDataflow,
    ArtifactTrace,
}

impl Profile {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "strict_static" => Self::StrictStatic,
            "implementation" => Self::Implementation,
            "infra_dataflow" => Self::InfraDataflow,
            "artifact_trace" => Self::ArtifactTrace,
            _ => return None,
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::StrictStatic => "strict_static",
            Self::Implementation => "implementation",
            Self::InfraDataflow => "infra_dataflow",
            Self::ArtifactTrace => "artifact_trace",
        }
    }

    /// `edge_matches_dependency_profile`.
    fn matches(self, edge: &GraphEdge) -> bool {
        let extra = match self {
            Self::StrictStatic => None,
            Self::Implementation => Some("CALLS"),
            Self::InfraDataflow => Some("REFERENCES"),
            Self::ArtifactTrace => Some("CROSS_ARTIFACT"),
        };
        let kind = edge.kind.as_str();
        if matches!(
            kind,
            "IMPORTS_FROM" | "DEPENDS_ON" | "INHERITS" | "IMPLEMENTS"
        ) {
            return true;
        }
        if Some(kind) != extra {
            return false;
        }
        kind != "CROSS_ARTIFACT" || is_reportable_bridge(edge)
    }
}

/// How nodes map to scopes and which edges count.
pub(crate) struct View {
    /// `file` keeps each file its own scope; anything else is the package.
    pub file_scopes: bool,
    pub artifact: Artifact,
    pub profile: Profile,
}

impl View {
    /// `review_helpers`' fixed view: packages, code, `strict_static`.
    pub(crate) fn review() -> Self {
        Self {
            file_scopes: false,
            artifact: Artifact::Code,
            profile: Profile::StrictStatic,
        }
    }

    /// `node_file_to_scope_key` for a non-empty path.
    fn scope_of(&self, file_path: &str) -> String {
        if self.file_scopes {
            file_path.to_string()
        } else {
            file_to_package(file_path)
        }
    }
}

/// Python truthiness of a JSON value.
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `float(value or default)`.
pub(crate) fn float_or(value: &Value, default: f64) -> f64 {
    if truthy(value) {
        value.as_f64().unwrap_or(default)
    } else {
        default
    }
}

pub(crate) fn str_of(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

/// `PurePosixPath(path)`'s components (`.` and empty ones dropped).
pub(crate) fn posix_parts(path: &str) -> (bool, Vec<&str>) {
    let absolute = path.starts_with('/');
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    (absolute, parts)
}

/// `Path(name).suffix`.
pub(crate) fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[index..],
        _ => "",
    }
}

pub(crate) fn file_name(path: &str) -> &str {
    posix_parts(path).1.last().copied().unwrap_or("")
}

/// `file_to_package`.
pub(crate) fn file_to_package(file_path: &str) -> String {
    let (absolute, parts) = posix_parts(file_path);
    let parent = &parts[..parts.len().saturating_sub(1)];
    match (absolute, parent.is_empty()) {
        (true, _) => format!("/{}", parent.join("/")),
        (false, true) => "<root>".to_string(),
        (false, false) => parent.join("/"),
    }
}

/// `scope_key_for_file` (package scope).
pub(crate) fn scope_key_for_file(file_path: &str) -> Option<String> {
    (!file_path.is_empty()).then(|| file_to_package(file_path))
}

/// `is_documentation_node`.
pub(crate) fn is_documentation_node(node: &GraphNode) -> bool {
    node.language.to_lowercase() == "markdown"
        || DOC_SUFFIXES.contains(&suffix(file_name(&node.file_path)).to_lowercase().as_str())
}

/// The nodes and edges `build_graph_snapshot` reads once.
pub(crate) struct Snapshot {
    pub all_nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl Snapshot {
    pub(crate) fn read(store: &GraphStore) -> Option<Self> {
        Some(Self {
            all_nodes: store.get_all_nodes_filtered(false).ok()?,
            edges: store.get_all_edges().ok()?,
        })
    }

    /// `build_node_scope_maps(store, scope_kind, artifact_scope)`.
    fn scope_maps(&self, view: &View) -> (HashMap<&str, String>, HashMap<&str, String>) {
        let mut qualified: HashMap<&str, String> = HashMap::new();
        let mut names: HashMap<&str, HashSet<String>> = HashMap::new();
        for node in &self.all_nodes {
            if !view.artifact.includes(node) || node.file_path.is_empty() {
                continue;
            }
            let scope = view.scope_of(&node.file_path);
            qualified.insert(&node.qualified_name, scope.clone());
            names.entry(&node.name).or_default().insert(scope);
        }
        let unique = names
            .into_iter()
            .filter(|(_, scopes)| scopes.len() == 1)
            .filter_map(|(name, scopes)| scopes.into_iter().next().map(|scope| (name, scope)))
            .collect();
        (qualified, unique)
    }

    /// Each dependency edge of the profile, as source and target scopes.
    pub(crate) fn dependencies(&self, view: &View) -> Vec<(String, String)> {
        let (qualified, names) = self.scope_maps(view);
        let mut out = Vec::new();
        for edge in &self.edges {
            if !view.profile.matches(edge) {
                continue;
            }
            let Some(source) = qualified.get(edge.source_qualified.as_str()) else {
                continue;
            };
            let target = qualified
                .get(edge.target_qualified.as_str())
                .or_else(|| names.get(edge.target_qualified.as_str()));
            if let Some(target) = target
                && target != source
            {
                out.push((source.clone(), target.clone()));
            }
        }
        out
    }
}

/// `_project_dependency_graph`: a networkx `DiGraph` in insertion order.
pub(crate) struct ScopeGraph {
    nodes: Vec<String>,
    index: HashMap<String, usize>,
    /// Successors in insertion order, with the aggregated weight.
    successors: Vec<Vec<(usize, i64)>>,
    predecessors: Vec<HashSet<usize>>,
}

impl ScopeGraph {
    pub(crate) fn new(dependencies: &[(String, String)]) -> Self {
        let mut graph = Self {
            nodes: Vec::new(),
            index: HashMap::new(),
            successors: Vec::new(),
            predecessors: Vec::new(),
        };
        for (source, target) in dependencies {
            let from = graph.add_node(source);
            let to = graph.add_node(target);
            match graph.successors[from].iter_mut().find(|(n, _)| *n == to) {
                Some(slot) => slot.1 += 1,
                None => {
                    graph.successors[from].push((to, 1));
                    graph.predecessors[to].insert(from);
                }
            }
        }
        graph
    }

    fn add_node(&mut self, name: &str) -> usize {
        if let Some(index) = self.index.get(name) {
            return *index;
        }
        let index = self.nodes.len();
        self.nodes.push(name.to_string());
        self.index.insert(name.to_string(), index);
        self.successors.push(Vec::new());
        self.predecessors.push(HashSet::new());
        index
    }

    fn instability(&self, node: usize) -> f64 {
        let ca = self.predecessors[node].len();
        let ce = self.successors[node].len();
        let total = ca + ce;
        if total > 0 {
            ce as f64 / total as f64
        } else {
            0.0
        }
    }

    /// `compute_sdp_metrics`, sorted by instability descending (stable).
    /// `g.number_of_nodes() == 0`.
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub(crate) fn sdp_metrics(&self) -> Vec<(String, i64, i64, f64)> {
        let mut metrics: Vec<(String, i64, i64, f64)> = (0..self.nodes.len())
            .map(|node| {
                (
                    self.nodes[node].clone(),
                    self.predecessors[node].len() as i64,
                    self.successors[node].len() as i64,
                    round4(self.instability(node)),
                )
            })
            .collect();
        metrics.sort_by(|left, right| {
            right
                .3
                .partial_cmp(&left.3)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        metrics
    }

    /// `find_sdp_violations(min_delta)`.
    pub(crate) fn sdp_violations(&self, min_delta: f64, profile: Profile) -> Vec<Value> {
        let mut violations: Vec<(f64, Value)> = Vec::new();
        for (source, successors) in self.successors.iter().enumerate() {
            for (target, _) in successors {
                let (i_src, i_tgt) = (self.instability(source), self.instability(*target));
                let delta = i_tgt - i_src;
                if delta > min_delta {
                    violations.push((
                        round4(delta),
                        json!({
                            "source": self.nodes[source],
                            "target": self.nodes[*target],
                            "source_instability": round4(i_src),
                            "target_instability": round4(i_tgt),
                            "delta": round4(delta),
                            "dependency_profile": profile.name(),
                        }),
                    ));
                }
            }
        }
        violations.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        violations.into_iter().map(|(_, value)| value).collect()
    }

    /// `find_adp_violations`: every simple cycle up to ten nodes, or `None`
    /// past Python's 5000-cycle limit.
    pub(crate) fn adp_violations(
        &self,
        min_size: i64,
        max_length: i64,
        profile: Profile,
    ) -> Option<Vec<Value>> {
        let mut cycles: Vec<Vec<usize>> = Vec::new();
        let mut steps = 0_usize;
        for start in 0..self.nodes.len() {
            // Each cycle once: from its smallest index, through larger ones.
            let mut path = vec![start];
            let mut on_path = vec![false; self.nodes.len()];
            on_path[start] = true;
            let mut stack: Vec<usize> = vec![0];
            while let Some(position) = stack.last_mut() {
                steps += 1;
                if steps > MAX_ADP_STEPS {
                    return None;
                }
                let node = *path.last()?;
                let successors = &self.successors[node];
                if *position >= successors.len() {
                    stack.pop();
                    let left = path.pop()?;
                    on_path[left] = false;
                    continue;
                }
                let (next, _) = successors[*position];
                *position += 1;
                if next == start {
                    if max_length < 1 || (path.len() as i64) > max_length {
                        continue;
                    }
                    if (path.len() as i64) < min_size {
                        continue;
                    }
                    cycles.push(path.clone());
                    if cycles.len() > MAX_ADP_CYCLES {
                        return None;
                    }
                } else if next > start && !on_path[next] && (path.len() as i64) < max_length {
                    path.push(next);
                    on_path[next] = true;
                    stack.push(0);
                }
            }
        }
        let weight = |from: usize, to: usize| {
            self.successors[from]
                .iter()
                .find(|(n, _)| *n == to)
                .map_or(0, |(_, w)| *w)
        };
        let mut violations: Vec<(i64, Vec<String>, Value)> = cycles
            .into_iter()
            .map(|cycle| {
                let names: Vec<&String> = cycle.iter().map(|n| &self.nodes[*n]).collect();
                let start = (0..names.len()).min_by_key(|i| names[*i]).unwrap_or(0);
                let rotated: Vec<usize> = cycle[start..]
                    .iter()
                    .chain(&cycle[..start])
                    .copied()
                    .collect();
                let edge_weight: i64 = (0..rotated.len())
                    .map(|i| weight(rotated[i], rotated[(i + 1) % rotated.len()]))
                    .sum();
                let severity = rotated.len() as i64 * edge_weight;
                let nodes: Vec<String> = rotated.iter().map(|n| self.nodes[*n].clone()).collect();
                let value = json!({
                    "nodes": nodes,
                    "length": rotated.len(),
                    "edge_weight": edge_weight,
                    "severity": severity,
                    "dependency_profile": profile.name(),
                });
                (severity, nodes, value)
            })
            .collect();
        violations.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        Some(violations.into_iter().map(|(_, _, value)| value).collect())
    }
}

/// `compute_sap_metrics(scope_kind, unit_filter, artifact_scope,
/// dependency_profile)`; `scope_kind` is the caller's spelling.
pub(crate) fn sap_metrics(
    snapshot: &Snapshot,
    view: &View,
    scope_kind: &str,
    unit_filter: Option<&[String]>,
) -> Vec<Value> {
    let (qualified, names) = snapshot.scope_maps(view);
    let mut na: HashMap<String, i64> = HashMap::new();
    let mut nt: HashMap<String, i64> = HashMap::new();
    let mut members: HashMap<String, i64> = HashMap::new();
    for node in &snapshot.all_nodes {
        let Some(scope) = qualified.get(node.qualified_name.as_str()) else {
            continue;
        };
        *members.entry(scope.clone()).or_default() += 1;
        if node.kind == "Class" && node.parent_name.is_none() {
            let extra = &node.extra;
            let role = extra
                .get("type_role")
                .and_then(Value::as_str)
                .unwrap_or("class");
            if matches!(
                role,
                "class"
                    | "abstract_class"
                    | "interface"
                    | "protocol"
                    | "trait"
                    | "abstract_type"
                    | "mixin"
            ) {
                *nt.entry(scope.clone()).or_default() += 1;
                let flag = |key: &str| extra.get(key).is_some_and(truthy);
                if flag("is_abstract")
                    || flag("is_contract")
                    || matches!(
                        role,
                        "abstract_class" | "interface" | "protocol" | "trait" | "abstract_type"
                    )
                {
                    *na.entry(scope.clone()).or_default() += 1;
                }
            }
        }
    }
    // `dep_graph`, keyed in first-seen order.
    let mut sources: Vec<String> = Vec::new();
    let mut outgoing: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    let mut scopes: HashSet<String> = members.keys().cloned().collect();
    for edge in &snapshot.edges {
        if !view.profile.matches(edge) {
            continue;
        }
        let Some(source) = qualified.get(edge.source_qualified.as_str()) else {
            continue;
        };
        let target = qualified
            .get(edge.target_qualified.as_str())
            .or_else(|| names.get(edge.target_qualified.as_str()));
        let Some(target) = target else { continue };
        if target == source {
            continue;
        }
        let row = outgoing.entry(source.clone()).or_insert_with(|| {
            sources.push(source.clone());
            Vec::new()
        });
        match row.iter_mut().find(|(t, _)| t == target) {
            Some(slot) => slot.1 += 1,
            None => row.push((target.clone(), 1)),
        }
        scopes.insert(source.clone());
        scopes.insert(target.clone());
    }
    let mut sorted: Vec<String> = scopes.into_iter().collect();
    sorted.sort();
    let mut results: Vec<(f64, String, Value)> = Vec::new();
    for scope in sorted {
        if let Some(prefixes) = unit_filter.filter(|p| !p.is_empty())
            && !prefixes
                .iter()
                .any(|prefix| scope.starts_with(prefix.as_str()))
        {
            continue;
        }
        let nt_count = nt.get(&scope).copied().unwrap_or(0);
        let na_count = na.get(&scope).copied().unwrap_or(0);
        let out = outgoing.get(&scope).cloned().unwrap_or_default();
        let incoming: Vec<(String, i64)> = sources
            .iter()
            .filter_map(|source| {
                outgoing[source]
                    .iter()
                    .find(|(t, _)| *t == scope)
                    .map(|(_, count)| (source.clone(), *count))
            })
            .collect();
        let (ce, ca) = (out.len() as i64, incoming.len() as i64);
        let mut notes = sap_notes(&scope);
        let abstractness = if nt_count > 0 {
            na_count as f64 / nt_count as f64
        } else {
            0.0
        };
        if nt_count == 0 {
            notes.push("no-eligible-types");
        }
        let total = ca + ce;
        let instability = if total > 0 {
            ce as f64 / total as f64
        } else {
            0.0
        };
        if total == 0 {
            notes.push("isolated");
        }
        let distance = (abstractness + instability - 1.0).abs();
        let (applicable, reason) = if nt_count == 0 {
            (false, "no-eligible-types")
        } else if total == 0 {
            (false, "isolated")
        } else {
            (true, "applicable")
        };
        let top = |items: &[(String, i64)]| {
            let mut items = items.to_vec();
            items.sort_by_key(|item| std::cmp::Reverse(item.1));
            items
                .into_iter()
                .take(5)
                .map(|(scope, count)| json!({"scope": scope, "count": count}))
                .collect::<Vec<_>>()
        };
        let mut entry = json!({
            "scope_kind": scope_kind,
            "scope_key": scope,
            "display_name": scope,
            "na": na_count,
            "nt": nt_count,
            "ca": ca,
            "ce": ce,
            "abstractness": round4(abstractness),
            "instability": round4(instability),
            "distance": round4(distance),
            "sap_applicable": applicable,
            "applicability_reason": reason,
            "dependency_profile": view.profile.name(),
            "member_count": members.get(&scope).copied().unwrap_or(0),
            "top_incoming_dependencies": top(&incoming),
            "top_outgoing_dependencies": top(&out),
        });
        if !notes.is_empty() {
            entry["notes"] = json!(notes);
        }
        results.push((round4(distance), scope, entry));
    }
    results.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.1.cmp(&right.1))
    });
    results.into_iter().map(|(_, _, entry)| entry).collect()
}

fn sap_notes(scope: &str) -> Vec<&'static str> {
    let normalized = scope.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    let mut notes = Vec::new();
    if parts
        .first()
        .is_some_and(|first| matches!(*first, "tests" | "test" | "__tests__"))
    {
        notes.push("test-scope");
    }
    if parts.contains(&"fixtures") {
        notes.push("fixture-scope");
    }
    notes
}

/// `find_sap_violations(min_distance)` over `compute_sap_metrics`' rows.
pub(crate) fn sap_violations(metrics: &[Value], min_distance: f64) -> Vec<Value> {
    let mut violations: Vec<Value> = metrics
        .iter()
        .filter(|metric| {
            let notes = metric.get("notes").and_then(Value::as_array);
            let has = |note: &str| notes.is_some_and(|n| n.iter().any(|x| x == note));
            metric["distance"].as_f64().unwrap_or(0.0) > min_distance
                && metric["sap_applicable"].as_bool().unwrap_or(false)
                && !has("test-scope")
                && !has("fixture-scope")
        })
        .cloned()
        .collect();
    violations.sort_by(|left, right| {
        right["distance"]
            .as_f64()
            .partial_cmp(&left["distance"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    violations
}
