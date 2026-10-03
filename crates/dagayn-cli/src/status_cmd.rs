//! `dagayn status`.

use clap::Args;
use dagayn_build::{GraphLock, LockMode, db_path_for_build, is_linked_worktree, status_lines};
use dagayn_graph::GraphStore;

use crate::{Failure, require_ported_layout, resolve_repo_root, unsupported, write_lock_timeout};

#[derive(Args)]
pub(crate) struct StatusArgs {
    /// Repository root (auto-detected).
    #[arg(long)]
    repo: Option<std::path::PathBuf>,
}

pub(crate) fn run(args: &StatusArgs) -> Result<u8, Failure> {
    let repo_root = resolve_repo_root(args.repo.as_deref())?;
    require_ported_layout(&repo_root)?;
    let db_path = db_path_for_build(&repo_root)?;
    if is_linked_worktree(&repo_root) && !db_path.is_file() {
        // Python seeds a new worktree's graph from the main checkout first.
        return Err(unsupported("seeding a linked worktree's first graph"));
    }
    let _lock = GraphLock::acquire_mode(&db_path, LockMode::Shared, Some(write_lock_timeout()))
        .map_err(|err| err.to_string())?;
    let store = GraphStore::open(&db_path)?;
    for line in status_lines(&repo_root, &store)? {
        println!("{line}");
    }
    Ok(0)
}
