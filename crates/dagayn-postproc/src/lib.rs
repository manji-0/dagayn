//! Rust post-processing: Leiden communities and the full post-build pipeline.
//!
//! Flow tracing lives in `dagayn-graph` (`flow_trace`) so reverse-CALLS
//! incremental updates stay on one BFS implementation.

mod communities;
mod pipeline;
mod prune;

pub use communities::{
    DetectedCommunity, detect_communities, detect_communities_json, incremental_detect_communities,
    refresh_community_stats_json,
};
pub use pipeline::{PostprocessResult, run_post_processing_json};
pub use prune::{prune_orphaned_graph_structures, prune_orphaned_graph_structures_json};

pub fn phase() -> u8 {
    2
}
