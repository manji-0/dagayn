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
    let min_size = min_size.max(1) as usize;
    let all_edges = store.get_all_edges()?;
    let unique_nodes = store.get_all_nodes_filtered(true)?;

    let mut results = detect_leiden(&unique_nodes, &all_edges, min_size);
    if results.is_empty() {
        results = detect_file_based(&unique_nodes, &all_edges, min_size);
    }

    results = split_oversized(results, &unique_nodes, &all_edges, 0.25, 10);
    Ok(results)
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

    let communities = detect_communities(store, min_size)?;
    let payload = serde_json::to_string(&communities).map_err(GraphError::from)?;
    store.store_communities_json(&payload)
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
