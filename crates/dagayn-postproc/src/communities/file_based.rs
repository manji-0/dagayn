use std::collections::{HashMap, HashSet};

use dagayn_graph::{GraphEdge, GraphNode};

use super::cohesion::compute_cohesion_batch;
use super::naming::generate_community_name;
use super::DetectedCommunity;

pub(crate) fn detect_file_based(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    min_size: usize,
) -> Vec<DetectedCommunity> {
    let mut all_dir_parts: Vec<Vec<String>> = Vec::new();
    for node in nodes {
        let parts: Vec<String> = node
            .file_path
            .replace('\\', "/")
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
        let dir_parts = if parts.len() > 1 {
            parts[..parts.len() - 1].to_vec()
        } else {
            Vec::new()
        };
        all_dir_parts.push(dir_parts);
    }

    let mut prefix_len = 0usize;
    if !all_dir_parts.is_empty() {
        let shortest = all_dir_parts.iter().map(Vec::len).min().unwrap_or(0);
        for i in 0..shortest {
            let seg = &all_dir_parts[0][i];
            if all_dir_parts.iter().all(|parts| parts.get(i) == Some(seg)) {
                prefix_len = i + 1;
            } else {
                break;
            }
        }
    }

    let group_at_depth = |depth: usize| -> HashMap<String, Vec<GraphNode>> {
        let mut groups: HashMap<String, Vec<GraphNode>> = HashMap::new();
        for (node, dir_parts) in nodes.iter().zip(&all_dir_parts) {
            let remainder = if dir_parts.len() > prefix_len {
                &dir_parts[prefix_len..]
            } else {
                &[][..]
            };
            let key = if !remainder.is_empty() {
                remainder
                    .iter()
                    .take(depth)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("/")
            } else {
                node.file_path
                    .replace('\\', "/")
                    .rsplit('/')
                    .next()
                    .and_then(|name| name.rsplit_once('.').map(|(stem, _)| stem.to_string()))
                    .unwrap_or_else(|| "root".to_string())
            };
            groups.entry(key).or_default().push(node.clone());
        }
        groups
    };

    let max_depth = all_dir_parts
        .iter()
        .map(|parts| parts.len().saturating_sub(prefix_len))
        .max()
        .unwrap_or(0);
    let mut best_groups = group_at_depth(1);
    for depth in 1..=max_depth.max(1) {
        let groups = group_at_depth(depth);
        let qualifying = groups
            .values()
            .filter(|members| members.len() >= min_size)
            .count();
        best_groups = groups;
        if qualifying >= 10 {
            break;
        }
    }

    let mut pending: Vec<(String, Vec<GraphNode>, HashSet<String>)> = Vec::new();
    for (dir_path, members) in best_groups {
        if members.len() < min_size {
            continue;
        }
        let member_qns = members
            .iter()
            .map(|node| node.qualified_name.clone())
            .collect::<HashSet<_>>();
        pending.push((dir_path, members, member_qns));
    }

    let cohesions = compute_cohesion_batch(
        &pending
            .iter()
            .map(|(_, _, qns)| qns.clone())
            .collect::<Vec<_>>(),
        edges,
    );

    pending
        .into_iter()
        .zip(cohesions)
        .map(|((dir_path, members, member_qns), cohesion)| {
            let dominant_language = dominant_language(&members);
            DetectedCommunity {
                name: generate_community_name(&members),
                level: 0,
                size: members.len() as i64,
                cohesion: round_cohesion(cohesion),
                dominant_language,
                description: format!("Directory-based community: {dir_path}"),
                members: member_qns.into_iter().collect(),
            }
        })
        .collect()
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
