//! `graph_answerability_summary` and `missingness_from_answerability`
//! (`dagayn.tools._common`).

use dagayn_build::CommitFreshness;
use dagayn_graph::{GraphStats, GraphStore};
use serde_json::{Map, Value, json};

use crate::suggestions::round_to;

/// Reason codes about communities and stored flows, which only the answers
/// read from them carry as missingness.
const DERIVED_STRUCTURE_CODES: [&str; 5] = [
    "missing_flows",
    "missing_communities",
    "missing_flows_table",
    "missing_communities_table",
    "stale_derived_structures",
];

/// Whether `code` is about communities or stored flows.
pub(crate) fn is_derived_structure_code(code: &str) -> bool {
    DERIVED_STRUCTURE_CODES.contains(&code)
}

pub(crate) struct Answerability {
    pub status: &'static str,
    pub score: f64,
    pub reason_codes: Vec<&'static str>,
    parse: Value,
    answerability: Value,
    unresolved_edges: i64,
    counts: Map<String, Value>,
}

impl Answerability {
    /// `graph_answerability_summary(store)` without a freshness argument: the
    /// commit tier of the root the graph records, none when it records none;
    /// `None` where Python would fail.
    pub(crate) fn recorded(store: &GraphStore, stats: &GraphStats) -> Option<Self> {
        let freshness = match store.get_metadata("repo_root").ok()? {
            Some(recorded) if !recorded.is_empty() => {
                dagayn_build::commit_tier_freshness(store, std::path::Path::new(&recorded)).ok()?
            }
            Some(_) => return None,
            None => None,
        };
        Some(Self::compute(store, stats, freshness.as_ref()))
    }

    /// The summary for `stats`, with the freshness codes of `freshness`
    /// (`None` where Python's freshness state is `None`).
    pub(crate) fn compute(
        store: &GraphStore,
        stats: &GraphStats,
        freshness: Option<&CommitFreshness>,
    ) -> Self {
        let counts = store.answerability_counts();
        let edge_kind = |kind: &str| stats.edges_by_kind.get(kind).copied().unwrap_or(0);
        let test_edges = edge_kind("TESTED_BY");
        let cross_artifact = edge_kind("CROSS_ARTIFACT");
        let reportable_cross = (cross_artifact - counts.unresolved_markdown_code_spans).max(0);
        let reportable_unresolved =
            (counts.unresolved_cross_artifact_edges - counts.unresolved_markdown_code_spans).max(0);
        let unresolved_ratio = if reportable_cross != 0 {
            reportable_unresolved as f64 / reportable_cross as f64
        } else {
            0.0
        };
        let last_updated = stats
            .last_updated
            .as_deref()
            .is_some_and(|value| !value.is_empty());

        let mut reason_codes: Vec<&'static str> = Vec::new();
        let mut score = 1.0_f64;
        if !counts.failures.is_empty() {
            for failure in &counts.failures {
                if !reason_codes.contains(failure) {
                    reason_codes.push(failure);
                }
            }
            score -= 0.2;
        }
        if stats.total_nodes == 0 || stats.files_count == 0 {
            reason_codes.push("empty_graph");
            score = 0.0;
        }
        if counts.flows == 0 {
            reason_codes.push("missing_flows");
            score -= 0.15;
        }
        if counts.communities == 0 {
            reason_codes.push("missing_communities");
            score -= 0.15;
        }
        if test_edges == 0 {
            reason_codes.push("missing_test_edges");
            score -= 0.1;
        }
        if cross_artifact != 0 && unresolved_ratio > 0.35 {
            reason_codes.push("many_unresolved_cross_artifact_edges");
            score -= 0.15;
        }
        if !last_updated {
            reason_codes.push("missing_last_updated");
            score -= 0.1;
        }
        if counts.stale_flow_memberships > 0 || counts.unassigned_nodes > 0 {
            reason_codes.push("stale_derived_structures");
            score -= 0.15;
        }
        // `_freshness_reason_codes`.
        let mut freshness_counts = Map::new();
        if let Some(fresh) = freshness {
            let drift = !fresh.extractor_drift.is_empty();
            let moved = fresh.git_head_sha.as_deref() != Some(fresh.current_head_sha.as_str());
            if fresh.state == "commit_drift" && (moved || !drift) {
                reason_codes.push("graph_describes_another_commit");
                score -= 0.25;
            } else if fresh.worktree_dirty {
                reason_codes.push("uncommitted_changes_may_be_unindexed");
                score -= 0.1;
            }
            if drift {
                reason_codes.push("graph_built_by_older_extractor");
                score -= 0.25;
            }
            freshness_counts.insert("graph_head_sha".into(), json!(fresh.git_head_sha));
            freshness_counts.insert("current_head_sha".into(), json!(fresh.current_head_sha));
            freshness_counts.insert("worktree_dirty".into(), json!(fresh.worktree_dirty));
        }
        let score = round_to(score, 4).max(0.0);
        let status = if score >= 0.75 {
            "ok"
        } else if score > 0.0 {
            "degraded"
        } else {
            "empty"
        };
        let mut all_counts = Map::new();
        all_counts.insert("flows".into(), json!(counts.flows));
        all_counts.insert("communities".into(), json!(counts.communities));
        all_counts.insert("test_edges".into(), json!(test_edges));
        all_counts.insert(
            "reportable_cross_artifact_edges".into(),
            json!(reportable_cross),
        );
        all_counts.insert(
            "reportable_unresolved_cross_artifact_edges".into(),
            json!(reportable_unresolved),
        );
        all_counts.insert(
            "stale_flow_memberships".into(),
            json!(counts.stale_flow_memberships),
        );
        all_counts.insert("unassigned_nodes".into(), json!(counts.unassigned_nodes));
        all_counts.extend(freshness_counts);
        Self {
            status,
            score,
            reason_codes,
            parse: json!([stats.files_count, stats.languages.len(), last_updated]),
            answerability: json!([
                counts.flows,
                counts.communities,
                test_edges,
                reportable_cross,
                round_to(unresolved_ratio, 4)
            ]),
            unresolved_edges: reportable_unresolved,
            counts: all_counts,
        }
    }

