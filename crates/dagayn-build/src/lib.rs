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
mod status;
mod update;
mod vcs;

pub use build::{BuildError, BuildOptions, BuildReport, full_build};
pub use data_dir::{
    DataDirError, db_path_for_build, graph_repo_mismatch, graph_repo_mismatch_message,
};
pub use lock::{GraphLock, GraphWriteLock, LockError, LockMode};
pub use postprocess::PostprocessLevel;
pub use status::{SyncAssessment, assess_graph_sync, status_lines};
pub use update::{UpdateOptions, UpdateReport, incremental_update};
pub use vcs::{Vcs, detect_vcs, is_linked_worktree, main_checkout};
