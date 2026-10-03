//! `dagayn update`: re-parse what changed since the graph's commit.
//!
//! Port of `dagayn.incremental_update_pipeline` (prepare, classify, plan,
//! apply, parse, finalize) and the incremental branch of
//! `build_or_update_graph`. Python walks several of these path sets in hash
//! order; here they are sorted, so the same change stores the same rows in
//! the same order.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use dagayn_graph::{FileBatchItem, GraphError, GraphStore};
use serde_json::Value;

use crate::build::{BuildError, extractor_versions_stamp, owns_path};
use crate::local_time::local_timestamp;
use crate::parse_batch::{
    CachedRustChangedFile, classify_changed_rust_owned_file_batch,
    collect_changed_rust_owned_file_batch, collect_rust_owned_file_batch,
    collect_unowned_file_batch, file_mtime_ns, sha256_hex,
};
use crate::postprocess::{PostprocessLevel, after_update};
use crate::vcs::{Vcs, changed_files, dedupe, detect_vcs, git_branch_info, resolve_commit_sha};

const PARSE_BATCH_SIZE: usize = 500;
/// Match the Rust writer: drop and rebuild indexes only for large batches.
const BULK_LOAD_FILE_THRESHOLD: usize = 64;
const MAX_DEPENDENT_FILES: usize = 500;
const EXTRACTOR_VERSIONS_KEY: &str = "extractor_versions";

#[derive(Clone, Debug)]
pub struct UpdateOptions {
    /// Git ref to diff against; normally the commit the graph was built at.
    pub base: String,
    pub postprocess: PostprocessLevel,
    pub recurse_submodules: bool,
}

#[derive(Debug, Default)]
pub struct UpdateReport {
    /// Changed, removed, and dependent files the update looked at.
    pub files_updated: usize,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub changed_files: Vec<String>,
    pub dependent_files: Vec<String>,
    /// `(file, error)` for files that failed to read or parse.
    pub errors: Vec<(String, String)>,
    pub postprocess: Option<Value>,
    pub warnings: Vec<String>,
}

/// Bring the graph of `repo_root` up to date with its working tree.
pub fn incremental_update(
    repo_root: &Path,
    store: &mut GraphStore,
    options: &UpdateOptions,
) -> Result<UpdateReport, BuildError> {
    match detect_vcs(repo_root) {
        Vcs::Jj => return Err(BuildError::UnsupportedVcs("jj")),
        Vcs::Svn => return Err(BuildError::UnsupportedVcs("svn")),
        Vcs::Git | Vcs::None => {}
    }
    let changed = changed_files(repo_root, &options.base);
    let pre_affected = if changed.is_empty() {
        0
    } else {
        store.count_affected_communities(&changed)?
    };
    let mut report = run_pipeline(repo_root, store, options, changed)?;

    if report.files_updated > 0
        && let Some(outcome) = after_update(
            repo_root,
            store,
            options.postprocess,
            options.recurse_submodules,
            &report.changed_files,
            pre_affected,
        )?
    {
        report.postprocess = Some(Value::Object(outcome.counters));
        report.warnings.extend(outcome.warnings);
    }
    if options.postprocess != PostprocessLevel::None {
        store.prune_orphaned_embeddings()?;
    }
    Ok(report)
}

#[derive(Default)]
struct Plan {
    removed: Vec<String>,
    mtime_only: Vec<(String, i64)>,
    /// Unchanged content, older extractor: parsed without a hash check.
    rust_full: Vec<String>,
    /// Content known to have changed; their bytes are cached.
    rust_forced: Vec<String>,
    /// Possibly changed: hashed against the stored hash before parsing.
    rust_checked: Vec<String>,
    unowned: Vec<String>,
    cached: HashMap<String, CachedRustChangedFile>,
}

impl Plan {
    fn parse_count(&self) -> usize {
        self.rust_full.len() + self.rust_forced.len() + self.rust_checked.len() + self.unowned.len()
    }
}

