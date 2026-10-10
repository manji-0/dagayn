//! `architecture_analysis_tool`'s `communities` and `community` modes
//! (`dagayn.communities` builds the communities they read).

use dagayn_graph::GraphStore;
use serde_json::{Value, json};

use crate::Ordered;
use crate::analysis::py_prefix;
use crate::query::node_dict;

/// `get_communities(store, sort_by, min_size)`.
fn get_communities(store: &GraphStore, sort_by: &str, min_size: i64) -> Option<Vec<Value>> {
    serde_json::from_str(&store.get_communities_json(sort_by, min_size).ok()?).ok()
}

/// `list_communities_func(sort_by, min_size, detail_level, limit)`.
pub(crate) fn list_communities(
    store: &GraphStore,
    sort_by: &str,
    min_size: i64,
    detail_level: &str,
    limit: i64,
) -> Option<Ordered> {
    let communities: Vec<Value> = if detail_level == "minimal" {
        store
            .community_summaries(sort_by, min_size)
            .ok()?
            .into_iter()
            .map(|(name, size, cohesion)| json!({"name": name, "size": size, "cohesion": cohesion}))
            .collect()
    } else {
        get_communities(store, sort_by, min_size)?
    };
    let total = communities.len();
    let truncated = total as i64 > limit;
    let mut summary = format!("Found {total} communities");
    if truncated {
        summary.push_str(&format!(". Showing first {limit}."));
    }
    let out = Ordered::default()
        .put("status", "ok")
        .put("summary", summary)
        .put("communities", Value::Array(py_prefix(&communities, limit)))
        .put("total", total)
        .put("truncated", truncated)
        .apply_output_budget(4000, &["communities"]);
    Some(out)
}

/// `get_community_func(community_name, community_id, include_members)`.
pub(crate) fn get_community(
    store: &GraphStore,
    name: Option<&str>,
    id: Option<i64>,
    include_members: bool,
) -> Option<Ordered> {
    let all = get_communities(store, "size", 0)?;
    let found = match (id, name) {
        (Some(id), _) => all.into_iter().find(|c| c["id"].as_i64() == Some(id)),
        (None, Some(name)) => {
            let wanted = name.to_lowercase();
            all.into_iter().find(|c| {
                c["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&wanted)
            })
        }
        (None, None) => None,
    };
    let Some(mut community) = found else {
        return Some(
            Ordered::default()
                .put("status", "not_found")
                .put("summary", "No community found matching the given criteria."),
        );
    };
    if include_members && let Some(cid) = community["id"].as_i64() {
        let members: Vec<Value> = store
            .get_nodes_by_community_id(cid)
            .ok()?
            .iter()
            .map(node_dict)
            .collect();
        let entries = community
            .as_object()?
            .iter()
            .fold(Ordered::default(), |o, (k, v)| o.put(k, v.clone()))
            .put("member_details", Value::Array(members));
        community = entries
            .apply_output_budget(5000, &["member_details"])
            .value();
    }
    let summary = format!(
        "Community '{}': {} nodes, cohesion {:.4}",
        community["name"].as_str().unwrap_or(""),
        community["size"],
        community["cohesion"].as_f64().unwrap_or(0.0)
    );
    let out = Ordered::default()
        .put("status", "ok")
        .put("summary", summary)
        .put("community", community);
    Some(out)
}
