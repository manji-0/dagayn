//! Post-processing after a build or an update.
//!
//! Port of `dagayn.tools.build._run_postprocess` and the branch of
//! `build_or_update_graph` that drives it for the native store (which has
//! no `_conn`, so Python never takes the single `run_post_processing_json`
//! pass there either). The step
//! order matters (bare-name resolution before demotion, manifest bridges
//! before native bindings), so it follows the Python one call for call.

use std::path::Path;

use dagayn_graph::{GraphError, GraphStore};
use serde_json::{Map, Value, json};

use crate::local_time::local_timestamp;

const MIN_COMMUNITY_SIZE: i64 = 2;
const FLOW_MAX_DEPTH: i64 = 15;
/// A changed file with one of these names can change manifest bridges.
const MANIFEST_FILENAMES: [&str; 3] = ["pyproject.toml", "package.json", "openapitools.json"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostprocessLevel {
    /// Signatures, FTS, edge resolution, flows, communities, summaries.
    Full,
    /// Signatures, FTS, and edge resolution; flows and communities are left
    /// as they were (`--skip-flows`).
    Minimal,
    /// Raw parse only.
    None,
}

impl PostprocessLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Minimal => "minimal",
            Self::None => "none",
        }
    }
}

/// Step counters (the keys of Python's `PostprocessResult`) and warnings.
#[derive(Debug, Default)]
pub(crate) struct PostprocessOutcome {
    pub counters: Map<String, Value>,
    pub warnings: Vec<String>,
}

impl PostprocessOutcome {
    fn set(&mut self, key: &str, value: i64) {
        self.counters.insert(key.to_string(), json!(value));
    }

    /// Run one step; a failure becomes a warning, as each Python step's
    /// `except` clause turns it into one, and the next step still runs.
    fn step<T>(&mut self, label: &str, f: impl FnOnce() -> Result<T, GraphError>) -> Option<T> {
        match f() {
            Ok(value) => Some(value),
            Err(err) => {
                self.warnings.push(format!("{label} failed: {err}"));
                None
            }
        }
    }
}

/// Post-process after a full rebuild.
/// `run_postprocess` (`run_postprocess_tool`): signatures, then the FTS
/// index, flows, and communities as requested, and `last_postprocessed_at`.
/// The counters in `build_result_payload`'s order; any step's failure is an
/// error, where Python would roll back and warn.
pub fn rerun_postprocess(
    store: &mut GraphStore,
    flows: bool,
    communities: bool,
    fts: bool,
) -> Result<Vec<(&'static str, Value)>, GraphError> {
    let mut out = Vec::new();
    store.compute_missing_signatures()?;
    out.push(("signatures_updated", json!(true)));
    if fts {
        out.push(("fts_indexed", json!(store.rebuild_fts_index()?)));
    }
    if flows {
        let raw = store.rebuild_flows_json(FLOW_MAX_DEPTH, false)?;
        let count = serde_json::from_str::<Value>(&raw)?
            .get("count")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        out.push(("flows_detected", json!(count)));
    }
    if communities {
        let detected = dagayn_postproc::detect_communities_json(store, MIN_COMMUNITY_SIZE)?;
        out.push((
            "communities_detected",
            json!(store.store_communities_json(&detected)?),
        ));
    }
    store.set_metadata("last_postprocessed_at", &local_timestamp())?;
    store.commit()?;
    Ok(out)
}

pub(crate) fn after_full_rebuild(
    repo_root: &Path,
    store: &mut GraphStore,
    level: PostprocessLevel,
    recurse_submodules: bool,
) -> Result<Option<PostprocessOutcome>, GraphError> {
    match level {
        PostprocessLevel::None => Ok(None),
        PostprocessLevel::Full => {
            full_after_rebuild(repo_root, store, recurse_submodules).map(Some)
        }
        PostprocessLevel::Minimal => {
            let mut outcome = PostprocessOutcome::default();
            minimal_steps(repo_root, store, recurse_submodules, None, &mut outcome);
            centrality(store, None, &mut outcome);
            prune_orphans(store, &mut outcome);
            record_level(store, level)?;
            Ok(Some(outcome))
        }
    }
}