fn run_pipeline(
    repo_root: &Path,
    store: &mut GraphStore,
    options: &UpdateOptions,
    changed: Vec<String>,
) -> Result<UpdateReport, BuildError> {
    store.set_metadata("repo_root", &repo_root.to_string_lossy())?;
    let ignore_patterns = dagayn_parser::load_ignore_patterns(repo_root);
    let diff_covers_graph = diff_covers_graph_commit(repo_root, store, &options.base)?;
    let (indexable, stale_scope) = indexable_scope(repo_root, store, options.recurse_submodules)?;
    let (outdated, extractor_files) = extractor_reparse_scope(repo_root, store, &indexable)?;
    if !outdated.is_empty() && extractor_files.is_empty() {
        // Nothing the outdated extractors own is indexed: only the stamp is stale.
        store.set_metadata(EXTRACTOR_VERSIONS_KEY, &extractor_versions_stamp())?;
        store.commit()?;
    }
    if changed.is_empty() && stale_scope.is_empty() && extractor_files.is_empty() {
        record_head_when_verified(repo_root, store, diff_covers_graph)?;
        return Ok(UpdateReport::default());
    }

    let mut report = UpdateReport {
        changed_files: expand_changed_submodules(repo_root, changed),
        ..UpdateReport::default()
    };
    let mut plan = Plan::default();

    // Classify: which changed paths still exist, and whose content changed.
    let changed_set: BTreeSet<String> = report.changed_files.iter().cloned().collect();
    let (candidates, mut removed) =
        filter_candidates(repo_root, &changed_set, &ignore_patterns, &indexable);
    removed = indexed_only(store, removed)?;
    removed = dedupe(removed.into_iter().chain(stale_scope));

    let (rust_candidates, unowned_candidates): (Vec<String>, Vec<String>) = candidates
        .into_iter()
        .partition(|path| owns_path(repo_root, path));
    let mut content_changed: BTreeSet<String> = BTreeSet::new();
    let mut rust_content_changed: HashSet<String> = HashSet::new();
    if !rust_candidates.is_empty() {
        let meta = store.get_file_meta_for_files(&rust_candidates)?;
        let (changed, mtime_updates, errors) =
            classify_changed_rust_owned_file_batch(repo_root, rust_candidates, &meta);
        if !mtime_updates.is_empty() {
            store.update_file_mtimes(&mtime_updates)?;
        }
        for (path, cached) in changed {
            rust_content_changed.insert(path.clone());
            content_changed.insert(path.clone());
            plan.cached.insert(path, cached);
        }
        report.errors.extend(errors);
    }
    if !unowned_candidates.is_empty() {
        let meta = store.get_file_meta_for_files(&unowned_candidates)?;
        for path in unowned_candidates {
            match content_state(repo_root, &path, meta.get(&path), false) {
                ContentState::Unchanged => {}
                ContentState::MtimeOnly(mtime_ns) => plan.mtime_only.push((path, mtime_ns)),
                ContentState::Changed => {
                    content_changed.insert(path);
                }
            }
        }
    }

    let roots: BTreeSet<String> = removed
        .iter()
        .cloned()
        .chain(content_changed.iter().cloned())
        .collect();
    report.dependent_files = find_dependents(store, &roots)?;
    let mut all_files: BTreeSet<String> = content_changed
        .iter()
        .chain(&removed)
        .chain(&report.dependent_files)
        .cloned()
        .collect();
    let mut candidates: Vec<String> = if report.dependent_files.is_empty() {
        plan.removed = removed;
        content_changed
            .iter()
            .filter(|path| indexable.contains(*path))
            .cloned()
            .collect()
    } else {
        let (candidates, extra_removed) =
            filter_candidates(repo_root, &all_files, &ignore_patterns, &indexable);
        let extra_removed = indexed_only(store, extra_removed)?;
        plan.removed = dedupe(removed.into_iter().chain(extra_removed));
        candidates
    };

    // Plan: what to parse, and how.
    if !extractor_files.is_empty() {
        all_files.extend(extractor_files.iter().cloned());
        let full: Vec<String> = extractor_files
            .into_iter()
            .filter(|path| !content_changed.contains(path))
            .collect();
        let full_set: HashSet<&String> = full.iter().collect();
        candidates.retain(|path| !full_set.contains(path));
        for path in full {
            if owns_path(repo_root, &path) {
                plan.rust_full.push(path);
            } else {
                plan.unowned.push(path);
            }
        }
    }
    let meta = store.get_file_meta_for_files(&candidates)?;
    for path in candidates {
        let stored = meta.get(&path);
        if owns_path(repo_root, &path) {
            if rust_content_changed.contains(&path) {
                plan.rust_forced.push(path);
            } else if !mtime_matches(repo_root, &path, stored) {
                plan.rust_checked.push(path);
            }
        } else {
            let known_changed = content_changed.contains(&path);
            match content_state(repo_root, &path, stored, !known_changed) {
                ContentState::Unchanged => {}
                ContentState::MtimeOnly(mtime_ns) => plan.mtime_only.push((path, mtime_ns)),
                ContentState::Changed => plan.unowned.push(path),
            }
        }
    }
    report.files_updated = all_files.len();

    // Apply removals and mtime-only updates.
    store.remove_files_data(&plan.removed)?;
    if !plan.removed.is_empty() || !plan.mtime_only.is_empty() {
        store.update_file_mtimes(&plan.mtime_only)?;
        store.commit()?;
    }
    if plan.parse_count() == 0 {
        record_head_when_verified(repo_root, store, diff_covers_graph)?;
        return Ok(report);
    }

    // Parse and store.
    let bulk = plan.parse_count() >= BULK_LOAD_FILE_THRESHOLD;
    if bulk {
        store.begin_bulk_load()?;
    }
    let parsed = parse_planned(repo_root, store, plan, &mut report);
    if bulk {
        store.finish_bulk_load()?;
    }
    parsed?;

    // Finalize.
    store.set_metadata("last_updated", &local_timestamp())?;
    store.set_metadata("last_build_type", "incremental")?;
    store.set_metadata(EXTRACTOR_VERSIONS_KEY, &extractor_versions_stamp())?;
    if diff_covers_graph {
        store_vcs_metadata(repo_root, store)?;
    }
    store.commit()?;
    Ok(report)
}

