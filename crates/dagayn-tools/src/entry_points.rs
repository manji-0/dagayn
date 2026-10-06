//! `flow_tool(mode="entry_points")`: the entry points whose calls reach a
//! target, each with one shortest call chain
//! (docs/plans/FLOW-TOOL-TARGET.md#target-contract).
//!
//! A breadth-first search backwards over the edges the flow trace follows
//! (`CALLS` plus reportable `CROSS_ARTIFACT` hops), computed at query time,
//! so it needs no stored flows. Test code is never walked through: a helper
//! that only tests call has no entry point. The search stops at the first
//! entry point on each path, so `main -> handle -> target` reports `handle`.

use std::collections::{HashMap, HashSet};

use dagayn_graph::{GraphNode, GraphStore, has_framework_decorator, is_conventional_entry_point};
use serde_json::{Value, json};

use crate::Ordered;
use crate::answerability::Answerability;
use crate::findings::is_production_code;
use crate::query::node_dict;
use crate::review::guidance_actions_to_hints;
use crate::review_summary::guidance_item;

/// Hops followed from the target, as the flow trace's `DEFAULT_MAX_DEPTH`.
const MAX_DEPTH: usize = 15;
/// Callers visited before the search stops and reports `truncated`.
const MAX_VISITED: usize = 10_000;
/// Name matches offered when a bare name is ambiguous.
const MAX_CANDIDATES: usize = 5;

struct Entry {
    node: GraphNode,
    kind: &'static str,
    chain: Vec<String>,
}

/// Why `node` starts an execution, or `None` when it is only a step.
/// `has_callers` counts every caller in the graph, tests included.
fn entry_kind(node: &GraphNode, has_callers: bool) -> Option<&'static str> {
    if node.kind == "File" {
        return (!has_callers).then_some("module_level");
    }
    if matches!(node.name.as_str(), "main" | "__main__") {
        return Some("main");
    }
    if has_framework_decorator(node) {
        return Some("framework_handler");
    }
    if node.extra.get("ffi_export").is_some_and(|v| !v.is_null()) {
        return Some("ffi_export");
    }
    if is_conventional_entry_point(node) {
        return Some("named_entry");
    }
    if has_callers {
        return None;
    }
    // A method nothing calls statically is reached through a trait,
    // interface, or framework the graph cannot see; it is still where the
    // known calls start.
    Some(if node.parent_name.is_some() {
        "dispatched_method"
    } else {
        "uncalled"
    })
}

/// The target node: an exact qualified name, else a unique node of that
/// name. `Err` carries the candidates when the name is ambiguous.
fn resolve(store: &GraphStore, target: &str) -> Option<Result<Option<GraphNode>, Vec<GraphNode>>> {
    if let Some(node) = store.get_node(target).ok()? {
        return Some(Ok(Some(node)));
    }
    let hits = store.search_nodes(target, 50).ok()?;
    let mut named: Vec<GraphNode> = hits.into_iter().filter(|hit| hit.name == target).collect();
    match named.len() {
        0 => Some(Ok(None)),
        1 => Some(Ok(named.pop())),
        _ => {
            named.truncate(MAX_CANDIDATES);
            Some(Err(named))
        }
    }
}

struct Search {
    entries: Vec<Entry>,
    reached: usize,
    truncated: bool,
}

fn search(store: &GraphStore, target: &GraphNode) -> Option<Search> {
    let (calls_out, _) = store.get_flow_edge_data().ok()?;
    let mut callers: HashMap<&str, Vec<&str>> = HashMap::new();
    for (source, targets) in &calls_out {
        for callee in targets {
            let list = callers.entry(callee.as_str()).or_default();
            if !list.contains(&source.as_str()) {
                list.push(source.as_str());
            }
        }
    }
    for list in callers.values_mut() {
        list.sort_unstable();
    }

    // `next[qn]` is the callee one hop closer to the target.
    let mut next: HashMap<String, Option<String>> = HashMap::new();
    next.insert(target.qualified_name.clone(), None);
    let mut entries: Vec<Entry> = Vec::new();
    let mut level: Vec<GraphNode> = vec![target.clone()];
    let mut truncated = false;
    let mut depth = 0;
    while !level.is_empty() {
        let mut wanted: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for node in &level {
            let qn = node.qualified_name.as_str();
            let has_callers = callers.get(qn).is_some_and(|list| !list.is_empty());
            if let Some(kind) = entry_kind(node, has_callers) {
                let mut chain = vec![node.qualified_name.clone()];
                while let Some(Some(step)) = next.get(chain.last()?) {
                    chain.push(step.clone());
                }
                entries.push(Entry {
                    node: node.clone(),
                    kind,
                    chain,
                });
                // The nearest entry point on each path answers; what calls
                // it is a question about that entry point, not the target.
                if depth > 0 {
                    continue;
                }
            }
            if depth == MAX_DEPTH {
                truncated |= has_callers;
                continue;
            }
            for caller in callers.get(qn).into_iter().flatten() {
                if !next.contains_key(*caller) && seen.insert(caller) {
                    wanted.push((*caller).to_string());
                }
            }
        }
        if wanted.is_empty() {
            break;
        }
        let nodes = store.get_nodes_by_qualified_names(&wanted).ok()?;
        let mut upper: Vec<GraphNode> = Vec::new();
        for node in &level {
            let qn = node.qualified_name.as_str();
            for caller in callers.get(qn).into_iter().flatten() {
                if next.contains_key(*caller) {
                    continue;
                }
                let Some(found) = nodes.get(*caller) else {
                    continue;
                };
                if !is_production_code(found, &found.file_path) {
                    continue;
                }
                if next.len() > MAX_VISITED {
                    truncated = true;
                    break;
                }
                next.insert((*caller).to_string(), Some(node.qualified_name.clone()));
                upper.push(found.clone());
            }
        }
        level = upper;
        depth += 1;
    }
    entries.sort_by(|a, b| {
        (a.chain.len(), &a.node.qualified_name).cmp(&(b.chain.len(), &b.node.qualified_name))
    });
    Some(Search {
        entries,
        reached: next.len() - 1,
        truncated,
    })
}

