use std::collections::{HashMap, HashSet};

use dagayn_graph::GraphEdge;

pub(crate) fn compute_cohesion_batch(
    community_member_qns: &[HashSet<String>],
    all_edges: &[GraphEdge],
) -> Vec<f64> {
    let mut qn_to_idx: HashMap<&str, usize> = HashMap::new();
    for (idx, members) in community_member_qns.iter().enumerate() {
        for qn in members {
            qn_to_idx.insert(qn.as_str(), idx);
        }
    }

    let n = community_member_qns.len();
    let mut internal = vec![0usize; n];
    let mut external = vec![0usize; n];

    for edge in all_edges {
        let sc = qn_to_idx.get(edge.source_qualified.as_str()).copied();
        let tc = qn_to_idx.get(edge.target_qualified.as_str()).copied();
        if sc.is_none() && tc.is_none() {
            continue;
        }
        if edge.kind == "CALLS" && !edge.target_qualified.contains("::") {
            continue;
        }
        match (sc, tc) {
            (Some(left), Some(right)) if left == right => internal[left] += 1,
            (Some(left), Some(right)) => {
                external[left] += 1;
                external[right] += 1;
            }
            (Some(left), None) => external[left] += 1,
            (None, Some(right)) => external[right] += 1,
            (None, None) => {}
        }
    }

    (0..n)
        .map(|idx| {
            let total = internal[idx] + external[idx];
            if total > 0 {
                internal[idx] as f64 / total as f64
            } else {
                0.0
            }
        })
        .collect()
}