/// Post-process after an incremental update that re-parsed `changed_files`.
pub(crate) fn after_update(
    repo_root: &Path,
    store: &mut GraphStore,
    level: PostprocessLevel,
    recurse_submodules: bool,
    changed_files: &[String],
    pre_affected_communities: i64,
) -> Result<Option<PostprocessOutcome>, GraphError> {
    let changed = Some(changed_files);
    let mut outcome = PostprocessOutcome::default();
    match level {
        PostprocessLevel::None => return Ok(None),
        PostprocessLevel::Minimal => {
            minimal_steps(repo_root, store, recurse_submodules, changed, &mut outcome);
            centrality(store, changed, &mut outcome);
            prune_orphans(store, &mut outcome);
        }
        PostprocessLevel::Full => {
            minimal_steps(repo_root, store, recurse_submodules, changed, &mut outcome);
            if let Some(count) = outcome.step("Flow detection", || {
                store.incremental_trace_flows(changed_files, FLOW_MAX_DEPTH)
            }) {
                outcome.set("flows_detected", count);
            }
            let pre_affected = (pre_affected_communities != 0).then_some(pre_affected_communities);
            if let Some(count) = outcome.step("Community detection", || {
                dagayn_postproc::incremental_detect_communities(
                    store,
                    changed_files,
                    MIN_COMMUNITY_SIZE,
                    pre_affected,
                )
            }) {
                outcome.set("communities_detected", count);
            }
            outcome.step("Summary computation", || store.compute_summaries());
            centrality(store, changed, &mut outcome);
            prune_orphans(store, &mut outcome);
        }
    }
    record_level(store, level)?;
    Ok(Some(outcome))
}

/// The full level after a full rebuild, as Python drives it for the native
/// store: the minimal steps, every flow and community from scratch, summaries,
/// then centrality and the orphan sweep.
fn full_after_rebuild(
    repo_root: &Path,
    store: &mut GraphStore,
    recurse_submodules: bool,
) -> Result<PostprocessOutcome, GraphError> {
    let mut outcome = PostprocessOutcome::default();
    minimal_steps(repo_root, store, recurse_submodules, None, &mut outcome);
    if let Some(count) = outcome.step("Flow detection", || {
        let raw = store.rebuild_flows_json(FLOW_MAX_DEPTH, false)?;
        Ok(serde_json::from_str::<Value>(&raw)?
            .get("count")
            .and_then(Value::as_i64)
            .unwrap_or(0))
    }) {
        outcome.set("flows_detected", count);
    }
    if let Some(count) = outcome.step("Community detection", || {
        let detected = dagayn_postproc::detect_communities_json(store, MIN_COMMUNITY_SIZE)?;
        store.store_communities_json(&detected)
    }) {
        outcome.set("communities_detected", count);
    }
    outcome.step("Summary computation", || store.compute_summaries());
    centrality(store, None, &mut outcome);
    prune_orphans(store, &mut outcome);
    record_level(store, PostprocessLevel::Full)?;
    Ok(outcome)
}

