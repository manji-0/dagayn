//! `dagayn.analysis`: hub, bridge, knowledge-gap, and surprising-connection
//! analysis over the artifact-scoped graph.

use std::collections::{HashMap, HashSet};

use dagayn_graph::{
    GraphEdge, GraphNode, GraphStore, is_conventional_entry_point, is_reportable_bridge,
};
use serde_json::{Value, json};

use crate::architecture::{Artifact, posix_parts};
use crate::query::sanitize;
use crate::suggestions::round_to;

/// `items[:limit]`.
pub(crate) fn py_prefix<T: Clone>(items: &[T], limit: i64) -> Vec<T> {
    let len = items.len() as i64;
    let end = if limit < 0 {
        (len + limit).max(0)
    } else {
        limit.min(len)
    };
    items[..end as usize].to_vec()
}

/// What `build_graph_snapshot` reads for these analyses.
pub(crate) struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub communities: HashMap<String, Option<i64>>,
}

impl Graph {
    pub(crate) fn read(store: &GraphStore) -> Option<Self> {
        Some(Self {
            nodes: store.get_all_nodes_filtered(true).ok()?,
            edges: store.get_all_edges().ok()?,
            communities: store.get_all_community_ids().ok()?,
        })
    }
}

/// `_persisted_scope_matches`: the persisted table that covers this scope.
fn persisted_table(artifact: Artifact, include_tests: bool) -> Option<bool> {
    match (artifact, include_tests) {
        (Artifact::All, true) => Some(false),
        (Artifact::Code, false) => Some(true),
        _ => None,
    }
}

/// `find_hub_nodes(top_n, artifact_scope, include_tests)`.
pub(crate) fn find_hubs(
    store: &GraphStore,
    graph: &Graph,
    top_n: i64,
    artifact: Artifact,
    include_tests: bool,
) -> Vec<Value> {
    if let Some(code) = persisted_table(artifact, include_tests) {
        // A missing table is created by Python, which then finds it empty.
        let rows = store.persisted_hub_scores(code, top_n).unwrap_or_default();
        if !rows.is_empty() {
            return rows;
        }
    }
    let (nodes, edges) = scoped(&graph.nodes, &graph.edges, artifact, include_tests);
    computed_hubs(&nodes, &pairs(&edges), &graph.communities, top_n)
}

/// `find_bridge_nodes(top_n, artifact_scope, include_tests)`.
pub(crate) fn find_bridges(
    store: &GraphStore,
    graph: &Graph,
    top_n: i64,
    artifact: Artifact,
    include_tests: bool,
) -> Vec<Value> {
    if let Some(code) = persisted_table(artifact, include_tests) {
        let rows = store
            .persisted_bridge_scores(code, top_n)
            .unwrap_or_default();
        if !rows.is_empty() {
            return rows;
        }
    }
    let (nodes, edges) = scoped(&graph.nodes, &graph.edges, artifact, include_tests);
    let mut first: Vec<GraphNode> = Vec::new();
    let mut seen = HashSet::new();
    for node in nodes {
        if seen.insert(node.qualified_name.clone()) {
            first.push(node);
        }
    }
    computed_bridges(&first, &pairs(&edges), &graph.communities, top_n)
}