    /// The summary without `counts`, as `get_minimal_context` reports it.
    pub(crate) fn without_counts(&self) -> Value {
        let mut health = Map::new();
        health.insert("status".into(), json!(self.status));
        health.insert("score".into(), json!(self.score));
        health.insert("reason_codes".into(), json!(self.reason_codes));
        health.insert("parse".into(), self.parse.clone());
        health.insert("answerability".into(), self.answerability.clone());
        if self.unresolved_edges != 0 {
            health.insert("unresolved_edges".into(), json!(self.unresolved_edges));
        }
        Value::Object(health)
    }

    /// `status`, `score`, and `reason_codes` only (`_COMPACT_ANSWERABILITY`).
    pub(crate) fn compact(&self) -> Value {
        json!({"status": self.status, "score": self.score, "reason_codes": self.reason_codes})
    }

    /// The full summary, `counts` included.
    pub(crate) fn full(&self) -> Value {
        let mut health = self.without_counts();
        if let Some(map) = health.as_object_mut() {
            map.insert("counts".into(), Value::Object(self.counts.clone()));
        }
        health
    }

    /// `missingness_from_answerability` for an answer that reads no derived
    /// structure: the codes about communities and stored flows say nothing
    /// about it (docs/plans/AGENT-WORKFLOW-TARGET.md#caveats).
    pub(crate) fn missingness(&self) -> Vec<Value> {
        self.missingness_where(|code| !is_derived_structure_code(code))
    }

    /// `missingness_from_answerability` for an answer read from communities
    /// or stored flows: every code.
    pub(crate) fn missingness_with_derived(&self) -> Vec<Value> {
        self.missingness_where(|_| true)
    }

    fn missingness_where(&self, keep: impl Fn(&str) -> bool) -> Vec<Value> {
        self.reason_codes
            .iter()
            .filter(|code| keep(code))
            .map(|code| {
                json!({
                    "reason_code": code,
                    "severity": severity(code),
                    "claim_effect": claim_effect(code),
                })
            })
            .collect()
    }
}

fn severity(code: &str) -> &'static str {
    match code {
        "empty_graph"
        | "no_sqlite_connection"
        | "graph_describes_another_commit"
        | "graph_built_by_older_extractor" => "high",
        "missing_flows"
        | "missing_communities"
        | "missing_test_edges"
        | "many_unresolved_cross_artifact_edges"
        | "missing_graph_stats"
        | "missing_flows_table"
        | "missing_communities_table"
        | "missing_cross_artifact_edge_metadata"
        | "missing_cross_artifact_edges"
        | "answerability_unavailable"
        | "stale_derived_structures"
        | "uncommitted_changes_may_be_unindexed" => "medium",
        _ => "low",
    }
}

fn claim_effect(code: &str) -> &'static str {
    match code {
        "graph_describes_another_commit" => {
            "the graph answers for a different commit -- absence, line numbers and blast \
             radius may all be wrong; run dagayn update before concluding anything"
        }
        "graph_built_by_older_extractor" => {
            "an older extractor parsed some files -- qualified names and edges may differ from \
             what the current parser produces; run dagayn update to re-parse them"
        }
        "uncommitted_changes_may_be_unindexed" => {
            "working-tree edits may not be indexed -- a symbol reported missing may exist on disk"
        }
        _ => "claims should be treated as graph-limited until this is resolved",
    }
}
