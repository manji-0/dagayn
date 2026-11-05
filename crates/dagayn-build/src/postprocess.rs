//! Post-processing after a build or an update.
//!
//! Port of `dagayn.tools.build._run_postprocess` and the branch of
//! `build_or_update_graph` that drives it. The passes themselves, and their
//! order, are `dagayn_postproc::run_passes`, which the Python path runs too;
//! this module adds the level metadata, communities, summaries, and the
//! orphan prune.

use std::path::Path;

use dagayn_graph::{GraphError, GraphStore};
use dagayn_postproc::{ManifestBridges, PostprocessResult};
use serde_json::{Map, Value, json};

use crate::local_time::local_timestamp;

const MIN_COMMUNITY_SIZE: i64 = 2;
/// A changed file with one of these names can change manifest bridges.
const MANIFEST_FILENAMES: [&str; 6] = [
    "pyproject.toml",
    "package.json",
    "openapitools.json",
    "wrangler.toml",
    "wrangler.json",
    "wrangler.jsonc",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostprocessLevel {
    /// Signatures, FTS, edge resolution, communities, summaries.
    Full,
    /// Signatures, FTS, and edge resolution; communities are left as they
    /// were.
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
/// index and communities as requested, and `last_postprocessed_at`.
/// The counters in `build_result_payload`'s order; any step's failure is an
/// error, where Python would roll back and warn.
pub fn rerun_postprocess(
    store: &mut GraphStore,
    communities: bool,
    fts: bool,
) -> Result<Vec<(&'static str, Value)>, GraphError> {
    let mut out = Vec::new();
    store.compute_missing_signatures()?;
    out.push(("signatures_updated", json!(true)));
    if fts {
        out.push(("fts_indexed", json!(store.rebuild_fts_index()?)));
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
            prune_orphans(store, &mut outcome);
        }
        PostprocessLevel::Full => {
            minimal_steps(repo_root, store, recurse_submodules, changed, &mut outcome);
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
            prune_orphans(store, &mut outcome);
        }
    }
    record_level(store, level)?;
    Ok(Some(outcome))
}

/// The full level after a full rebuild, as Python drives it for the native
/// store: the minimal steps, every community from scratch, summaries,
/// then the orphan sweep.
fn full_after_rebuild(
    repo_root: &Path,
    store: &mut GraphStore,
    recurse_submodules: bool,
) -> Result<PostprocessOutcome, GraphError> {
    let mut outcome = PostprocessOutcome::default();
    minimal_steps(repo_root, store, recurse_submodules, None, &mut outcome);
    if let Some(count) = outcome.step("Community detection", || {
        let detected = dagayn_postproc::detect_communities_json(store, MIN_COMMUNITY_SIZE)?;
        store.store_communities_json(&detected)
    }) {
        outcome.set("communities_detected", count);
    }
    outcome.step("Summary computation", || store.compute_summaries());
    prune_orphans(store, &mut outcome);
    record_level(store, PostprocessLevel::Full)?;
    Ok(outcome)
}

/// The minimal steps: `dagayn_postproc::run_passes`, with the manifest
/// bridges scanned from the VCS listing unless an update touched no
/// manifest. The counters go to `outcome` without `signatures_computed`,
/// which the build reply does not carry.
fn minimal_steps(
    repo_root: &Path,
    store: &mut GraphStore,
    recurse_submodules: bool,
    changed_files: Option<&[String]>,
    outcome: &mut PostprocessOutcome,
) {
    let manifests = if should_scan_manifests(changed_files) {
        ManifestBridges::Replace {
            extractor_id: dagayn_postproc::manifest_bridges::EXTRACTOR_ID,
            discover: || manifest_bridges_json(repo_root, recurse_submodules),
        }
    } else {
        ManifestBridges::Keep
    };
    let mut result = PostprocessResult::default();
    dagayn_postproc::run_passes(store, changed_files, manifests, &mut result);
    outcome.warnings.append(&mut result.warnings);
    result.signatures_computed = None;
    if let Ok(Value::Object(counters)) = serde_json::to_value(&result) {
        outcome.counters.extend(counters);
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
pub(crate) fn should_scan_manifests(changed_files: Option<&[String]>) -> bool {
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