fn parse_planned(
    repo_root: &Path,
    store: &mut GraphStore,
    mut plan: Plan,
    report: &mut UpdateReport,
) -> Result<(), GraphError> {
    for chunk in plan.rust_full.chunks(PARSE_BATCH_SIZE) {
        let (batch, nodes, edges, errors) =
            collect_rust_owned_file_batch(repo_root, chunk.to_vec());
        store_batch(store, &batch, nodes, edges, errors, report)?;
    }
    for paths in [
        std::mem::take(&mut plan.rust_forced),
        std::mem::take(&mut plan.rust_checked),
    ] {
        if paths.is_empty() {
            continue;
        }
        let meta = store.get_file_meta_for_files(&paths)?;
        let cached: HashMap<String, CachedRustChangedFile> = paths
            .iter()
            .filter_map(|path| plan.cached.remove(path).map(|entry| (path.clone(), entry)))
            .collect();
        let (batch, mtime_updates, nodes, edges, errors) =
            collect_changed_rust_owned_file_batch(repo_root, paths, &meta, cached);
        store.update_file_mtimes(&mtime_updates)?;
        store_batch(store, &batch, nodes, edges, errors, report)?;
    }
    if !plan.unowned.is_empty() {
        let (batch, nodes, edges, errors) = collect_unowned_file_batch(repo_root, plan.unowned);
        store_batch(store, &batch, nodes, edges, errors, report)?;
    }
    Ok(())
}

fn store_batch(
    store: &mut GraphStore,
    batch: &[FileBatchItem],
    nodes: usize,
    edges: usize,
    errors: Vec<(String, String)>,
    report: &mut UpdateReport,
) -> Result<(), GraphError> {
    if !batch.is_empty() {
        store.store_file_batch(batch)?;
    }
    report.total_nodes += nodes;
    report.total_edges += edges;
    report.errors.extend(errors);
    Ok(())
}

