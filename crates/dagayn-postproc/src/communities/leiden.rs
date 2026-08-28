use std::collections::{HashMap, HashSet};

use dagayn_graph::{GraphEdge, GraphNode};
use leiden_rs::{from_petgraph, Leiden, LeidenConfig, QualityType};
use petgraph::graph::Graph;
use petgraph::Undirected;

use super::cohesion::compute_cohesion_batch;
use super::file_based::detect_file_based;
use super::naming::generate_community_name;
use super::DetectedCommunity;

const LEIDEN_RANDOM_SEED: u64 = 20260813;

pub(crate) fn leiden_resolution(node_count: usize) -> f64 {
    let n_nodes = node_count.max(1) as f64;
    (1.0_f64 / n_nodes.max(10.0_f64).log10()).max(0.05_f64)
}

const EDGE_WEIGHTS: &[(&str, f64)] = &[
    ("CALLS", 1.0),
    ("IMPORTS_FROM", 0.5),
    ("INHERITS", 0.8),
    ("IMPLEMENTS", 0.7),
    ("CONTAINS", 0.3),
    ("TESTED_BY", 0.4),
    ("DEPENDS_ON", 0.6),
    ("CROSS_ARTIFACT", 0.6),
];

pub(crate) fn detect_leiden(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    min_size: usize,
) -> Vec<DetectedCommunity> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let mut qn_to_idx: HashMap<String, usize> = HashMap::new();
    for (idx, node) in nodes.iter().enumerate() {
        qn_to_idx.insert(node.qualified_name.clone(), idx);
    }

    let mut graph: Graph<(), f64, Undirected> = Graph::new_undirected();
    let mut node_indices = Vec::with_capacity(nodes.len());
    for _ in nodes {
        node_indices.push(graph.add_node(()));
    }

    let mut seen_edges: HashSet<(usize, usize)> = HashSet::new();
    for edge in edges {
        let Some(src_idx) = qn_to_idx.get(&edge.source_qualified).copied() else {
            continue;
        };
        let Some(tgt_idx) = qn_to_idx.get(&edge.target_qualified).copied() else {
            continue;
        };
        if src_idx == tgt_idx {
            continue;
        }
        let pair = if src_idx < tgt_idx {
            (src_idx, tgt_idx)
        } else {
            (tgt_idx, src_idx)
        };
        if seen_edges.insert(pair) {
            let weight = EDGE_WEIGHTS
                .iter()
                .find_map(|(kind, weight)| (*kind == edge.kind).then_some(*weight))
                .unwrap_or(0.5);
            graph.add_edge(node_indices[pair.0], node_indices[pair.1], weight);
        }
    }

    if graph.edge_count() == 0 {
        return detect_file_based(nodes, edges, min_size);
    }

    let data = match from_petgraph(&graph) {
        Ok(data) => data,
        Err(_) => return detect_file_based(nodes, edges, min_size),
    };

    let resolution = leiden_resolution(graph.node_count());

    let result = match Leiden::new(LeidenConfig {
        max_iterations: 2,
        resolution,
        seed: Some(LEIDEN_RANDOM_SEED),
        quality: QualityType::Modularity,
        ..Default::default()
    })
    .run(&data)
    {
        Ok(result) => result,
        Err(_) => return detect_file_based(nodes, edges, min_size),
    };

    let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    for idx in 0..nodes.len() {
        let community_id = result.partition.community_of(idx);
        clusters.entry(community_id).or_default().push(idx);
    }

    let mut pending: Vec<(Vec<GraphNode>, HashSet<String>)> = Vec::new();
    for cluster_ids in clusters.into_values() {
        if cluster_ids.len() < min_size {
            continue;
        }
        let members: Vec<GraphNode> = cluster_ids
            .iter()
            .filter_map(|idx| nodes.get(*idx).cloned())
            .collect();
        if members.len() < min_size {
            continue;
        }
        let member_qns = members
            .iter()
            .map(|node| node.qualified_name.clone())
            .collect::<HashSet<_>>();
        pending.push((members, member_qns));
    }

    let cohesions = compute_cohesion_batch(
        &pending
            .iter()
            .map(|(_, qns)| qns.clone())
            .collect::<Vec<_>>(),
        edges,
    );

    pending
        .into_iter()
        .zip(cohesions)
        .map(|((members, member_qns), cohesion)| {
            let dominant_language = dominant_language(&members);
            DetectedCommunity {
                name: generate_community_name(&members),
                level: 0,
                size: members.len() as i64,
                cohesion: round_cohesion(cohesion),
                dominant_language,
                description: format!("Community of {} nodes", members.len()),
                members: member_qns.into_iter().collect(),
            }
        })
        .collect()
}