/// The minimal steps of `_run_postprocess`, in its order.
fn minimal_steps(
    repo_root: &Path,
    store: &mut GraphStore,
    recurse_submodules: bool,
    changed_files: Option<&[String]>,
    outcome: &mut PostprocessOutcome,
) {
    outcome.step("Signature computation", || {
        store.compute_missing_signatures()
    });
    if let Some(count) = outcome.step("FTS index rebuild", || match changed_files {
        Some(files) if !files.is_empty() => store.sync_fts_for_file_paths(files),
        _ => store.rebuild_fts_index(),
    }) {
        outcome.set("fts_indexed", count);
    }
    if let Some((calls, inheritance)) = outcome.step("Bare-name edge resolution", || {
        Ok((
            store.resolve_bare_call_targets()?,
            store.resolve_bare_inheritance_targets()?,
        ))
    }) {
        outcome.set("bare_call_targets_resolved", calls);
        outcome.set("bare_inheritance_targets_resolved", inheritance);
    }
    if let Some(count) = outcome.step("Rust impl member linking", || {
        store.link_foreign_impl_members()
    }) {
        outcome.set("foreign_impl_members_linked", count);
    }
    if let Some(count) = outcome.step("Terraform module reference resolution", || {
        store.resolve_terraform_module_references()
    }) {
        outcome.set("terraform_module_references_resolved", count);
    }
    if let Some(count) = outcome.step("Unresolved endpoint demotion", || {
        store.demote_unresolved_endpoint_edges()
    }) {
        outcome.set("unresolved_endpoint_edges_demoted", count);
    }
    if let Some((resolved, dropped, re_resolved, still)) = outcome
        .step("Markdown artifact ref resolution", || {
            store.resolve_markdown_artifact_refs()
        })
    {
        outcome.set("markdown_artifact_refs_resolved", resolved);
        outcome.set("markdown_artifact_refs_dropped", dropped);
        outcome.set("markdown_artifact_refs_re_resolved", re_resolved);
        outcome.set("markdown_artifact_refs_still_unresolved", still);
    }
    if let Some((resolved, still)) = outcome.step("Terraform artifact ref resolution", || {
        store.resolve_terraform_artifact_refs()
    }) {
        outcome.set("terraform_artifact_refs_resolved", resolved);
        outcome.set("terraform_artifact_refs_still_unresolved", still);
    }
    if should_scan_manifests(changed_files)
        && let Some((edges, nodes)) = outcome.step("Manifest bridge extraction", || {
            let (nodes_json, edges_json) = manifest_bridges_json(repo_root, recurse_submodules)?;
            let edge_count = serde_json::from_str::<Vec<Value>>(&edges_json)
                .map(|edges| edges.len() as i64)
                .unwrap_or(0);
            let nodes = store.replace_manifest_bridges_json(
                dagayn_postproc::manifest_bridges::EXTRACTOR_ID,
                &nodes_json,
                &edges_json,
            )?;
            Ok((edge_count, nodes))
        })
    {
        outcome.set("manifest_bridges_edges", edges);
        outcome.set("manifest_bridges_nodes", nodes);
    }
    if let Some(count) = outcome.step("Native binding resolution", || {
        store.resolve_native_bindings()
    }) {
        outcome.set("native_bindings_resolved", count);
    }
}

/// Re-parsed files invalidate the hub / bridge score tables wholesale, so every
/// level but `none` recomputes them.
fn centrality(
    store: &mut GraphStore,
    changed: Option<&[String]>,
    outcome: &mut PostprocessOutcome,
) {
    if let Some(scores) = outcome.step("Centrality score persistence", || {
        store.persist_centrality_scores_filtered(changed)
    }) {
        for key in [
            "hub_scores_persisted",
            "bridge_scores_persisted",
            "hub_scores_code_persisted",
            "bridge_scores_code_persisted",
        ] {
            outcome.set(key, scores.get(key).copied().unwrap_or(0));
        }
    }
}

fn prune_orphans(store: &mut GraphStore, outcome: &mut PostprocessOutcome) {
    if let Some(pruned) = outcome.step("Orphaned structure pruning", || {
        dagayn_postproc::prune_orphaned_graph_structures(store)
    }) && !pruned.is_empty()
    {
        outcome
            .counters
            .insert("orphans_pruned".to_string(), json!(pruned));
    }
}

fn record_level(store: &GraphStore, level: PostprocessLevel) -> Result<(), GraphError> {
    store.set_metadata("last_postprocessed_at", &local_timestamp())?;
    store.set_metadata("postprocess_level", level.as_str())
}

/// `_should_scan_manifests`: an update that touched no manifest keeps them.
fn should_scan_manifests(changed_files: Option<&[String]>) -> bool {
    match changed_files {
        Some(files) if !files.is_empty() => files.iter().any(|path| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| MANIFEST_FILENAMES.contains(&name))
        }),
        _ => true,
    }
}

/// Manifest bridge nodes and edges as JSON arrays.
///
/// Scoped to the VCS listing, as the Python build scopes them: a gitignored
/// manifest stored here is pruned as out of scope by the next update.
fn manifest_bridges_json(
    repo_root: &Path,
    recurse_submodules: bool,
) -> Result<(String, String), GraphError> {
    let scope: Option<std::collections::HashSet<String>> =
        dagayn_parser::collect_vcs_scope(repo_root, Some(recurse_submodules))
            .map(|paths| paths.into_iter().collect());
    let discovered =
        dagayn_postproc::manifest_bridges::discover_manifest_bridges(repo_root, scope.as_ref());
    Ok((
        serde_json::to_string(&discovered.nodes)?,
        serde_json::to_string(&discovered.edges)?,
    ))
}