/// `_is_analysis_excluded_from_test_gap`.
pub(crate) fn excluded_from_analysis(node: &GraphNode) -> bool {
    if node.is_test || node.kind == "Test" || node.language == "markdown" {
        return true;
    }
    let path = node.file_path.replace('\\', "/");
    let (absolute, parts) = posix_parts(&path);
    let lowered: Vec<String> = parts.iter().map(|p| p.to_lowercase()).collect();
    if lowered
        .iter()
        .any(|p| matches!(p.as_str(), "tests" | "test" | "__tests__"))
    {
        return true;
    }
    let _ = absolute;
    let name = lowered.last().cloned().unwrap_or_default();
    name.starts_with("test_")
        || name == "test.rs"
        || name == "tests.rs"
        || name.ends_with("_test.py")
        || name.ends_with("_tests.py")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// `_scoped_nodes_and_edges`: the nodes in scope and the edges between them.
pub(crate) fn scoped(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    artifact: Artifact,
    include_tests: bool,
) -> (Vec<GraphNode>, Vec<GraphEdge>) {
    let kept: Vec<GraphNode> = nodes
        .iter()
        .filter(|node| artifact.includes(node) && (include_tests || !excluded_from_analysis(node)))
        .cloned()
        .collect();
    let names: HashSet<&str> = kept.iter().map(|n| n.qualified_name.as_str()).collect();
    let links = edges
        .iter()
        .filter(|e| {
            names.contains(e.source_qualified.as_str())
                && names.contains(e.target_qualified.as_str())
        })
        .cloned()
        .collect();
    (kept, links)
}

fn pairs(edges: &[GraphEdge]) -> Vec<(String, String)> {
    edges
        .iter()
        .map(|e| (e.source_qualified.clone(), e.target_qualified.clone()))
        .collect()
}

/// `find_hub_nodes` computed from the scoped graph.
fn computed_hubs(
    nodes: &[GraphNode],
    links: &[(String, String)],
    communities: &HashMap<String, Option<i64>>,
    top_n: i64,
) -> Vec<Value> {
    let mut inbound: HashMap<&str, i64> = HashMap::new();
    let mut outbound: HashMap<&str, i64> = HashMap::new();
    for (source, target) in links {
        *outbound.entry(source).or_default() += 1;
        *inbound.entry(target).or_default() += 1;
    }
    let mut scored: Vec<(i64, Value)> = Vec::new();
    for node in nodes {
        let qn = node.qualified_name.as_str();
        let (ind, outd) = (
            inbound.get(qn).copied().unwrap_or(0),
            outbound.get(qn).copied().unwrap_or(0),
        );
        if ind + outd == 0 {
            continue;
        }
        scored.push((
            ind + outd,
            json!({
                "name": crate::query::sanitize(&node.name),
                "qualified_name": node.qualified_name,
                "kind": node.kind,
                "file": node.file_path,
                "in_degree": ind,
                "out_degree": outd,
                "total_degree": ind + outd,
                "community_id": communities.get(qn).copied().flatten(),
            }),
        ));
    }
    scored.sort_by_key(|item| std::cmp::Reverse(item.0));
    let ranked: Vec<Value> = scored.into_iter().map(|(_, value)| value).collect();
    py_prefix(&ranked, top_n)
}

/// `nx.betweenness_centrality(G, normalized=True)`, sampling `k=500` sources
/// with `seed=0` past 5000 nodes, as `find_bridge_nodes` calls it; each
/// source's BFS and accumulation run in networkx's order so the sums match.
fn betweenness(count: usize, successors: &[Vec<usize>]) -> Vec<f64> {
    let mut scores = vec![0.0_f64; count];
    let sampled: Option<Vec<usize>> = (count > 5000).then(|| {
        let k = 500.min(count);
        crate::pyrandom::PyRandom::new(0).sample(count, k)
    });
    let sources: Vec<usize> = sampled.clone().unwrap_or_else(|| (0..count).collect());
    let mut sigma = vec![0.0_f64; count];
    let mut distance = vec![-1_i64; count];
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut delta = vec![0.0_f64; count];
    for &source in &sources {
        let mut order = Vec::new();
        let mut queue = std::collections::VecDeque::from([source]);
        sigma[source] = 1.0;
        distance[source] = 0;
        while let Some(v) = queue.pop_front() {
            order.push(v);
            for &w in &successors[v] {
                if distance[w] < 0 {
                    queue.push_back(w);
                    distance[w] = distance[v] + 1;
                }
                if distance[w] == distance[v] + 1 {
                    sigma[w] += sigma[v];
                    predecessors[w].push(v);
                }
            }
        }
        for &w in order.iter().rev() {
            let coeff = (1.0 + delta[w]) / sigma[w];
            for &v in &predecessors[w] {
                delta[v] += sigma[v] * coeff;
            }
            if w != source {
                scores[w] += delta[w];
            }
        }
        for &v in &order {
            sigma[v] = 0.0;
            distance[v] = -1;
            predecessors[v].clear();
            delta[v] = 0.0;
        }
    }
    // `_rescale(normalized=True, directed=True, endpoints=False)`.
    let n_pairs = count as i64 - 1;
    if n_pairs < 2 {
        return scores;
    }
    match sampled {
        None => {
            let scale = 1.0 / ((n_pairs * (n_pairs - 1)) as f64);
            if scale != 1.0 {
                scores.iter_mut().for_each(|score| *score *= scale);
            }
        }
        Some(sampled) => {
            let k = sampled.len() as i64;
            let scale_source = if k > 1 {
                1.0 / (((k - 1) * (n_pairs - 1)) as f64)
            } else {
                f64::NAN
            };
            let scale_other = 1.0 / ((k * (n_pairs - 1)) as f64);
            let sampled: HashSet<usize> = sampled.into_iter().collect();
            for (index, score) in scores.iter_mut().enumerate() {
                *score *= if sampled.contains(&index) {
                    scale_source
                } else {
                    scale_other
                };
            }
        }
    }
    scores
}

/// `find_bridge_nodes` computed from the scoped graph.
fn computed_bridges(
    nodes: &[GraphNode],
    links: &[(String, String)],
    communities: &HashMap<String, Option<i64>>,
    top_n: i64,
) -> Vec<Value> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.qualified_name.as_str(), i))
        .collect();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (source, target) in links {
        let (from, to) = (index[source.as_str()], index[target.as_str()]);
        if !successors[from].contains(&to) {
            successors[from].push(to);
        }
    }
    let scores = betweenness(nodes.len(), &successors);
    let mut results: Vec<(f64, Value)> = Vec::new();
    for (node, score) in nodes.iter().zip(scores) {
        if score <= 0.0 || node.kind == "File" {
            continue;
        }
        let rounded = round_to(score, 6);
        results.push((
            rounded,
            json!({
                "name": crate::query::sanitize(&node.name),
                "qualified_name": node.qualified_name,
                "kind": node.kind,
                "file": node.file_path,
                "betweenness": rounded,
                "community_id": communities.get(&node.qualified_name).copied().flatten(),
            }),
        ));
    }
    results.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let ranked: Vec<Value> = results.into_iter().map(|(_, value)| value).collect();
    py_prefix(&ranked, top_n)
}

