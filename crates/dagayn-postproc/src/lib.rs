//! Rust post-processing: flow tracing, community detection, and future targets.

mod communities;
mod entry_points;
mod flows;
mod pipeline;
mod search;

pub use communities::{
    detect_communities, detect_communities_json, incremental_detect_communities,
    refresh_community_stats_json, DetectedCommunity,
};
pub use flows::{
    incremental_trace_flows_json, refresh_flow_criticalities, trace_flows, trace_flows_json,
    TraceOptions, TracedFlow, DEFAULT_FLOW_MAX_DEPTH, DEFAULT_FLOW_MAX_NODES,
};
pub use pipeline::{run_post_processing_json, PostprocessResult};
pub use search::{hybrid_search_json, rrf_merge};

pub fn phase() -> u8 {
    2
}
