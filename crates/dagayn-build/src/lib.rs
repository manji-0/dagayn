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
mod vcs;

pub use build::{BuildError, BuildOptions, BuildReport, PostprocessLevel, full_build};
pub use data_dir::{DataDirError, db_path_for_build};
pub use lock::{GraphWriteLock, LockError};
pub use vcs::{Vcs, detect_vcs};