/// `_load_source_lines_for_node`: strict UTF-8, or nothing.
fn source_lines<'a>(
    store: &GraphStore,
    file_path: &str,
    cache: &'a mut HashMap<String, Vec<String>>,
) -> &'a [String] {
    cache
        .entry(file_path.to_string())
        .or_insert_with(|| crate::dead_code::source_lines(store, file_path))
}

fn rust_public_item() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^pub(\([^)]*\))?\s+(fn|struct|enum|trait|type|const|static)\b")
            .expect("regex")
    })
}

const JS_PUBLIC_MARKERS: &[&str] = &[
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

/// `_is_rust_cfg_test_candidate`.
fn rust_cfg_test_candidate(node: &GraphNode, lines: &[String]) -> bool {
    if node.language != "rust" || node.line_start <= 0 || lines.is_empty() {
        return false;
    }
    let target = ((node.line_start - 1) as usize).min(lines.len() - 1);
    for idx in (0..=target).rev() {
        if !lines[idx].contains("mod tests") {
            continue;
        }
        let window = lines[idx.saturating_sub(3)..=idx].join("\n");
        if !window.contains("#[cfg(test)]") {
            continue;
        }
        let depth: i64 = lines[idx..=target]
            .iter()
            .map(|line| line.matches('{').count() as i64 - line.matches('}').count() as i64)
            .sum();
        if depth > 0 {
            return true;
        }
    }
    false
}

/// `_low_signal_isolated_reason`.
fn low_signal_isolated_reason(
    store: &GraphStore,
    node: &GraphNode,
    cache: &mut HashMap<String, Vec<String>>,
) -> Option<&'static str> {
    if is_conventional_entry_point(node) {
        return Some("entry_point");
    }
    let lines = source_lines(store, &node.file_path, cache);
    let ls = node.line_start;
    if ls <= 0 || ls as usize > lines.len() {
        return None;
    }
    let line = lines[ls as usize - 1].trim();
    if line.is_empty() {
        return None;
    }
    if rust_cfg_test_candidate(node, lines) {
        return Some("test_candidate");
    }
    if node.language == "rust" && node.kind == "Class" && line.starts_with("impl ") {
        return Some("implementation_block");
    }
    if node.language == "rust" && rust_public_item().is_match(line) {
        return Some("public_api_candidate");
    }
    if matches!(
        node.language.as_str(),
        "typescript" | "tsx" | "javascript" | "vue" | "svelte"
    ) && (JS_PUBLIC_MARKERS.iter().any(|m| line.starts_with(m))
        || format!(" {line} ").contains(" export "))
    {
        return Some("public_api_candidate");
    }
    if line.starts_with("public ") || line.starts_with("export ") {
        return Some("public_api_candidate");
    }
    None
}