/// `_filter_incremental_candidates` restricted to the indexable scope:
/// `(parseable, removed)`, removed including paths outside the scope.
fn filter_candidates(
    repo_root: &Path,
    paths: &BTreeSet<String>,
    ignore_patterns: &[String],
    indexable: &HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    let paths: Vec<String> = paths.iter().cloned().collect();
    let (mut parseable, mut removed) =
        dagayn_parser::filter_incremental_candidates(repo_root, &paths, ignore_patterns);
    parseable.retain(|path| indexable.contains(path));
    removed.extend(paths.into_iter().filter(|path| !indexable.contains(path)));
    (parseable, removed)
}

/// Only the paths the graph holds a file row for.
fn indexed_only(store: &GraphStore, paths: Vec<String>) -> Result<Vec<String>, GraphError> {
    if paths.is_empty() {
        return Ok(paths);
    }
    let indexed = store.get_file_meta_for_files(&paths)?;
    Ok(paths
        .into_iter()
        .filter(|path| indexed.contains_key(path))
        .collect())
}

/// `(indexable scope, graph files outside it)`: the VCS listing minus ignore
/// patterns, or the full parseable walk without one.
fn indexable_scope(
    repo_root: &Path,
    store: &GraphStore,
    recurse_submodules: bool,
) -> Result<(HashSet<String>, Vec<String>), GraphError> {
    let indexable: HashSet<String> =
        dagayn_parser::collect_vcs_scope(repo_root, Some(recurse_submodules))
            .unwrap_or_else(|| {
                dagayn_parser::collect_parseable_files(repo_root, Some(recurse_submodules))
            })
            .into_iter()
            .collect();
    let mut stale: Vec<String> = store
        .get_all_files()?
        .into_iter()
        .filter(|path| !indexable.contains(path))
        .collect();
    stale.sort();
    Ok((indexable, stale))
}

/// Extractors whose stored version differs from the running parser and that
/// parsed something in a graph of `languages`: `outdated_extractors` in
/// Python. A graph without a stamp counts as version 0.
pub(crate) fn outdated_extractors(
    store: &GraphStore,
    languages: &[String],
) -> Result<Vec<&'static dagayn_parser::ExtractorVersion>, GraphError> {
    let stored: HashMap<String, u32> = store
        .get_metadata(EXTRACTOR_VERSIONS_KEY)?
        .unwrap_or_default()
        .split(',')
        .filter_map(|item| {
            let (name, version) = item.trim().split_once('=')?;
            Some((name.to_string(), version.parse().ok()?))
        })
        .collect();
    let present: HashSet<&str> = languages.iter().map(String::as_str).collect();
    let mut outdated: Vec<&dagayn_parser::ExtractorVersion> = dagayn_parser::extractor_versions()
        .iter()
        .filter(|entry| stored.get(entry.extractor).copied().unwrap_or(0) != entry.version)
        .filter(|entry| entry.languages.iter().any(|lang| present.contains(lang)))
        .collect();
    outdated.sort_by_key(|entry| entry.extractor);
    Ok(outdated)
}

/// The outdated extractors and the indexable files they own: those are
/// re-parsed once even if unchanged.
fn extractor_reparse_scope(
    repo_root: &Path,
    store: &GraphStore,
    indexable: &HashSet<String>,
) -> Result<(Vec<String>, Vec<String>), GraphError> {
    let languages = store.get_stats()?.languages.to_vec();
    let outdated = outdated_extractors(store, &languages)?;
    if outdated.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let owned: HashSet<&str> = outdated
        .iter()
        .flat_map(|entry| entry.languages.iter().copied())
        .collect();
    let mut files: Vec<String> = indexable
        .iter()
        .filter(|path| {
            dagayn_parser::detect_language(&repo_root.join(path))
                .is_some_and(|lang| owned.contains(lang))
        })
        .cloned()
        .collect();
    files.sort();
    let names = outdated
        .iter()
        .map(|entry| entry.extractor.to_string())
        .collect();
    Ok((names, files))
}

