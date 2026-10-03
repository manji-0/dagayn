//! `dagayn status`.

use std::process::ExitCode;

use clap::Args;
use dagayn_build::{
    GraphLock, LockMode, Vcs, db_path_for_build, detect_vcs, is_linked_worktree, status_lines,
};
use dagayn_graph::GraphStore;

use crate::{resolve_repo_root, unsupported, write_lock_timeout};

#[derive(Args)]
pub(crate) struct StatusArgs {
    /// Repository root (auto-detected).
    #[arg(long)]
    repo: Option<std::path::PathBuf>,
}

pub(crate) fn run(args: &StatusArgs) -> Result<ExitCode, String> {
    let repo_root = resolve_repo_root(args.repo.as_deref())?;
    match detect_vcs(&repo_root) {
        Vcs::Jj => return Err(unsupported("status in a jj workspace")),
        Vcs::Svn => return Err(unsupported("status in an SVN working copy")),
        Vcs::Git | Vcs::None => {}
    }
    let db_path = db_path_for_build(&repo_root).map_err(|err| err.to_string())?;
    if is_linked_worktree(&repo_root) && !db_path.is_file() {
        // Python seeds a new worktree's graph from the main checkout first.
        return Err(unsupported("seeding a linked worktree's first graph"));
    }
    let _lock = GraphLock::acquire_mode(&db_path, LockMode::Shared, Some(write_lock_timeout()))
        .map_err(|err| err.to_string())?;
    let store = GraphStore::open(&db_path).map_err(|err| err.to_string())?;
    for line in status_lines(&repo_root, &store).map_err(|err| err.to_string())? {
        println!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}
