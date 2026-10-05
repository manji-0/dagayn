//! `traverse_graph_tool` (`dagayn.tools.query.traverse_graph_func`): the
//! start node is `hybrid_search`'s top hit, with the embedding arm wherever
//! `semantic_search_nodes_tool` answers it; then BFS one batched layer at a
//! time, or DFS hydrating only the nodes it visits, under an approximate
//! token budget.

use std::collections::{HashMap, HashSet};

use dagayn_graph::{GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use crate::query::sanitize;
use crate::search::{embedding_request, top_qualified_name};
use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo, suggestions};

const NOT_FOUND_SUGGESTIONS: [&str; 2] = [
    "semantic_search_nodes_tool -- search more broadly for the symbol",
    "query_graph_tool -- inspect a known qualified name directly",
];
const FOUND_SUGGESTIONS: [&str; 2] = [
    "query_graph_tool callers_of -- focused relationship query",
    "review_tool mode=\"impact\" -- blast radius analysis",
];

struct Entry {
    name: String,
    qualified_name: String,
    kind: String,
    file: String,
    depth: i64,
}

impl Entry {
    fn new(node: &GraphNode, depth: i64) -> Self {
        Self {
            name: sanitize(&node.name),
            qualified_name: node.qualified_name.clone(),
            kind: node.kind.clone(),
            file: node.file_path.clone(),
            depth,
        }
    }

    /// `_estimate_traversal_entry_tokens`.
    fn tokens(&self) -> i64 {
        let chars = self.qualified_name.chars().count()
            + self.file.chars().count()
            + self.name.chars().count();
        (chars as i64 + 30) / 4
    }

    fn value(&self) -> Value {
        json!({
            "name": self.name,
            "qualified_name": self.qualified_name,
            "kind": self.kind,
            "file": self.file,
            "depth": self.depth,
        })
    }
}

#[derive(Default)]
struct Walk {
    traversal: Vec<Entry>,
    unresolved: Vec<String>,
    budget_exceeded: bool,
}

impl Walk {
    fn unresolved(&mut self, seen: &mut HashSet<String>, qualified_name: &str) {
        if seen.insert(qualified_name.to_string()) {
            self.unresolved.push(qualified_name.to_string());
        }
    }
}

/// The neighbors of `qualified_name`: outgoing targets, then incoming sources.
fn neighbors(store: &GraphStore, qualified_name: &str) -> Option<Vec<String>> {
    let keys = [qualified_name.to_string()];
    let (outgoing, incoming) = store.get_edges_by_endpoints(&keys).ok()?;
    let mut out: Vec<String> = outgoing
        .get(qualified_name)
        .into_iter()
        .flatten()
        .map(|edge| edge.target_qualified.clone())
        .collect();
    out.extend(
        incoming
            .get(qualified_name)
            .into_iter()
            .flatten()
            .map(|edge| edge.source_qualified.clone()),
    );
    Some(out)
}

/// `_traverse_dfs_lazy`.
fn dfs(store: &GraphStore, start: &str, depth: i64, budget: i64) -> Option<Walk> {
    let mut walk = Walk::default();
    let mut visited: HashMap<String, i64> = HashMap::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut seen = HashSet::new();
    let mut nodes: HashMap<String, Option<GraphNode>> = HashMap::new();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    let mut tokens = 0;
    let mut stack = vec![(start.to_string(), 0)];
    while let Some((current, level)) = stack.pop() {
        if level > depth {
            continue;
        }
        if visited.get(&current).is_some_and(|prev| level >= *prev) {
            continue;
        }
        if !nodes.contains_key(&current) {
            let mut found = store
                .get_nodes_by_qualified_names(std::slice::from_ref(&current))
                .ok()?;
            nodes.insert(current.clone(), found.remove(&current));
        }
        let Some(node) = nodes.get(&current).and_then(Option::as_ref) else {
            visited.insert(current.clone(), level);
            walk.unresolved(&mut seen, &current);
            continue;
        };
        visited.insert(current.clone(), level);
        let entry = Entry::new(node, level);
        tokens += entry.tokens();
        if tokens > budget {
            walk.budget_exceeded = true;
            break;
        }
        match index.get(&current) {
            Some(at) => walk.traversal[*at] = entry,
            None => {
                index.insert(current.clone(), walk.traversal.len());
                walk.traversal.push(entry);
            }
        }
        if level + 1 > depth {
            continue;
        }
        if !adjacency.contains_key(&current) {
            adjacency.insert(current.clone(), neighbors(store, &current)?);
        }
        for neighbor in adjacency[&current].iter().rev() {
            if visited.get(neighbor).is_none_or(|prev| level + 1 < *prev) {
                stack.push((neighbor.clone(), level + 1));
            }
        }
    }
    Some(walk)
}

