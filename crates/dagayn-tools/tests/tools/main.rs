//! The native tools against a graph built in Rust: what they answer, and
//! where they leave the call to Python (`None`). The pytest suite
//! (`tests/test_mcp_frontend.py`) checks the answers equal fastmcp's.

mod ambiguous;
mod architecture;
mod common;
mod docs;
mod ensure_graph;
mod flow;
mod go_outside_module;
mod minimal_context;
mod postprocess;
mod query_graph;
mod refactor;
mod reply_contract;
mod request_boundary;
mod review;
mod search;
mod stats;
