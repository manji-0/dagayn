//! Full post-build pipeline orchestration for the Rust backend.

use dagayn_graph::{GraphError, GraphStore, Result};
use serde::Serialize;
use serde_json::Value;

use crate::communities::{
    detect_communities, detect_communities_from, incremental_detect_communities,
};

const DEFAULT_FLOW_MAX_DEPTH: i64 = 15;

#[derive(Clone, Debug, Default, Serialize)]
pub struct PostprocessResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signatures_computed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fts_indexed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bare_call_targets_resolved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bare_inheritance_targets_resolved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unresolved_endpoint_edges_demoted: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_artifact_refs_resolved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_artifact_refs_dropped: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_artifact_refs_re_resolved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_artifact_refs_still_unresolved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terraform_artifact_refs_resolved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terraform_artifact_refs_still_unresolved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_bridges_edges: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_bridges_nodes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flows_detected: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub communities_detected: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_scores_persisted: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_scores_persisted: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_scores_code_persisted: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_scores_code_persisted: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn record_step<T>(
    warnings: &mut Vec<String>,
    label: &str,
    f: impl FnOnce() -> Result<T>,
) -> Option<T> {
    match f() {
        Ok(value) => Some(value),
        Err(err) => {
            warnings.push(format!("{label} failed: {err}"));
            None
        }
    }
}

