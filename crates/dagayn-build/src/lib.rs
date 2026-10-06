//! Graph builds without Python: file discovery, parse-and-store, graph
//! metadata, and post-processing, in the order `dagayn build` runs them.
//!
//! The Python package drives the same steps through `dagayn._core`; this crate
//! is what the standalone `dagayn` binary links instead.

mod build;
mod data_dir;
pub mod jj;
mod local_time;
mod lock;
pub mod parse_batch;
mod postprocess;
mod project_root;
mod pyerr;
mod status;
pub mod svn;
pub mod task_queue;
mod update;
mod vcs;

pub use build::{BuildError, BuildOptions, BuildReport, full_build};
pub use data_dir::{
    DataDirError, db_path_for_build, existing_db_path, graph_repo_mismatch,
    graph_repo_mismatch_message, repo_slug,
};
pub use lock::{GraphLock, GraphWriteLock, LockError, LockMode};
pub use postprocess::{PostprocessLevel, rerun_postprocess};
pub use project_root::{ProjectRoot, find_project_root, unsafe_root_reason};
pub use status::{
    CommitFreshness, EmbeddingRefresh, SyncAssessment, assess_graph_sync, clear_seed_verification,
    commit_tier_freshness, embedding_refresh_action, embedding_refresh_skips, openai_names_match,
    resolve_active_embedding_provider, status_lines,
};
pub use update::{UpdateOptions, UpdateReport, incremental_update};
pub use vcs::{
    ChangeError, ChangeSources, Vcs, change_file_sources, default_review_base, detect_vcs,
    diff_stamp_error, is_linked_worktree, is_safe_git_ref, main_checkout, staged_and_unstaged,
};
