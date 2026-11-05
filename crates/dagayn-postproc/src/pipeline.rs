//! Full post-build pipeline orchestration for the Rust backend.

use dagayn_graph::{GraphError, GraphStore, Result};
use serde::Serialize;

use crate::communities::{
    detect_communities, detect_communities_from, incremental_detect_communities,
};

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
    pub foreign_impl_members_linked: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terraform_module_references_resolved: Option<i64>,
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
    pub native_bindings_resolved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_boundaries_resolved: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub communities_detected: Option<i64>,
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

/// What the manifest bridge step does in one run.
pub enum ManifestBridges<'a, F>
where
    F: FnOnce() -> Result<(String, String)>,
{
    /// No step: the stored bridges stay and no counter is recorded (an
    /// update that touched no manifest).
    Keep,
    /// Nothing was discovered: the stored bridges stay and both counters
    /// are 0.
    NoneFound,
    /// Replace the stored bridges with the `(nodes_json, edges_json)`
    /// arrays `discover` returns; a discovery error is the step's warning.
    Replace { extractor_id: &'a str, discover: F },
}

/// The post-processing passes, in order: signatures, the FTS index, the
/// edge resolvers, the manifest bridges, and the passes that bind to them.
///
/// This is the one list of passes. `run_post_processing_json` (the Python
/// build and `dagayn watch`) and `dagayn-build`'s post-processing (the Rust
/// CLI) both run it; communities, summaries, and the orphan prune stay with
/// each caller. A failed pass becomes a warning and the next pass still runs.
pub fn run_passes<F>(
    store: &mut GraphStore,
    changed_files: Option<&[String]>,
    manifests: ManifestBridges<'_, F>,
    result: &mut PostprocessResult,
) where
    F: FnOnce() -> Result<(String, String)>,
{
    let warnings = &mut result.warnings;

    result.signatures_computed = record_step(warnings, "Signature computation", || {
        store.compute_missing_signatures()
    });

    result.fts_indexed = record_step(warnings, "FTS index rebuild", || match changed_files {
        Some(files) if !files.is_empty() => store.sync_fts_for_file_paths(files),
        _ => store.rebuild_fts_index(),
    });

    // Two steps, so a failed call resolution still resolves inheritance.
    result.bare_call_targets_resolved =
        record_step(warnings, "Bare-name edge resolution (calls)", || {
            store.resolve_bare_call_targets()
        });

    result.bare_inheritance_targets_resolved =
        record_step(warnings, "Bare-name edge resolution (inheritance)", || {
            store.resolve_bare_inheritance_targets()
        });

    result.foreign_impl_members_linked = record_step(warnings, "Rust impl member linking", || {
        store.link_foreign_impl_members()
    });

    result.terraform_module_references_resolved =
        record_step(warnings, "Terraform module reference resolution", || {
            store.resolve_terraform_module_references()
        });

    result.unresolved_endpoint_edges_demoted =
        record_step(warnings, "Unresolved endpoint demotion", || {
            store.demote_unresolved_endpoint_edges()
        });

    if let Some((resolved, dropped, re_resolved, still_unresolved)) =
        record_step(warnings, "Markdown artifact ref resolution", || {
            store.resolve_markdown_artifact_refs()
        })
    {
        result.markdown_artifact_refs_resolved = Some(resolved);
        result.markdown_artifact_refs_dropped = Some(dropped);
        result.markdown_artifact_refs_re_resolved = Some(re_resolved);
        result.markdown_artifact_refs_still_unresolved = Some(still_unresolved);
    }

    if let Some((resolved, still_unresolved)) =
        record_step(warnings, "Terraform artifact ref resolution", || {
            store.resolve_terraform_artifact_refs()
        })
    {
        result.terraform_artifact_refs_resolved = Some(resolved);
        result.terraform_artifact_refs_still_unresolved = Some(still_unresolved);
    }

    match manifests {
        ManifestBridges::Keep => {}
        ManifestBridges::NoneFound => {
            result.manifest_bridges_edges = Some(0);
            result.manifest_bridges_nodes = Some(0);
        }
        ManifestBridges::Replace {
            extractor_id,
            discover,
        } => {
            if let Some((edges, nodes)) =
                record_step(warnings, "Manifest bridge extraction", || {
                    let (nodes_json, edges_json) = discover()?;
                    let edge_count = serde_json::from_str::<Vec<serde_json::Value>>(&edges_json)
                        .map(|edges| edges.len() as i64)
                        .unwrap_or(0);
                    let nodes = store.replace_manifest_bridges_json(
                        extractor_id,
                        &nodes_json,
                        &edges_json,
                    )?;
                    Ok((edge_count, nodes))
                })
            {
                result.manifest_bridges_edges = Some(edges);
                result.manifest_bridges_nodes = Some(nodes);
            }
        }
    }

    // After the manifest bridges: they name the crates this step binds to.
    result.native_bindings_resolved = record_step(warnings, "Native binding resolution", || {
        store.resolve_native_bindings()
    });

    // After the manifest bridges: they name the worker entry modules.
    result.request_boundaries_resolved =
        record_step(warnings, "Request boundary resolution", || {
            store.resolve_request_boundaries()
        });
}

/// How `run_post_processing_json` runs the manifest bridge step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestMode {
    /// Replace the stored bridges with the given arrays, but treat two empty
    /// arrays as nothing found and leave the stored bridges (`dagayn watch`).
    Auto,
    /// Replace the stored bridges with the given arrays, even empty ones.
    Replace,
    /// Leave the stored bridges and record no counter.
    Keep,
}