fn entry_value(entry: &Entry, detail_level: &str) -> Value {
    let mut out = json!({
        "entry_point": entry.node.qualified_name,
        "kind": entry.kind,
        "hops": entry.chain.len() - 1,
        "chain": entry.chain,
    });
    if detail_level != "minimal" {
        out["file"] = json!(entry.node.file_path);
        out["line"] = json!(entry.node.line_start);
    }
    out
}

/// `flow_tool(mode="entry_points")`.
pub(crate) fn entry_points(
    store: &GraphStore,
    answerability: &Answerability,
    target: &str,
    limit: i64,
    detail_level: &str,
) -> Option<Ordered> {
    let node = match resolve(store, target)? {
        Ok(Some(node)) => node,
        Ok(None) => {
            let mut missingness = answerability.missingness();
            missingness.push(json!({
                "reason_code": "target_not_found",
                "severity": "medium",
                "claim_effect": "no node has this qualified name or name in the current graph",
            }));
            return Some(
                Ordered::default()
                    .put("status", "not_found")
                    .put("summary", format!("No node named '{target}'."))
                    .put("target", target)
                    .put("answerability", answerability.full())
                    .put("missingness", json!(missingness)),
            );
        }
        Err(candidates) => {
            let mut missingness = answerability.missingness();
            missingness.push(json!({
                "reason_code": "ambiguous_target",
                "severity": "medium",
                "claim_effect": "entry points were not searched for a unique node",
            }));
            return Some(
                Ordered::default()
                    .put("status", "ambiguous")
                    .put(
                        "summary",
                        format!("Multiple matches for '{target}'. Please use a qualified name."),
                    )
                    .put("target", target)
                    .put(
                        "candidates",
                        json!(candidates.iter().map(node_dict).collect::<Vec<_>>()),
                    )
                    .put("answerability", answerability.full())
                    .put("missingness", json!(missingness)),
            );
        }
    };
    let found = search(store, &node)?;
    let total = found.entries.len();
    let keep = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
    let shown: Vec<Value> = found
        .entries
        .iter()
        .take(keep)
        .map(|entry| entry_value(entry, detail_level))
        .collect();
    let qn = node.qualified_name.as_str();
    let summary = match total {
        0 => format!("No entry point outside tests reaches {qn}."),
        _ => format!(
            "{total} entry point(s) reach {qn} through {} caller(s).",
            found.reached
        ),
    };
    let mut missingness = answerability.missingness();
    let mut caveats = vec![json!({
        "reason_code": "static_calls_only",
        "severity": "low",
        "claim_effect": "calls through dynamic dispatch, reflection, or a framework the graph cannot see are not followed; dispatched_method marks where they start",
    })];
    if found.truncated {
        caveats.push(json!({
            "reason_code": "truncated_search",
            "severity": "medium",
            "claim_effect": format!("the search stopped at {MAX_DEPTH} hops or {MAX_VISITED} callers; farther entry points are not listed"),
        }));
    }
    missingness.extend(caveats.iter().cloned());
    let guidance = vec![guidance_item(
        summary.clone(),
        json!({
            "type": "computed",
            "target": qn,
            "entry_point_count": total,
            "reached_callers": found.reached,
        }),
        if total > 0 { "medium" } else { "low" },
        caveats,
        if total > 0 {
            "query_graph_tool pattern=\"source_of\" -- read the entry point and each step of its chain"
        } else {
            "query_graph_tool pattern=\"callers_of\" -- check whether only tests call the target"
        },
        vec![json!("query_time_reverse_search")],
        json!({"entry_point_count": total}),
    )];
    let hints = guidance_actions_to_hints(&guidance);
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("target", qn)
            .put("entry_points", Value::Array(shown))
            .put("entry_points_omitted", total.saturating_sub(keep))
            .put("reached_callers", found.reached)
            .put("truncated", found.truncated)
            .put("answerability", answerability.full())
            .put("missingness", json!(missingness))
            .put("guidance", Value::Array(guidance))
            .put("_hints", hints),
    )
}