/// `_natural_single_file_community_reason`.
fn natural_single_file_reason(file_path: &str) -> Option<String> {
    let normalized = file_path.replace('\\', "/");
    let (_, parts) = posix_parts(&normalized);
    let name = parts.last().copied().unwrap_or("");
    let suffix = match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[i..],
        _ => "",
    };
    let stem = if suffix.is_empty() {
        name
    } else {
        &name[..name.len() - suffix.len()]
    };
    let (name, stem, suffix) = (
        name.to_lowercase(),
        stem.to_lowercase(),
        suffix.to_lowercase(),
    );
    if name.starts_with("readme") && matches!(suffix.as_str(), ".md" | ".rst" | ".txt") {
        return Some("standalone_readme".to_string());
    }
    if matches!(
        stem.as_str(),
        "license"
            | "licence"
            | "copying"
            | "security"
            | "code_of_conduct"
            | "contributing"
            | "authors"
            | "contributors"
            | "changelog"
            | "changes"
            | "release_notes"
    ) {
        return Some(format!("standalone_{stem}"));
    }
    if matches!(
        name.as_str(),
        "license" | "licence" | "copying" | "notice" | "authors" | "contributors" | "changelog"
    ) {
        return Some(format!("standalone_{name}"));
    }
    None
}

#[derive(Clone, Copy, Default)]
struct EdgeMetrics {
    internal: i64,
    external: i64,
    external_degree: i64,
    cohesion: f64,
    ratio: f64,
}

impl EdgeMetrics {
    fn fields(self) -> Vec<(&'static str, Value)> {
        vec![
            ("internal_edges", json!(self.internal)),
            ("external_edges", json!(self.external)),
            ("external_degree", json!(self.external_degree)),
            ("cohesion", json!(self.cohesion)),
            ("external_edge_ratio", json!(self.ratio)),
        ]
    }
}

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn with(item: &Value, extra: Vec<(&str, Value)>) -> Value {
    let mut map = item.as_object().cloned().unwrap_or_default();
    for (k, v) in extra {
        map.insert(k.to_string(), v);
    }
    Value::Object(map)
}