/// `run_passes`, then communities unless `min_community_size` is `None`;
/// the counters and warnings as `PostprocessResult` JSON.
pub fn run_post_processing_json(
    store: &mut GraphStore,
    manifest_extractor_id: &str,
    manifest_nodes_json: &str,
    manifest_edges_json: &str,
    manifest_mode: ManifestMode,
    min_community_size: Option<i64>,
    changed_files: Option<&[String]>,
) -> Result<String> {
    let mut result = PostprocessResult::default();

    let discover = || {
        Ok((
            manifest_nodes_json.to_string(),
            manifest_edges_json.to_string(),
        ))
    };
    let manifests = match manifest_mode {
        ManifestMode::Keep => ManifestBridges::Keep,
        ManifestMode::Auto if manifest_nodes_json == "[]" && manifest_edges_json == "[]" => {
            ManifestBridges::NoneFound
        }
        ManifestMode::Auto | ManifestMode::Replace => ManifestBridges::Replace {
            extractor_id: manifest_extractor_id,
            discover,
        },
    };
    run_passes(store, changed_files, manifests, &mut result);

    let Some(min_size) = min_community_size else {
        return serde_json::to_string(&result).map_err(GraphError::from);
    };

    let incremental = matches!(changed_files, Some(files) if !files.is_empty());
    let loaded = if incremental {
        None
    } else {
        record_step(&mut result.warnings, "Load graph snapshot", || {
            Ok((store.get_all_nodes_filtered(true)?, store.get_all_edges()?))
        })
    };

    if let Some(community_count) =
        record_step(
            &mut result.warnings,
            "Community detection",
            || match changed_files {
                Some(files) if !files.is_empty() => {
                    incremental_detect_communities(store, files, min_size, None)
                }
                _ => {
                    let communities = if let Some((nodes, edges)) = &loaded {
                        detect_communities_from(nodes, edges, min_size)
                    } else {
                        detect_communities(store, min_size)?
                    };
                    let payload = serde_json::to_string(&communities).map_err(GraphError::from)?;
                    store.store_communities_json(&payload)
                }
            },
        )
    {
        result.communities_detected = Some(community_count);
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

        let raw = run_post_processing_json(
            &mut store,
            "manifest_bridges",
            "[]",
            "[]",
            ManifestMode::Auto,
            Some(2),
            None,
        )
        .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert!(payload.get("signatures_computed").is_some());
        assert!(payload.get("flows_detected").is_none());
        assert!(payload.get("communities_detected").is_some());
        assert!(payload.get("hub_scores_persisted").is_none());
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
            ManifestMode::Auto,
            Some(2),
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