pub(crate) fn split_oversized(
    communities: Vec<DetectedCommunity>,
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    threshold_pct: f64,
    min_split_size: usize,
) -> Vec<DetectedCommunity> {
    let total: i64 = communities.iter().map(|community| community.size).sum();
    if total == 0 {
        return communities;
    }

    let threshold = (total as f64 * threshold_pct) as i64;
    let threshold = threshold.max(min_split_size as i64);
    let mut result = Vec::new();
    let mut next_id = 1i64;

    let nodes_by_qn: HashMap<&str, &GraphNode> = nodes
        .iter()
        .map(|node| (node.qualified_name.as_str(), node))
        .collect();

    let mut qn_to_oversized: HashMap<&str, usize> = HashMap::new();
    for (idx, community) in communities.iter().enumerate() {
        if community.size > threshold {
            for member in &community.members {
                qn_to_oversized.insert(member.as_str(), idx);
            }
        }
    }
    let mut edges_by_community: HashMap<usize, Vec<&GraphEdge>> = HashMap::new();
    if !qn_to_oversized.is_empty() {
        for edge in edges {
            let Some(&src_comm) = qn_to_oversized.get(edge.source_qualified.as_str()) else {
                continue;
            };
            let Some(&tgt_comm) = qn_to_oversized.get(edge.target_qualified.as_str()) else {
                continue;
            };
            if src_comm == tgt_comm {
                edges_by_community.entry(src_comm).or_default().push(edge);
            }
        }
    }

    for (idx, community) in communities.into_iter().enumerate() {
        let members: HashSet<String> = community.members.iter().cloned().collect();
        if community.size <= threshold {
            result.push(community);
            continue;
        }

        let member_nodes: Vec<GraphNode> = members
            .iter()
            .filter_map(|qn| nodes_by_qn.get(qn.as_str()).cloned())
            .cloned()
            .collect();
        if member_nodes.len() < min_split_size {
            result.push(community);
            continue;
        }

        let member_edges: Vec<GraphEdge> = edges_by_community
            .remove(&idx)
            .unwrap_or_default()
            .into_iter()
            .cloned()
            .collect();

        let sub_communities = detect_leiden_subgraph(&member_nodes, &member_edges, 0.5);
        if sub_communities.len() <= 1 {
            result.push(community);
            continue;
        }

        let parent_name = community.name.clone();
        for mut sub in sub_communities {
            sub.level = community.level + 1;
            sub.name = format!("{parent_name}-sub{next_id}");
            next_id += 1;
            result.push(sub);
        }
    }

    backfill_split_cohesion(&mut result, edges);
    result
}

fn detect_leiden_subgraph(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    resolution: f64,
) -> Vec<DetectedCommunity> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let mut qn_to_idx: HashMap<String, usize> = HashMap::new();
    for (idx, node) in nodes.iter().enumerate() {
        qn_to_idx.insert(node.qualified_name.clone(), idx);
    }

    let mut graph: Graph<(), f64, Undirected> = Graph::new_undirected();
    let mut node_indices = Vec::with_capacity(nodes.len());
    for _ in nodes {
        node_indices.push(graph.add_node(()));
    }

    for edge in edges {
        let Some(src_idx) = qn_to_idx.get(&edge.source_qualified).copied() else {
            continue;
        };
        let Some(tgt_idx) = qn_to_idx.get(&edge.target_qualified).copied() else {
            continue;
        };
        if src_idx == tgt_idx {
            continue;
        }
        let weight = EDGE_WEIGHTS
            .iter()
            .find_map(|(kind, weight)| (*kind == edge.kind).then_some(*weight))
            .unwrap_or(0.5);
        graph.add_edge(node_indices[src_idx], node_indices[tgt_idx], weight);
    }

    if graph.edge_count() == 0 {
        return Vec::new();
    }

    let data = match from_petgraph(&graph) {
        Ok(data) => data,
        Err(_) => return Vec::new(),
    };

    let result = match Leiden::new(LeidenConfig {
        resolution,
        seed: Some(LEIDEN_RANDOM_SEED),
        quality: QualityType::Modularity,
        ..Default::default()
    })
    .run(&data)
    {
        Ok(result) => result,
        Err(_) => return Vec::new(),
    };

    let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    for idx in 0..nodes.len() {
        clusters
            .entry(result.partition.community_of(idx))
            .or_default()
            .push(idx);
    }

    clusters
        .into_values()
        .filter_map(|cluster_ids| {
            let members: Vec<GraphNode> = cluster_ids
                .iter()
                .filter_map(|idx| nodes.get(*idx).cloned())
                .collect();
            if members.is_empty() {
                return None;
            }
            let member_qns = members
                .iter()
                .map(|node| node.qualified_name.clone())
                .collect::<Vec<_>>();
            Some(DetectedCommunity {
                name: generate_community_name(&members),
                level: 0,
                size: members.len() as i64,
                cohesion: 0.0,
                dominant_language: dominant_language(&members),
                description: format!("Split from {}", generate_community_name(&members)),
                members: member_qns,
            })
        })
        .collect()
}

