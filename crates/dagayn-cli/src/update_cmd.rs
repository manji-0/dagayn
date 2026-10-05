//! `dagayn update`.

use std::path::Path;

use clap::Args;
use dagayn_build::{
    GraphLock, LockError, LockMode, UpdateOptions, db_path_for_build, is_linked_worktree,
};
use dagayn_graph::GraphStore;

use crate::hook::{
    HOOK_SKIP_MARKER, hook_updates_disabled, running_from_hook, start_budget_watchdog,
    update_budget,
};
use crate::{
    Failure, env_flag, open_matching_graph, postprocess_level, print_postprocess_summary,
    print_warnings, refuse_local_embedding, require_ported_layout, resolve_repo_root, unsupported,
    write_lock_timeout,
};

#[derive(Args)]
pub(crate) struct UpdateArgs {
    /// Git diff base (default: the commit the graph was built at, falling back to HEAD~1).
    #[arg(long)]
    base: Option<String>,
    /// Repository root (auto-detected).
    #[arg(long)]
    repo: Option<std::path::PathBuf>,
    /// Skip flow/community detection (signatures + FTS only).
    #[arg(long)]
    skip_flows: bool,
    /// Skip all post-processing (raw parse only).
    #[arg(long)]
    skip_postprocess: bool,
    /// Stop the update if it outlives this many seconds. Hook-triggered runs
    /// (DAGAYN_HOOK_UPDATE=1) default to 120s; manual runs are unbounded. Use 0
    /// to disable.
    #[arg(long)]
    budget_seconds: Option<f64>,
    /// Local embeddings (not supported yet).
    #[arg(long, num_args = 0..=1, default_missing_value = "bge-m3")]
    local_embedding: Option<String>,
}

pub(crate) fn run(args: &UpdateArgs) -> Result<u8, Failure> {
    refuse_local_embedding(args.local_embedding.as_deref())?;
    let postprocess = postprocess_level(args.skip_flows, args.skip_postprocess);
    let repo_root = resolve_repo_root(args.repo.as_deref())?;
    require_ported_layout(&repo_root)?;
    let db_path = db_path_for_build(&repo_root)?;
    if is_linked_worktree(&repo_root) && !db_path.is_file() {
        // Python seeds a new worktree's graph from the main checkout first.
        return Err(unsupported("seeding a linked worktree's first graph"));
    }

    let hook = running_from_hook();
    if hook && hook_updates_disabled(&repo_root) {
        println!("Skipped: .dagayn/{HOOK_SKIP_MARKER} disables hook-triggered updates here");
        return Ok(0);
    }
    start_budget_watchdog(update_budget(args.budget_seconds), "update");

    // A hook run must not wait for another writer: it would skip anyway, and
    // waiting turns that skip into a hang the editor sits through.
    let wait = (!hook).then(write_lock_timeout);
    let skipped = || {
        println!(
            "Skipped: another process is writing the graph \
             (hook update must not queue behind it)"
        );
        Ok(0)
    };
    let base = match &args.base {
        Some(base) => base.clone(),
        None => match peek_base(&db_path, wait) {
            Ok(base) => base,
            Err(LockError::Busy { .. }) if hook => return skipped(),
            Err(err) => return Err(err.to_string().into()),
        },
    };
    let _lock = match GraphLock::acquire_mode(&db_path, LockMode::Exclusive, wait) {
        Ok(lock) => lock,
        Err(LockError::Busy { .. }) if hook => return skipped(),
        Err(err) => return Err(err.to_string().into()),
    };
    let mut store = open_matching_graph(&db_path, &repo_root)?;
    let options = UpdateOptions {
        base,
        postprocess,
        recurse_submodules: env_flag("CRG_RECURSE_SUBMODULES"),
    };
    let report = dagayn_build::incremental_update(&repo_root, &mut store, &options)?;
    drop(store);

    println!(
        "Incremental: {} files updated, {} nodes, {} edges (postprocess={})",
        report.files_updated,
        report.total_nodes,
        report.total_edges,
        postprocess.as_str()
    );
    if report.files_updated > 0
        && let Some(counters) = &report.postprocess
    {
        print_postprocess_summary(counters);
    }
    print_warnings(&report.warnings);
    Ok(0)
}

/// The commit the graph was built at, read under the shared lock; `HEAD~1`
/// for a graph that never recorded one.
fn peek_base(db_path: &Path, wait: Option<std::time::Duration>) -> Result<String, LockError> {
    let _lock = GraphLock::acquire_mode(db_path, LockMode::Shared, wait)?;
    let stored = GraphStore::open(db_path)
        .ok()
        .and_then(|store| store.get_metadata("git_head_sha").ok().flatten())
        .filter(|sha| !sha.is_empty());
    Ok(stored.unwrap_or_else(|| "HEAD~1".to_string()))
}