/// `find_knowledge_gaps(top_n, artifact_scope, include_tests)`.
pub(crate) fn find_knowledge_gaps(
    store: &GraphStore,
    graph: &Graph,
    top_n: i64,
    artifact: Artifact,
    artifact_name: &str,
    include_tests: bool,
) -> Value {
    let (nodes, edges) = scoped(&graph.nodes, &graph.edges, artifact, include_tests);
    let mut degree: HashMap<&str, i64> = HashMap::new();
    for e in &edges {
        *degree.entry(&e.source_qualified).or_default() += 1;
        *degree.entry(&e.target_qualified).or_default() += 1;
    }
    let mut full_degree: HashMap<&str, i64> = HashMap::new();
    let mut tested_sources: HashSet<&str> = HashSet::new();
    for e in &graph.edges {
        *full_degree.entry(&e.source_qualified).or_default() += 1;
        *full_degree.entry(&e.target_qualified).or_default() += 1;
        if e.kind == "TESTED_BY" {
            tested_sources.insert(&e.source_qualified);
        }
    }
    let scoped_qns: HashSet<&str> = nodes.iter().map(|n| n.qualified_name.as_str()).collect();
    let full = |qn: &str| full_degree.get(qn).copied().unwrap_or(0);
    let mut positive: Vec<i64> = nodes
        .iter()
        .filter(|n| !excluded_from_analysis(n) && full(&n.qualified_name) > 0)
        .map(|n| full(&n.qualified_name))
        .collect();
    positive.sort_unstable();
    let p95 = if positive.is_empty() {
        0
    } else {
        let rank = (0.95 * positive.len() as f64).ceil() as i64;
        positive[(rank - 1).clamp(0, positive.len() as i64 - 1) as usize]
    };
    let hotspot_min = p95.max(5);
    let top_n = top_n.max(1);
    let noise_limit = top_n.min(10);

    let mut cache = HashMap::new();
    let (mut isolated, mut low_isolated) = (Vec::new(), Vec::new());
    for n in &nodes {
        let d = degree.get(n.qualified_name.as_str()).copied().unwrap_or(0);
        if d > 1 {
            continue;
        }
        let item = json!({"name": sanitize(&n.name), "qualified_name": n.qualified_name, "kind": n.kind, "file": n.file_path, "degree": d});
        match low_signal_isolated_reason(store, n, &mut cache) {
            Some(reason) => low_isolated.push(with(&item, vec![("classification", json!(reason))])),
            None => isolated.push(item),
        }
    }

    let mut sizes: HashMap<i64, i64> = HashMap::new();
    let mut files: HashMap<i64, Vec<String>> = HashMap::new();
    let mut qn_cid: HashMap<&str, i64> = HashMap::new();
    for n in &nodes {
        if let Some(cid) = graph.communities.get(&n.qualified_name).copied().flatten() {
            *sizes.entry(cid).or_default() += 1;
            let list = files.entry(cid).or_default();
            if !list.contains(&n.file_path) {
                list.push(n.file_path.clone());
            }
            qn_cid.insert(&n.qualified_name, cid);
        }
    }
    let (mut internal, mut external): (HashMap<i64, i64>, HashMap<i64, i64>) = Default::default();
    let mut neighbors: HashMap<i64, HashSet<&str>> = HashMap::new();
    for e in &edges {
        let (s, t) = (
            qn_cid.get(e.source_qualified.as_str()).copied(),
            qn_cid.get(e.target_qualified.as_str()).copied(),
        );
        if s.is_none() && t.is_none() {
            continue;
        }
        if s.is_some() && s == t {
            *internal.entry(s.unwrap_or_default()).or_default() += 1;
            continue;
        }
        if let Some(s) = s {
            *external.entry(s).or_default() += 1;
            neighbors.entry(s).or_default().insert(&e.target_qualified);
        }
        if let Some(t) = t {
            *external.entry(t).or_default() += 1;
            neighbors.entry(t).or_default().insert(&e.source_qualified);
        }
    }
    let metrics_for = |cid: i64| -> EdgeMetrics {
        let size = sizes.get(&cid).copied().unwrap_or(0);
        let (inner, outer) = (
            internal.get(&cid).copied().unwrap_or(0),
            external.get(&cid).copied().unwrap_or(0),
        );
        let max_internal = (size * (size - 1)).max(1);
        let total = inner + outer;
        EdgeMetrics {
            internal: inner,
            external: outer,
            external_degree: neighbors.get(&cid).map_or(0, HashSet::len) as i64,
            cohesion: round_to((inner as f64 / max_internal as f64).min(1.0), 4),
            ratio: if total > 0 {
                round_to(outer as f64 / total as f64, 4)
            } else {
                0.0
            },
        }
    };

    let communities = store.get_communities_list().unwrap_or_default();
    let (mut thin, mut small_thin) = (Vec::new(), Vec::new());
    for (cid, name) in &communities {
        let Some(&size) = sizes.get(cid) else {
            continue;
        };
        if size >= 3 {
            continue;
        }
        let item = json!({"community_id": cid, "name": name, "size": size});
        let file_list = files.get(cid).cloned().unwrap_or_default();
        if file_list.len() == 1 {
            let mut extra = vec![("file", json!(file_list[0]))];
            extra.extend(metrics_for(*cid).fields());
            extra.push(("classification", json!("small_single_file_cluster")));
            small_thin.push(with(&item, extra));
        } else {
            thin.push(item);
        }
    }

    let mut hotspots: Vec<(i64, Value)> = Vec::new();
    for n in &nodes {
        let d = full(&n.qualified_name);
        if d >= hotspot_min
            && !(tested_sources.contains(n.qualified_name.as_str())
                && scoped_qns.contains(n.qualified_name.as_str()))
            && !excluded_from_analysis(n)
        {
            hotspots.push((d, json!({
                "name": sanitize(&n.name),
                "qualified_name": n.qualified_name,
                "kind": n.kind,
                "file": n.file_path,
                "degree": d,
                "hotspot_min_degree": hotspot_min,
                "evidence": "degree is at or above the repository p95 non-file degree threshold and no TESTED_BY edge starts from it",
            })));
        }
    }
    hotspots.sort_by_key(|item| std::cmp::Reverse(item.0));
    let hotspots: Vec<Value> = hotspots.into_iter().map(|(_, v)| v).collect();

    let (mut single, mut natural, mut small, mut integrated) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (cid, name) in &communities {
        let Some(&size) = sizes.get(cid) else {
            continue;
        };
        let file_list = files.get(cid).cloned().unwrap_or_default();
        if file_list.len() != 1 || size < 3 {
            continue;
        }
        let metrics = metrics_for(*cid);
        let mut fields = vec![
            ("community_id", json!(cid)),
            ("name", json!(name)),
            ("size", json!(size)),
            ("file", json!(file_list[0])),
        ];
        fields.extend(metrics.fields());
        let item = record(fields);
        if let Some(reason) = natural_single_file_reason(&file_list[0]) {
            natural.push(with(&item, vec![("classification", json!(reason))]));
        } else if size < 10 {
            small.push(with(
                &item,
                vec![("classification", json!("small_single_file_cluster"))],
            ));
        } else if metrics.external_degree >= 3.max((size as f64 * 0.2).ceil() as i64)
            && metrics.ratio >= 0.25
        {
            integrated.push(with(
                &item,
                vec![("classification", json!("integrated_single_file_component"))],
            ));
        } else {
            single.push(with(&item, vec![(
                "evidence",
                json!("community members are concentrated in one file and have limited external graph connectivity"),
            )]));
        }
    }

    let take = |items: &[Value], n: i64| py_prefix(items, n);
    let raw = json!({
        "untested_hotspots": hotspots.len(),
        "single_file_communities": single.len(),
        "isolated_nodes": isolated.len(),
        "thin_communities": thin.len(),
    });
    let returned = |items: &[Value]| take(items, top_n).len();
    let truncated = hotspots.len() > returned(&hotspots)
        || single.len() > returned(&single)
        || isolated.len() > returned(&isolated)
        || thin.len() > returned(&thin);
    json!({
        "untested_hotspots": take(&hotspots, top_n),
        "single_file_communities": take(&single, top_n),
        "isolated_nodes": take(&isolated, top_n),
        "thin_communities": take(&thin, top_n),
        "_meta": {
            "thresholds": {
                "isolated_max_degree": 1,
                "thin_community_min_size": 3,
                "single_file_min_size": 3,
                "untested_hotspot_min_degree": hotspot_min,
                "untested_hotspot_percentile": 0.95,
            },
            "degree_distribution": {"candidate_positive_degree_count": positive.len(), "p95_degree": p95},
            "artifact_scope": artifact_name,
            "include_tests": include_tests,
            "scoped_counts": {"nodes": nodes.len(), "edges": edges.len()},
            "top_n": top_n,
            "raw_counts": raw,
            "returned_counts": {
                "untested_hotspots": returned(&hotspots),
                "single_file_communities": returned(&single),
                "isolated_nodes": returned(&isolated),
                "thin_communities": returned(&thin),
            },
            "truncated": truncated,
            "exclusions": {
                "isolated_nodes": ["public API candidates, conventional entry points, test-only nodes, and implementation-block containers"],
                "thin_communities": ["single-file clusters with fewer than 3 members"],
                "untested_hotspots": ["test nodes and test-like file paths", "markdown documentation sections"],
                "single_file_communities": [
                    "natural standalone repo documents such as README, LICENSE, SECURITY, CODE_OF_CONDUCT",
                    "single-file clusters with fewer than 10 members",
                    "single-file communities with enough external graph connectivity to look like integrated components",
                ],
            },
            "classified_noise_counts": {
                "low_signal_isolated_nodes": low_isolated.len(),
                "small_single_file_thin_communities": small_thin.len(),
                "natural_single_file_communities": natural.len(),
                "small_single_file_communities": small.len(),
                "integrated_single_file_communities": integrated.len(),
            },
            "classified_noise_examples": {
                "low_signal_isolated_nodes": take(&low_isolated, noise_limit),
                "small_single_file_thin_communities": take(&small_thin, noise_limit),
                "natural_single_file_communities": take(&natural, noise_limit),
                "small_single_file_communities": take(&small, noise_limit),
                "integrated_single_file_communities": take(&integrated, noise_limit),
            },
        },
    })
}

