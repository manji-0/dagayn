//! `dagayn build`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use dagayn_build::{BuildOptions, GraphWriteLock, db_path_for_build};
use dagayn_graph::GraphStore;

use crate::{
    env_flag, postprocess_level, print_postprocess_summary, print_warnings, resolve_repo_root,
    unsupported, write_lock_timeout,
};

#[derive(Args)]
pub(crate) struct BuildArgs {
    /// Repository root (auto-detected).
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Delete the existing graph database before rebuilding.
    #[arg(long = "force-full-build", visible_alias = "force")]
    force: bool,
    /// Skip flow/community detection (signatures + FTS only).
    #[arg(long)]
    skip_flows: bool,
    /// Skip all post-processing (raw parse only).
    #[arg(long)]
    skip_postprocess: bool,
    /// Settle call targets with SCIP indexers (not supported yet).
    #[arg(long)]
    scip: bool,
    /// Local embeddings (not supported yet).
    #[arg(long, num_args = 0..=1, default_missing_value = "bge-m3")]
    local_embedding: Option<String>,
}

pub(crate) fn run(args: &BuildArgs) -> Result<ExitCode, String> {
    if args.scip {
        return Err(unsupported("--scip"));
    }
    if args
        .local_embedding
        .as_deref()
        .is_some_and(|mode| mode != "none")
    {
        return Err(unsupported("--local-embedding"));
    }
    let postprocess = postprocess_level(args.skip_flows, args.skip_postprocess);

    let repo_root = resolve_repo_root(args.repo.as_deref())?;
    let db_path = db_path_for_build(&repo_root).map_err(|err| err.to_string())?;
    let _lock =
        GraphWriteLock::acquire(&db_path, write_lock_timeout()).map_err(|err| err.to_string())?;
    if args.force {
        remove_database(&db_path)?;
    }
    let mut store = GraphStore::open(&db_path).map_err(|err| err.to_string())?;
    if let Some(recorded) = dagayn_build::graph_repo_mismatch(&store, &repo_root) {
        return Err(dagayn_build::graph_repo_mismatch_message(
            &db_path, &recorded, &repo_root,
        ));
    }
    let options = BuildOptions {
        recurse_submodules: env_flag("CRG_RECURSE_SUBMODULES"),
        postprocess,
    };
    let report = dagayn_build::full_build(&repo_root, &mut store, &options)
        .map_err(|err| err.to_string())?;
    drop(store);

    println!(
        "Full build: {} files, {} nodes, {} edges (postprocess={})",
        report.files_parsed,
        report.total_nodes,
        report.total_edges,
        postprocess.as_str()
    );
    if !report.errors.is_empty() {
        println!("Errors: {}", report.errors.len());
    }
    if let Some(counters) = &report.postprocess {
        print_postprocess_summary(counters);
    }
    print_warnings(&report.warnings);
    Ok(ExitCode::SUCCESS)
}

fn remove_database(db_path: &Path) -> Result<(), String> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = db_path.as_os_str().to_os_string();
        name.push(suffix);
        match std::fs::remove_file(PathBuf::from(name)) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("cannot delete {}: {err}", db_path.display())),
        }
    }
    Ok(())
}