/// Files that import from or otherwise depend on `roots`, nearest first,
/// `CRG_DEPENDENT_HOPS` hops out (2 by default) and at most 500 files.
fn find_dependents(
    store: &GraphStore,
    roots: &BTreeSet<String>,
) -> Result<Vec<String>, GraphError> {
    let max_hops: usize = std::env::var("CRG_DEPENDENT_HOPS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(2);
    let mut ordered = Vec::new();
    let mut visited: HashSet<String> = roots.iter().cloned().collect();
    let mut frontier: Vec<String> = roots.iter().cloned().collect();
    for _ in 0..max_hops {
        if frontier.is_empty() {
            break;
        }
        let mut next: Vec<String> = store
            .get_direct_dependents(&frontier)?
            .into_iter()
            .filter(|path| !visited.contains(path))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        visited.extend(next.iter().cloned());
        ordered.append(&mut next.clone());
        if ordered.len() > MAX_DEPENDENT_FILES {
            ordered.truncate(MAX_DEPENDENT_FILES);
            break;
        }
        frontier = std::mem::take(&mut next);
    }
    Ok(ordered)
}

/// A changed submodule shows up as its directory; expand it into the files it
/// tracks so the hash comparison can skip what did not change.
fn expand_changed_submodules(repo_root: &Path, paths: Vec<String>) -> Vec<String> {
    let mut expanded = Vec::new();
    for path in paths {
        let dir = repo_root.join(&path);
        if !dir.is_dir() || !dir.join(".git").exists() {
            expanded.push(path);
            continue;
        }
        let inner = std::process::Command::new("git")
            .args(["ls-files", "-z"])
            .current_dir(&dir)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split('\0')
                    .filter(|name| !name.is_empty())
                    .map(|name| format!("{path}/{name}"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if inner.is_empty() {
            expanded.push(path);
        } else {
            expanded.extend(inner);
        }
    }
    dedupe(expanded)
}

/// `diff base..HEAD` covers everything the graph misses only when `base` is
/// the commit the graph was built at; a graph without one keeps the historical
/// behaviour and counts as covered.
fn diff_covers_graph_commit(
    repo_root: &Path,
    store: &GraphStore,
    base: &str,
) -> Result<bool, GraphError> {
    let Some(resolved) = resolve_commit_sha(repo_root, base) else {
        return Ok(false);
    };
    Ok(match store.get_metadata("git_head_sha")? {
        Some(stored) if !stored.is_empty() => stored == resolved,
        _ => true,
    })
}

fn record_head_when_verified(
    repo_root: &Path,
    store: &GraphStore,
    diff_covers_graph: bool,
) -> Result<(), GraphError> {
    if diff_covers_graph {
        store_vcs_metadata(repo_root, store)?;
        store.commit()?;
    }
    Ok(())
}

fn store_vcs_metadata(repo_root: &Path, store: &GraphStore) -> Result<(), GraphError> {
    if detect_vcs(repo_root) != Vcs::Git {
        return Ok(());
    }
    let (branch, sha) = git_branch_info(repo_root);
    if !branch.is_empty() {
        store.set_metadata("git_branch", &branch)?;
    }
    if !sha.is_empty() {
        store.set_metadata("git_head_sha", &sha)?;
    }
    Ok(())
}

enum ContentState {
    /// The stored mtime matches and it was trusted; nothing was read.
    Unchanged,
    /// Same bytes, new mtime.
    MtimeOnly(i64),
    Changed,
}

/// `_classify_python_changed_files` for one file. An unreadable file counts
/// as changed so the parse reports the error.
fn content_state(
    repo_root: &Path,
    path: &str,
    stored: Option<&(String, i64)>,
    trust_mtime: bool,
) -> ContentState {
    let full = repo_root.join(path);
    let Ok(mtime_ns) = file_mtime_ns(&full) else {
        return ContentState::Changed;
    };
    if trust_mtime && stored.is_some_and(|(_, stored_mtime)| *stored_mtime == mtime_ns) {
        return ContentState::Unchanged;
    }
    match std::fs::read(&full) {
        Ok(source) if stored.is_some_and(|(hash, _)| *hash == sha256_hex(&source)) => {
            ContentState::MtimeOnly(mtime_ns)
        }
        _ => ContentState::Changed,
    }
}

fn mtime_matches(repo_root: &Path, path: &str, stored: Option<&(String, i64)>) -> bool {
    stored.is_some_and(|(_, stored_mtime)| {
        file_mtime_ns(&repo_root.join(path)).is_ok_and(|mtime_ns| mtime_ns == *stored_mtime)
    })
}