pub fn run_post_processing_json(
    store: &mut GraphStore,
    manifest_extractor_id: &str,
    manifest_nodes_json: &str,
    manifest_edges_json: &str,
    min_community_size: i64,
    changed_files: Option<&[String]>,
) -> Result<String> {
    let mut result = PostprocessResult::default();

    if let Some(count) = record_step(&mut result.warnings, "Signature computation", || {
        store.compute_missing_signatures()
    }) {
        result.signatures_computed = Some(count);
    }

    if let Some(count) =
        record_step(
            &mut result.warnings,
            "FTS index rebuild",
            || match changed_files {
                Some(files) if !files.is_empty() => store.sync_fts_for_file_paths(files),
                _ => store.rebuild_fts_index(),
            },
        )
    {
        result.fts_indexed = Some(count);
    }

    if let Some(count) = record_step(
        &mut result.warnings,
        "Bare-name edge resolution (calls)",
        || store.resolve_bare_call_targets(),
    ) {
        result.bare_call_targets_resolved = Some(count);
    }

    if let Some(count) = record_step(
        &mut result.warnings,
        "Bare-name edge resolution (inheritance)",
        || store.resolve_bare_inheritance_targets(),
    ) {
        result.bare_inheritance_targets_resolved = Some(count);
    }

    if let Some((resolved, dropped, re_resolved, still_unresolved)) = record_step(
        &mut result.warnings,
        "Markdown artifact ref resolution",
        || store.resolve_markdown_artifact_refs(),
    ) {
        result.markdown_artifact_refs_resolved = Some(resolved);
        result.markdown_artifact_refs_dropped = Some(dropped);
        result.markdown_artifact_refs_re_resolved = Some(re_resolved);
        result.markdown_artifact_refs_still_unresolved = Some(still_unresolved);
    }

    if let Some((resolved, still_unresolved)) = record_step(
        &mut result.warnings,
        "Terraform artifact ref resolution",
        || store.resolve_terraform_artifact_refs(),
    ) {
        result.terraform_artifact_refs_resolved = Some(resolved);
        result.terraform_artifact_refs_still_unresolved = Some(still_unresolved);
    }

    if let Some(count) = record_step(&mut result.warnings, "Unresolved endpoint demotion", || {
        store.demote_unresolved_endpoint_edges()
    }) {
        result.unresolved_endpoint_edges_demoted = Some(count);
    }

    if manifest_nodes_json != "[]" || manifest_edges_json != "[]" {
        if let Some(nodes_upserted) =
            record_step(&mut result.warnings, "Manifest bridge extraction", || {
                store.replace_manifest_bridges_json(
                    manifest_extractor_id,
                    manifest_nodes_json,
                    manifest_edges_json,
                )
            })
        {
            let edge_count = serde_json::from_str::<Vec<serde_json::Value>>(manifest_edges_json)
                .map(|edges| edges.len() as i64)
                .unwrap_or(0);
            result.manifest_bridges_edges = Some(edge_count);
            result.manifest_bridges_nodes = Some(nodes_upserted);
        }
    } else {
        result.manifest_bridges_edges = Some(0);
        result.manifest_bridges_nodes = Some(0);
    }

    if let Some(count) = record_step(
        &mut result.warnings,
        "Flow detection",
        || match changed_files {
            Some(files) if !files.is_empty() => {
                store.incremental_trace_flows(files, DEFAULT_FLOW_MAX_DEPTH)
            }
            _ => store
                .rebuild_flows_json(DEFAULT_FLOW_MAX_DEPTH, false)
                .map(|raw| {
                    serde_json::from_str::<Value>(&raw)
                        .ok()
                        .and_then(|payload| payload.get("count").and_then(Value::as_i64))
                        .unwrap_or(0)
                }),
        },
    ) {
        result.flows_detected = Some(count);
    }

    let loaded = record_step(&mut result.warnings, "Load graph snapshot", || {
        Ok((store.get_all_nodes_filtered(true)?, store.get_all_edges()?))
    });

    if let Some(community_count) =
        record_step(
            &mut result.warnings,
            "Community detection",
            || match changed_files {
                Some(files) if !files.is_empty() => {
                    incremental_detect_communities(store, files, min_community_size, None)
                }
                _ => {
                    let communities = if let Some((nodes, edges)) = &loaded {
                        detect_communities_from(nodes, edges, min_community_size)
                    } else {
                        detect_communities(store, min_community_size)?
                    };
                    let payload = serde_json::to_string(&communities).map_err(GraphError::from)?;
                    store.store_communities_json(&payload)
                }
            },
        )
    {
        result.communities_detected = Some(community_count);
    }

    if let Some(scores) =
        record_step(
            &mut result.warnings,
            "Centrality score persistence",
            || match &loaded {
                Some((nodes, edges)) => {
                    store.persist_centrality_from_graph(nodes, edges, changed_files)
                }
                None => store.persist_centrality_scores_filtered(changed_files),
            },
        )
    {
        result.hub_scores_persisted = scores.get("hub_scores_persisted").copied();
        result.bridge_scores_persisted = scores.get("bridge_scores_persisted").copied();
        result.hub_scores_code_persisted = scores.get("hub_scores_code_persisted").copied();
        result.bridge_scores_code_persisted = scores.get("bridge_scores_code_persisted").copied();
    }

    serde_json::to_string(&result).map_err(GraphError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dagayn_graph::{EdgeInput, NodeInput};
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_db(label: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("dagayn-postproc-{label}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn run_post_processing_json_returns_step_counters() {
        let path = temp_db("pipeline");
        let mut store = GraphStore::open(path.to_string_lossy().to_string()).unwrap();
        let entry = NodeInput {
            kind: "Function".to_string(),
            name: "entry".to_string(),
            file_path: "app.py".to_string(),
            line_start: 1,
            line_end: 3,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({}),
        };
        let leaf = NodeInput {
            kind: "Function".to_string(),
            name: "leaf".to_string(),
            file_path: "app.py".to_string(),
            line_start: 4,
            line_end: 6,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({}),
        };
        let call = EdgeInput {
            kind: "CALLS".to_string(),
            source: "app.py::entry".to_string(),
            target: "app.py::leaf".to_string(),
            file_path: "app.py".to_string(),
            line: 2,
            extra: json!({}),
        };
        store
            .store_file_batch(&[(
                "app.py".to_string(),
                vec![entry, leaf],
                vec![call],
                "hash".to_string(),
                0,
            )])
            .unwrap();

        let raw =
            run_post_processing_json(&mut store, "manifest_bridges", "[]", "[]", 2, None).unwrap();
        let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert!(payload.get("signatures_computed").is_some());
        assert!(payload.get("flows_detected").is_some());
        assert!(payload.get("communities_detected").is_some());
        assert!(payload.get("hub_scores_persisted").is_some());
    }

    #[test]
    fn incremental_postprocess_fts_does_not_drop_unchanged_files() {
        let path = temp_db("pipeline-fts");
        let mut store = GraphStore::open(path.to_string_lossy().to_string()).unwrap();
        let alpha = NodeInput {
            kind: "Function".to_string(),
            name: "alpha_widget".to_string(),
            file_path: "src/a.py".to_string(),
            line_start: 1,
            line_end: 2,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({}),
        };
        let beta = NodeInput {
            kind: "Function".to_string(),
            name: "beta_gadget".to_string(),
            file_path: "src/b.py".to_string(),
            line_start: 1,
            line_end: 2,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({}),
        };
        store
            .store_file_batch(&[
                (
                    "src/a.py".to_string(),
                    vec![alpha],
                    vec![],
                    "hash-a".to_string(),
                    0,
                ),
                (
                    "src/b.py".to_string(),
                    vec![beta],
                    vec![],
                    "hash-b".to_string(),
                    0,
                ),
            ])
            .unwrap();
        store.rebuild_fts_index().unwrap();

        let changed = vec!["src/a.py".to_string()];
        let raw = run_post_processing_json(
            &mut store,
            "manifest_bridges",
            "[]",
            "[]",
            2,
            Some(&changed),
        )
        .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            payload.get("fts_indexed").and_then(|v| v.as_i64()),
            Some(1),
            "incremental FTS should reindex only nodes in changed files"
        );
        let _ = std::fs::remove_file(&path);
    }
}
