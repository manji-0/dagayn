//! `run_postprocess_tool` (`dagayn.tools.build.run_postprocess`): signatures,
//! the FTS index and communities on an existing graph.

use serde_json::{Map, Value, json};

use crate::{Context, Ordered, Payload, open_graph_for_write, resolve_repo};

/// A `bool` argument with a default; a JSON boolean only.
fn flag(arguments: &Map<String, Value>, key: &str) -> Option<bool> {
    match arguments.get(key) {
        None => Some(true),
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => None,
    }
}

pub(crate) fn run_postprocess(
    context: &Context,
    arguments: &Map<String, Value>,
) -> Option<Payload> {
    let args = crate::Args::new(arguments, &["communities", "fts", "repo_root"])?;
    let communities = flag(arguments, "communities")?;
    let fts = flag(arguments, "fts")?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let mut graph = open_graph_for_write(&root)?;
    // A failed step leaves the call to Python, which reruns every step.
    let counters = dagayn_build::rerun_postprocess(&mut graph.store, communities, fts).ok()?;
    let mut out = Ordered::default()
        .put("status", "ok")
        .put("summary", "Post-processing complete.");
    for (key, value) in counters {
        out = out.put(key, value);
    }
    // `PostprocessResult.warnings` is flattened in even when empty.
    Some(
        out.put("warnings", json!([]))
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}
