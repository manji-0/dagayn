//! Graph builds without Python: file discovery, parse-and-store, graph
//! metadata, and post-processing, in the order `dagayn build` runs them.
//!
//! The Python package drives the same steps through `dagayn._core`; this crate
//! is what the standalone `dagayn` binary links instead.

mod build;
mod data_dir;
mod local_time;
mod lock;
pub mod parse_batch;
mod postprocess;
mod project_root;
mod status;
mod update;
mod vcs;

pub use build::{BuildError, BuildOptions, BuildReport, full_build};
pub use data_dir::{
    DataDirError, db_path_for_build, graph_repo_mismatch, graph_repo_mismatch_message,
};
pub use lock::{GraphLock, GraphWriteLock, LockError, LockMode};
pub use postprocess::{PostprocessLevel, rerun_postprocess};
pub use project_root::{ProjectRoot, find_project_root, unsafe_root_reason};
pub use status::{
    CommitFreshness, SyncAssessment, assess_graph_sync, commit_tier_freshness,
    embedding_refresh_skips, openai_names_match, resolve_active_embedding_provider, status_lines,
};
pub use update::{UpdateOptions, UpdateReport, incremental_update};
pub use vcs::{
    ChangeSources, Vcs, change_file_sources, detect_vcs, is_linked_worktree, main_checkout,
    staged_and_unstaged,
};