fn backfill_split_cohesion(communities: &mut [DetectedCommunity], edges: &[GraphEdge]) {
    let member_sets: Vec<HashSet<String>> = communities
        .iter()
        .map(|community| community.members.iter().cloned().collect())
        .collect();
    let cohesions = compute_cohesion_batch(&member_sets, edges);
    for (community, cohesion) in communities.iter_mut().zip(cohesions) {
        if community.cohesion == 0.0 {
            community.cohesion = round_cohesion(cohesion);
        }
    }
}

fn dominant_language(members: &[GraphNode]) -> String {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for node in members {
        if !node.language.is_empty() {
            *counts.entry(node.language.as_str()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(language, _)| language.to_string())
        .unwrap_or_default()
}

fn round_cohesion(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use dagayn_graph::{ConfidenceTier, GraphEdge, GraphNode};
    use serde_json::Value;

    fn test_node(qualified_name: &str) -> GraphNode {
        let (file_path, name) = qualified_name
            .split_once("::")
            .unwrap_or((qualified_name, qualified_name));
        GraphNode {
            id: 0,
            kind: "Function".to_string(),
            name: name.to_string(),
            qualified_name: qualified_name.to_string(),
            file_path: file_path.to_string(),
            line_start: 1,
            line_end: 2,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            is_test: false,
            file_hash: None,
            extra: Value::Null,
            signature: None,
        }
    }

    fn test_edge(source: &str, target: &str) -> GraphEdge {
        GraphEdge {
            id: 0,
            kind: "CALLS".to_string(),
            source_qualified: source.to_string(),
            target_qualified: target.to_string(),
            file_path: String::new(),
            line: 1,
            extra: Value::Null,
            confidence: 1.0,
            confidence_tier: ConfidenceTier::Exact,
        }
    }

    #[test]
    fn split_oversized_keeps_small_communities() {
        let nodes = vec![
            test_node("a.py::a1"),
            test_node("a.py::a2"),
            test_node("a.py::a3"),
            test_node("a.py::a4"),
            test_node("b.py::b1"),
            test_node("c.py::only"),
        ];
        let edges = vec![
            test_edge("a.py::a1", "a.py::a2"),
            test_edge("a.py::a2", "a.py::a3"),
            test_edge("a.py::a3", "a.py::a4"),
            test_edge("b.py::b1", "c.py::only"),
        ];
        let communities = vec![
            DetectedCommunity {
                name: "big-a".to_string(),
                level: 0,
                size: 4,
                cohesion: 0.0,
                dominant_language: "python".to_string(),
                description: String::new(),
                members: vec![
                    "a.py::a1".into(),
                    "a.py::a2".into(),
                    "a.py::a3".into(),
                    "a.py::a4".into(),
                ],
            },
            DetectedCommunity {
                name: "tiny".to_string(),
                level: 0,
                size: 1,
                cohesion: 0.0,
                dominant_language: "python".to_string(),
                description: String::new(),
                members: vec!["c.py::only".into()],
            },
            DetectedCommunity {
                name: "small-b".to_string(),
                level: 0,
                size: 1,
                cohesion: 0.0,
                dominant_language: "python".to_string(),
                description: String::new(),
                members: vec!["b.py::b1".into()],
            },
        ];

        let result = split_oversized(communities, &nodes, &edges, 0.25, 2);
        assert!(
            result.iter().any(|community| community.name == "tiny"),
            "communities under the size threshold must be kept as-is"
        );
        let member_count: usize = result.iter().map(|community| community.members.len()).sum();
        assert_eq!(member_count, 6);
    }

    #[test]
    fn leiden_resolution_stays_on_log10_heuristic() {
        assert!((leiden_resolution(10) - 1.0).abs() < 1e-9);
        assert!(leiden_resolution(1_000_000) >= 0.05);
        assert!(leiden_resolution(100) < leiden_resolution(10));
    }
}
