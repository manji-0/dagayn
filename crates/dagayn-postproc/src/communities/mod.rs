mod cohesion;
mod file_based;
mod leiden;
mod naming;

use dagayn_graph::{GraphError, GraphStore, Result};
use serde::Serialize;
use serde_json::json;

use file_based::detect_file_based;
use leiden::{detect_leiden, split_oversized};

#[derive(Clone, Debug, Serialize)]
pub struct DetectedCommunity {
    pub name: String,
    pub level: i64,
    pub size: i64,
    pub cohesion: f64,
    pub dominant_language: String,
    pub description: String,
    pub members: Vec<String>,
}

pub fn detect_communities(store: &GraphStore, min_size: i64) -> Result<Vec<DetectedCommunity>> {
    let unique_nodes = store.get_all_nodes_filtered(true)?;
    let all_edges = store.get_all_edges()?;
    Ok(detect_communities_from(&unique_nodes, &all_edges, min_size))
}

pub fn detect_communities_from(
    nodes: &[dagayn_graph::GraphNode],
    edges: &[dagayn_graph::GraphEdge],
    min_size: i64,
) -> Vec<DetectedCommunity> {
    let min_size = min_size.max(1) as usize;
    let mut results = detect_leiden(nodes, edges, min_size);
    if results.is_empty() {
        results = detect_file_based(nodes, edges, min_size);
    }
    split_oversized(results, nodes, edges, 0.25, 10)
}

pub fn detect_communities_json(store: &GraphStore, min_size: i64) -> Result<String> {
    let communities = detect_communities(store, min_size)?;
    serde_json::to_string(&communities).map_err(GraphError::from)
}

pub fn incremental_detect_communities(
    store: &mut GraphStore,
    changed_files: &[String],
    min_size: i64,
    pre_affected_count: Option<i64>,
) -> Result<i64> {
    if changed_files.is_empty() {
        return Ok(0);
    }

    let affected_count = match pre_affected_count {
        Some(count) => count,
        None => store.count_affected_communities(changed_files)?,
    };
    if affected_count == 0 {
        return Ok(0);
    }

    let affected_ids = store.affected_community_id_set(changed_files)?;
    if affected_ids.is_empty() {
        let communities = detect_communities(store, min_size)?;
        let payload = serde_json::to_string(&communities).map_err(GraphError::from)?;
        return store.store_communities_json(&payload);
    }

    let region_ids = store.expand_neighbor_community_ids(&affected_ids)?;
    let region_vec: Vec<i64> = region_ids.iter().copied().collect();
    let region_nodes = store.get_nodes_by_community_ids(&region_vec)?;
    let total_nodes = store.get_all_nodes_filtered(true)?.len();
    if region_nodes.is_empty()
        || total_nodes == 0
        || (region_nodes.len() as f64) / (total_nodes as f64) > 0.5
    {
        let communities = detect_communities(store, min_size)?;
        let payload = serde_json::to_string(&communities).map_err(GraphError::from)?;
        return store.store_communities_json(&payload);
    }

    let region_qns: std::collections::HashSet<String> = region_nodes
        .iter()
        .map(|node| node.qualified_name.clone())
        .collect();
    let all_edges = store.get_all_edges()?;
    let region_edges: Vec<_> = all_edges
        .into_iter()
        .filter(|edge| {
            region_qns.contains(&edge.source_qualified)
                && region_qns.contains(&edge.target_qualified)
        })
        .collect();
    let detected = detect_communities_from(&region_nodes, &region_edges, min_size);
    let inputs: Vec<dagayn_graph::CommunityInput> = detected
        .into_iter()
        .map(|community| dagayn_graph::CommunityInput {
            name: community.name,
            level: community.level,
            cohesion: community.cohesion,
            size: community.size,
            dominant_language: community.dominant_language,
            description: community.description,
            members: community.members,
        })
        .collect();
    store.replace_communities(&region_vec, &inputs)
}

pub fn refresh_community_stats_json(store: &mut GraphStore) -> Result<String> {
    let members_by_id = store.get_all_community_member_qns()?;
    let all_edges = store.get_all_edges()?;
    let mut updated = 0i64;
    let mut deleted = 0i64;

    if !members_by_id.is_empty() {
        let community_ids: Vec<i64> = members_by_id.keys().copied().collect();
        let member_sets: Vec<std::collections::HashSet<String>> = community_ids
            .iter()
            .map(|community_id| {
                members_by_id
                    .get(community_id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            })
            .collect();
        let cohesions = cohesion::compute_cohesion_batch(&member_sets, &all_edges);

        for ((community_id, member_qns), cohesion) in
            community_ids.into_iter().zip(member_sets).zip(cohesions)
        {
            let size = member_qns.len() as i64;
            if size == 0 {
                store.delete_community(community_id)?;
                deleted += 1;
            } else {
                store.update_community_stats(community_id, size, round_cohesion(cohesion))?;
                updated += 1;
            }
        }
    }

    deleted += store.delete_orphan_communities()?;
    serde_json::to_string(&json!({"updated": updated, "deleted": deleted}))
        .map_err(GraphError::from)
}

fn round_cohesion(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}