/// `find_surprising_connections(top_n, artifact_scope, include_tests)`.
pub(crate) fn find_surprising_connections(
    graph: &Graph,
    top_n: i64,
    artifact: Artifact,
    include_tests: bool,
) -> Vec<Value> {
    let (nodes, edges) = scoped(&graph.nodes, &graph.edges, artifact, include_tests);
    let node_map: HashMap<&str, &GraphNode> = nodes
        .iter()
        .map(|n| (n.qualified_name.as_str(), n))
        .collect();
    let mut degree: HashMap<&str, i64> = HashMap::new();
    for e in &edges {
        *degree.entry(&e.source_qualified).or_default() += 1;
        *degree.entry(&e.target_qualified).or_default() += 1;
    }
    let mut degrees: Vec<i64> = degree.values().copied().filter(|d| *d > 0).collect();
    if degrees.is_empty() {
        return Vec::new();
    }
    degrees.sort_unstable();
    let median = degrees[degrees.len() / 2];
    let high = (median * 3).max(10);
    let max_degree = degrees.last().copied().unwrap_or(1).max(1);
    let community = |qn: &str| graph.communities.get(qn).copied().flatten();
    let mut pair_counts: HashMap<(i64, i64, &str), i64> = HashMap::new();
    for e in &edges {
        if let (Some(s), Some(t)) = (
            community(&e.source_qualified),
            community(&e.target_qualified),
        ) && s != t
        {
            *pair_counts
                .entry((s.min(t), s.max(t), e.kind.as_str()))
                .or_default() += 1;
        }
    }
    let lang = |path: &str| -> String {
        if path.contains('.') {
            path.rsplit('.').next().unwrap_or("").to_string()
        } else {
            String::new()
        }
    };
    let mut scored: Vec<(f64, Value)> = Vec::new();
    for e in &edges {
        if e.kind == "CONTAINS" {
            continue;
        }
        let (Some(src), Some(tgt)) = (
            node_map.get(e.source_qualified.as_str()),
            node_map.get(e.target_qualified.as_str()),
        ) else {
            continue;
        };
        if src.kind == "File" || tgt.kind == "File" {
            continue;
        }
        let mut score = 0.0_f64;
        let mut reasons: Vec<&str> = Vec::new();
        let mut boundary = false;
        let (src_cid, tgt_cid) = (
            community(&e.source_qualified),
            community(&e.target_qualified),
        );
        if let (Some(s), Some(t)) = (src_cid, tgt_cid)
            && s != t
        {
            score += 0.3;
            reasons.push("cross-community");
            boundary = true;
            let count = pair_counts[&(s.min(t), s.max(t), e.kind.as_str())];
            let bonus = round_to(0.05 / count as f64, 3).min(0.05);
            score += bonus;
            if bonus != 0.0 {
                reasons.push("rare-community-pair");
            }
        }
        let (sl, tl) = (lang(&src.file_path), lang(&tgt.file_path));
        if !sl.is_empty() && !tl.is_empty() && sl != tl {
            score += 0.2;
            reasons.push("cross-language");
            boundary = true;
        }
        let sd = degree
            .get(e.source_qualified.as_str())
            .copied()
            .unwrap_or(0);
        let td = degree
            .get(e.target_qualified.as_str())
            .copied()
            .unwrap_or(0);
        if (sd <= 2 && td >= high) || (td <= 2 && sd >= high) {
            score += 0.2;
            reasons.push("peripheral-to-hub");
        }
        let imbalance = round_to(
            (((sd - td).abs() as f64) / max_degree as f64 * 0.09).min(0.09),
            3,
        );
        if imbalance != 0.0 {
            score += imbalance;
            reasons.push("degree-imbalance");
        }
        if src.is_test != tgt.is_test && e.kind == "CALLS" {
            score += 0.15;
            reasons.push("cross-test-boundary");
            boundary = true;
        }
        if is_reportable_bridge(e) {
            score += 0.25;
            reasons.push("cross-artifact-bridge");
            boundary = true;
        }
        if e.kind == "CALLS" && src.kind == "Type" {
            score += 0.15;
            reasons.push("unusual-edge-kind");
            boundary = true;
        }
        if score > 0.0 && boundary {
            let rounded = round_to(score, 3);
            scored.push((
                rounded,
                json!({
                    "source": sanitize(&src.name),
                    "source_qualified": e.source_qualified,
                    "target": sanitize(&tgt.name),
                    "target_qualified": e.target_qualified,
                    "edge_kind": e.kind,
                    "surprise_score": rounded,
                    "reasons": reasons,
                    "source_community": src_cid,
                    "target_community": tgt_cid,
                }),
            ));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let ranked: Vec<Value> = scored.into_iter().map(|(_, v)| v).collect();
    py_prefix(&ranked, top_n)
}
