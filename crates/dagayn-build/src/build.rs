//! `dagayn build`: a full rebuild followed by post-processing.
//!
//! Port of `dagayn.incremental_build.full_build` plus the full-rebuild path of
//! `dagayn.tools.build.build_or_update_graph`. Embeddings and the SCIP overlay
//! are not part of this path.

use std::path::Path;

use dagayn_graph::{FileBatchItem, GraphError, GraphStore};
use serde_json::Value;

use crate::local_time::local_timestamp;
use crate::parse_batch::{collect_rust_owned_file_batch, collect_unowned_file_batch};
use crate::vcs::{Vcs, detect_vcs, git_branch_info};

/// Files parsed and stored per batch; `DAGAYN_RUST_PARSE_BATCH_SIZE` in Python.
const PARSE_BATCH_SIZE: usize = 500;
const MIN_COMMUNITY_SIZE: i64 = 2;
const EXTRACTOR_VERSIONS_KEY: &str = "extractor_versions";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostprocessLevel {
    /// Signatures, FTS, edge resolution, flows, communities, summaries.
    Full,
    /// Raw parse only.
    None,
}

impl PostprocessLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub recurse_submodules: bool,
    pub postprocess: PostprocessLevel,
}

#[derive(Debug, Default)]
pub struct BuildReport {
    pub files_parsed: usize,
    pub total_nodes: usize,
    pub total_edges: usize,
    /// `(file, error)` for files that failed to read or parse.
    pub errors: Vec<(String, String)>,
    /// Post-processing step counters, as `run_post_processing_json` returns them.
    pub postprocess: Option<Value>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("{0} working copies are not supported by this build of dagayn yet")]
    UnsupportedVcs(&'static str),
    #[error(transparent)]
    Graph(#[from] GraphError),
}

/// Rebuild the graph of `repo_root` into `store` and post-process it.
pub fn full_build(
    repo_root: &Path,
    store: &mut GraphStore,
    options: &BuildOptions,
) -> Result<BuildReport, BuildError> {
    let vcs = detect_vcs(repo_root);
    match vcs {
        Vcs::Jj => return Err(BuildError::UnsupportedVcs("jj")),
        Vcs::Svn => return Err(BuildError::UnsupportedVcs("svn")),
        Vcs::Git | Vcs::None => {}
    }
    store.set_metadata("repo_root", &repo_root.to_string_lossy())?;
    let files = dagayn_parser::collect_parseable_files(repo_root, Some(options.recurse_submodules));

    let current: std::collections::HashSet<&str> = files.iter().map(String::as_str).collect();
    let stale: Vec<String> = store
        .get_all_files()?
        .into_iter()
        .filter(|path| !current.contains(path.as_str()))
        .collect();
    store.remove_files_data(&stale)?;
    if !stale.is_empty() {
        store.commit()?;
    }

    let mut report = BuildReport {
        files_parsed: files.len(),
        ..BuildReport::default()
    };
    store.begin_bulk_load()?;
    let stored = parse_and_store(repo_root, store, files, &mut report);
    store.finish_bulk_load()?;
    stored?;

    store.set_metadata("last_updated", &local_timestamp())?;
    store.set_metadata("last_build_type", "full")?;
    if vcs == Vcs::Git {
        let (branch, sha) = git_branch_info(repo_root);
        if !branch.is_empty() {
            store.set_metadata("git_branch", &branch)?;
        }
        if !sha.is_empty() {
            store.set_metadata("git_head_sha", &sha)?;
        }
    }
    store.set_metadata(EXTRACTOR_VERSIONS_KEY, &extractor_versions_stamp())?;
    store.commit()?;

    if options.postprocess == PostprocessLevel::Full {
        postprocess_full(repo_root, store, &mut report)?;
    }
    if options.postprocess != PostprocessLevel::None {
        store.prune_orphaned_embeddings()?;
    }
    Ok(report)
}

/// Parse in path-ownership order: Rust-owned files first, then the rest, as
/// the Python build does. Node ids follow storage order.
fn parse_and_store(
    repo_root: &Path,
    store: &mut GraphStore,
    files: Vec<String>,
    report: &mut BuildReport,
) -> Result<(), GraphError> {
    let (owned, unowned): (Vec<String>, Vec<String>) = files
        .into_iter()
        .partition(|path| owns_path(repo_root, path));
    for chunk in owned.chunks(PARSE_BATCH_SIZE) {
        let summary = collect_rust_owned_file_batch(repo_root, chunk.to_vec());
        store_summary(store, summary, report)?;
    }
    if !unowned.is_empty() {
        store_summary(
            store,
            collect_unowned_file_batch(repo_root, unowned),
            report,
        )?;
    }
    Ok(())
}

/// `dagayn.incremental_build._rust_parser_owns_path`: by extension, or by
/// content for an extensionless script.
fn owns_path(repo_root: &Path, rel_path: &str) -> bool {
    if dagayn_parser::rust_parser_owns_path(rel_path) {
        return true;
    }
    if Path::new(rel_path).extension().is_some() {
        return false;
    }
    std::fs::read(repo_root.join(rel_path))
        .map(|source| dagayn_parser::rust_parser_owns_source(rel_path, &source))
        .unwrap_or(false)
}

fn store_summary(
    store: &mut GraphStore,
    (batch, nodes, edges, errors): (Vec<FileBatchItem>, usize, usize, Vec<(String, String)>),
    report: &mut BuildReport,
) -> Result<(), GraphError> {
    if !batch.is_empty() {
        store.store_file_batch(&batch)?;
    }
    report.total_nodes += nodes;
    report.total_edges += edges;
    report.errors.extend(errors);
    Ok(())
}

/// `name=version` pairs sorted by name: `format_extractor_versions` in Python.
fn extractor_versions_stamp() -> String {
    let mut pairs: Vec<(&str, u32)> = dagayn_parser::extractor_versions()
        .iter()
        .map(|entry| (entry.extractor, entry.version))
        .collect();
    pairs.sort_unstable();
    pairs
        .into_iter()
        .map(|(name, version)| format!("{name}={version}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn postprocess_full(
    repo_root: &Path,
    store: &mut GraphStore,
    report: &mut BuildReport,
) -> Result<(), BuildError> {
    let (extractor_id, nodes_json, edges_json) = manifest_bridges(repo_root);
    let raw = dagayn_postproc::run_post_processing_json(
        store,
        extractor_id,
        &nodes_json,
        &edges_json,
        MIN_COMMUNITY_SIZE,
        None,
    )?;
    let mut payload: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    if let Some(warnings) = payload
        .as_object_mut()
        .and_then(|object| object.remove("warnings"))
        .and_then(|value| value.as_array().cloned())
    {
        report.warnings.extend(
            warnings
                .iter()
                .filter_map(|w| w.as_str().map(str::to_string)),
        );
    }
    report.postprocess = Some(payload);

    if let Err(err) = dagayn_postproc::prune_orphaned_graph_structures(store) {
        report
            .warnings
            .push(format!("Orphaned structure pruning failed: {err}"));
    }
    if let Err(err) = store.compute_summaries() {
        report
            .warnings
            .push(format!("Summary computation failed: {err}"));
    }
    store.set_metadata("last_postprocessed_at", &local_timestamp())?;
    store.set_metadata("postprocess_level", PostprocessLevel::Full.as_str())?;
    Ok(())
}

/// Manifest bridge nodes and edges as JSON for `run_post_processing_json`.
fn manifest_bridges(_repo_root: &Path) -> (&'static str, String, String) {
    // Filled in by the Rust port of `dagayn.parser.manifest_bridges`.
    ("manifest_bridges", "[]".to_string(), "[]".to_string())
}