/// The batched breadth-first walk.
fn bfs(store: &GraphStore, start: &str, depth: i64, budget: i64) -> Option<Walk> {
    let mut walk = Walk::default();
    let mut visited: HashSet<String> = HashSet::new();
    let mut seen = HashSet::new();
    let mut tokens = 0;
    let mut frontier = vec![start.to_string()];
    let mut level = 0;
    while !frontier.is_empty() && level <= depth && !walk.budget_exceeded {
        let mut layer: Vec<String> = Vec::new();
        let mut in_layer = HashSet::new();
        for qualified_name in &frontier {
            if visited.contains(qualified_name) || !in_layer.insert(qualified_name.clone()) {
                continue;
            }
            layer.push(qualified_name.clone());
        }
        if layer.is_empty() {
            break;
        }
        let nodes = store.get_nodes_by_qualified_names(&layer).ok()?;
        let (outgoing, incoming) = store.get_edges_by_endpoints(&layer).ok()?;
        let mut next = Vec::new();
        for current in &layer {
            if !visited.insert(current.clone()) {
                continue;
            }
            let Some(node) = nodes.get(current) else {
                walk.unresolved(&mut seen, current);
                continue;
            };
            let entry = Entry::new(node, level);
            tokens += entry.tokens();
            if tokens > budget {
                walk.budget_exceeded = true;
                break;
            }
            walk.traversal.push(entry);
            if level + 1 > depth {
                continue;
            }
            for edge in outgoing.get(current).into_iter().flatten() {
                if !visited.contains(&edge.target_qualified) {
                    next.push(edge.target_qualified.clone());
                }
            }
            for edge in incoming.get(current).into_iter().flatten() {
                if !visited.contains(&edge.source_qualified) {
                    next.push(edge.source_qualified.clone());
                }
            }
        }
        frontier = next;
        level += 1;
    }
    Some(walk)
}

pub(crate) fn traverse_graph(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(
        arguments,
        &[
            "query",
            "mode",
            "depth",
            "token_budget",
            "repo_root",
            "model",
            "provider",
        ],
    )?;
    let query = args.string("query")?;
    if query.trim().is_empty() {
        return None;
    }
    let mode = match arguments.get("mode") {
        None => "bfs",
        Some(Value::String(mode)) if mode == "bfs" || mode == "dfs" => mode,
        Some(_) => return None,
    };
    let depth = args.integer("depth", 3)?;
    let budget = args.integer("token_budget", 2000)?;
    let request = embedding_request(context, &args)?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let store = &graph.store;
    let depth = depth.clamp(1, 6);

    let Some(start) = top_qualified_name(store, &graph.db_path, query, &request)? else {
        let (hints, kept) = suggestions(context, &NOT_FOUND_SUGGESTIONS);
        return Some(
            Ordered::default()
                .put("status", "not_found")
                .put("summary", format!("No node matching '{query}'."))
                .put("start_node", Value::Null)
                .put("mode", mode)
                .put("max_depth", depth)
                .put("nodes_visited", 0)
                .put("traversal", json!([]))
                .put("truncated", false)
                .put(
                    "reachability",
                    json!({"state": "not_found", "truncated": false, "max_depth": depth, "nodes_visited": 0}),
                )
                .put("_hints", hints)
                .put("next_tool_suggestions", kept)
                .put("_repo", graph.repo_context())
                .into_payload(),
        );
    };

    let walk = if mode == "dfs" {
        dfs(store, &start, depth, budget)?
    } else {
        bfs(store, &start, depth, budget)?
    };
    let unresolved_count = walk.unresolved.len();
    let truncated = walk.budget_exceeded || unresolved_count > 0;
    let visited = walk.traversal.len();
    let reachability = json!({
        "state": if truncated { "truncated" } else { "complete" },
        "truncated": truncated,
        "max_depth": depth,
        "nodes_visited": visited,
        "unresolved_count": unresolved_count,
        "unresolved_targets": walk.unresolved,
    });
    let suffix = if walk.budget_exceeded {
        " Output was truncated to fit the token budget.".to_string()
    } else if unresolved_count > 0 {
        format!(" Traversal stopped at {unresolved_count} unresolvable endpoint(s).")
    } else {
        String::new()
    };
    let traversal: Vec<Value> = walk.traversal.iter().map(Entry::value).collect();
    let (hints, kept) = suggestions(context, &FOUND_SUGGESTIONS);
    Some(
        Ordered::default()
            .put("status", "ok")
            .put(
                "summary",
                format!("Traversed {visited} node(s) from '{start}' up to depth {depth}.{suffix}"),
            )
            .put("start_node", start)
            .put("mode", mode)
            .put("max_depth", depth)
            .put("nodes_visited", visited)
            .put("traversal", Value::Array(traversal))
            .put("truncated", walk.budget_exceeded)
            .put("reachability", reachability)
            .put("_hints", hints)
            .put("next_tool_suggestions", kept)
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}
