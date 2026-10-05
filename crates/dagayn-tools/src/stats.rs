//! `list_graph_stats_tool` (`dagayn.tools.query.list_graph_stats`).

use serde_json::{Map, Value, json};

use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo, suggestions};

const NEXT_TOOL_SUGGESTIONS: [&str; 3] = [
    "architecture_analysis_tool mode=\"communities\" -- inspect structure",
    "flow_tool mode=\"list\" -- inspect critical reachable-set flows",
    "semantic_search_nodes_tool -- search for specific entities",
];

pub(crate) fn list_graph_stats(
    context: &Context,
    arguments: &Map<String, Value>,
) -> Option<Payload> {
    let args = Args::new(arguments, &["repo_root"])?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let stats = graph.store.get_stats().ok()?;
    // Every vector, whatever the provider; none without the table.
    let embeddings: i64 = graph
        .store
        .embedding_provider_counts()
        .ok()?
        .map(|counts| counts.values().sum())
        .unwrap_or(0);
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let summary = format!(
        "Graph stats for {name}: {} nodes, {} edges, {} files, {} language(s), {embeddings} embedding(s).",
        stats.total_nodes,
        stats.total_edges,
        stats.files_count,
        stats.languages.len()
    );
    let (hints, kept) = suggestions(context, &NEXT_TOOL_SUGGESTIONS);
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("total_nodes", stats.total_nodes)
            .put("total_edges", stats.total_edges)
            .put("nodes_by_kind", json!(stats.nodes_by_kind))
            .put("edges_by_kind", json!(stats.edges_by_kind))
            .put("languages", json!(stats.languages))
            .put("files_count", stats.files_count)
            .put("last_updated", json!(stats.last_updated))
            .put("embeddings_count", embeddings)
            .put("_hints", hints)
            .put("next_tool_suggestions", kept)
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}
