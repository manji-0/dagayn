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
use std::path::Path;

use dagayn_graph::{GraphNode, GraphStore, has_framework_decorator, is_conventional_entry_point};
use serde_json::{Map, Value, json};

use crate::Ordered;
use crate::answerability::Answerability;
use crate::findings::is_production_code;
use crate::query::node_dict;
use crate::review::guidance_actions_to_hints;
use crate::review_summary::guidance_item;
use crate::units::UnitIndex;

/// Callers visited before the search stops and reports `truncated`. There
/// is no hop limit: the search keeps a visited set, so it ends; on this
/// repository the longest chain is 21 hops and no search visits more than
/// 840 callers (docs/plans/FLOW-TOOL-TARGET.md#order-of-work).
const MAX_VISITED: usize = 10_000;
/// Languages whose files run no code at load time: a call attributed to
/// such a file comes from a constant or a macro, not a script.
const NO_TOP_LEVEL_CODE: &[&str] = &[
    "rust", "go", "java", "kotlin", "c", "cpp", "csharp", "swift", "scala", "dart", "zig", "objc",
];
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
        let runs = !NO_TOP_LEVEL_CODE.contains(&node.language.as_str());
        return (runs && !has_callers).then_some("module_level");
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
            // Production code first, as `next::tests_last` orders it.
            named.sort_by_key(|node| node.is_test);
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

/// Searches from every target at once; each entry's chain ends at the
/// nearest target.
fn search(store: &GraphStore, targets: &[GraphNode]) -> Option<Search> {
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

    // `next[qn]` is the callee one hop closer to a target.
    let mut next: HashMap<String, Option<String>> = HashMap::new();
    let mut level: Vec<GraphNode> = Vec::new();
    for target in targets {
        if next.insert(target.qualified_name.clone(), None).is_none() {
            level.push(target.clone());
        }
    }
    let sources = next.len();
    let mut entries: Vec<Entry> = Vec::new();
    let mut truncated = false;
    let mut depth = 0;
    while !level.is_empty() {
        let mut wanted: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for node in &level {
            let qn = node.qualified_name.as_str();
            let has_callers = callers.get(qn).is_some_and(|list| !list.is_empty());
            // A target is its own entry point only when nothing calls it; a
            // called target named like an entry (`render`, `handle`) is not.
            let kind = entry_kind(node, has_callers).filter(|_| depth > 0 || !has_callers);
            if let Some(kind) = kind {
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
        reached: next.len() - sources,
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
    arguments: &Map<String, Value>,
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
            let shown: Vec<&GraphNode> = candidates.iter().collect();
            let next = crate::next::retries(
                "flow_tool",
                arguments,
                &[
                    ("mode", None),
                    ("limit", None),
                    ("detail_level", Some(json!("standard"))),
                ],
                &shown,
            );
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
                    .put("next", next.clone())
                    .put("answerability", answerability.full())
                    .put("missingness", json!(missingness))
                    .put("_hints", crate::next::as_hints(&next)),
            );
        }
    };
    let found = search(store, std::slice::from_ref(&node))?;
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
            "claim_effect": format!("the search stopped after {MAX_VISITED} callers; farther entry points are not listed"),
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
            .put("next", crate::next::read_entry_points(&shown))
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

/// The nearest entry points reaching any of `targets`, as
/// `review_tool(mode="affected_flows")` reports them: the first `limit`
/// entries, how many were left out, the callers walked, and whether the
/// search stopped early.
pub(crate) fn entry_points_reaching(
    store: &GraphStore,
    targets: &[GraphNode],
    limit: usize,
    detail_level: &str,
) -> Option<(Vec<Value>, usize, usize, bool)> {
    if targets.is_empty() {
        return Some((Vec::new(), 0, 0, false));
    }
    let found = search(store, targets)?;
    let shown = found
        .entries
        .iter()
        .take(limit)
        .map(|entry| entry_value(entry, detail_level))
        .collect();
    Some((
        shown,
        found.entries.len().saturating_sub(limit),
        found.reached,
        found.truncated,
    ))
}

/// Entry kinds in the order a listing shows them: the ones a person starts
/// first.
const KIND_ORDER: [&str; 7] = [
    "main",
    "framework_handler",
    "ffi_export",
    "named_entry",
    "module_level",
    "dispatched_method",
    "uncalled",
];

/// An entry point and its kind.
type Found = (GraphNode, &'static str);

/// Every entry point of the repository's production code with its kind.
/// A file counts (`module_level`) only when its top-level code calls
/// something.
pub(crate) fn all_entry_points(store: &GraphStore) -> Option<Vec<Found>> {
    let (calls_out, _) = store.get_flow_edge_data().ok()?;
    let called: HashSet<&str> = calls_out.values().flatten().map(String::as_str).collect();
    let nodes = store.get_all_nodes_filtered(false).ok()?;
    let known: HashSet<&str> = nodes.iter().map(|n| n.qualified_name.as_str()).collect();
    let mut found: Vec<Found> = Vec::new();
    for node in nodes.iter() {
        // A module's top level counts when it calls the repository's own
        // code, not only a library (`logging.getLogger`).
        let calls = calls_out
            .get(&node.qualified_name)
            .is_some_and(|c| c.iter().any(|callee| known.contains(callee.as_str())));
        let eligible = match node.kind.as_str() {
            "Function" => true,
            "File" => calls,
            _ => false,
        };
        if !eligible || !is_production_code(node, &node.file_path) {
            continue;
        }
        let has_callers = called.contains(node.qualified_name.as_str());
        if let Some(kind) = entry_kind(node, has_callers) {
            found.push((node.clone(), kind));
        }
    }
    let rank = |kind: &str| {
        KIND_ORDER
            .iter()
            .position(|k| *k == kind)
            .unwrap_or(KIND_ORDER.len())
    };
    found.sort_by(|(a, ak), (b, bk)| {
        (rank(ak), &a.qualified_name).cmp(&(rank(bk), &b.qualified_name))
    });
    Some(found)
}

/// `flow_tool(mode="entry_points")` without a target: the repository's
/// entry points per declared unit, counted by kind, the first `limit` of
/// each unit listed.
pub(crate) fn entry_point_map(
    store: &GraphStore,
    root: &Path,
    answerability: &Answerability,
    limit: i64,
    detail_level: &str,
) -> Option<Ordered> {
    let found = all_entry_points(store)?;
    let keep = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
    let files: Vec<String> = found
        .iter()
        .map(|(node, _)| node.file_path.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let index = UnitIndex::discover(root, files.iter().map(String::as_str));
    // Unit index -> entries, `None` for code no manifest or directory claims.
    let mut by_unit: Vec<(Option<usize>, Vec<&Found>)> = Vec::new();
    for entry in &found {
        let unit = index.unit_of(&entry.0.file_path);
        match by_unit.iter_mut().find(|(u, _)| *u == unit) {
            Some((_, list)) => list.push(entry),
            None => by_unit.push((unit, vec![entry])),
        }
    }
    by_unit.sort_by(|(a, al), (b, bl)| bl.len().cmp(&al.len()).then(a.cmp(b)));
    let count_kinds = |entries: &[&Found]| -> Map<String, Value> {
        let mut counts: Map<String, Value> = Map::new();
        for kind in KIND_ORDER {
            let n = entries.iter().filter(|(_, k)| *k == kind).count();
            if n > 0 {
                counts.insert(kind.to_string(), json!(n));
            }
        }
        counts
    };
    let all: Vec<&Found> = found.iter().collect();
    let units: Vec<Value> = by_unit
        .iter()
        .map(|(unit, entries)| {
            let unit = unit.and_then(|i| index.units.get(i));
            let listed: Vec<Value> = entries
                .iter()
                .take(keep)
                .map(|(node, kind)| {
                    let mut out = json!({"entry_point": node.qualified_name, "kind": kind});
                    if detail_level != "minimal" {
                        out["file"] = json!(node.file_path);
                        out["line"] = json!(node.line_start);
                    }
                    out
                })
                .collect();
            json!({
                "unit": unit.map_or("(no unit)", |u| u.name.as_str()),
                "unit_kind": unit.map(|u| u.kind),
                "entry_point_count": entries.len(),
                "kinds": count_kinds(entries),
                "entry_points": listed,
                "entry_points_omitted": entries.len().saturating_sub(keep),
            })
        })
        .collect();
    let total = found.len();
    let summary = format!("{total} entry point(s) in {} unit(s).", units.len());
    let mut missingness = answerability.missingness();
    let caveat = json!({
        "reason_code": "static_calls_only",
        "severity": "low",
        "claim_effect": "a function reached only through dynamic dispatch, reflection, or a framework the graph cannot see is listed as uncalled or dispatched_method",
    });
    missingness.push(caveat.clone());
    let guidance = vec![guidance_item(
        summary.clone(),
        json!({"type": "computed", "entry_point_count": total, "unit_count": units.len()}),
        "medium",
        vec![caveat],
        "flow_tool mode=\"entry_points\" target=<symbol> -- see which of these reach a symbol",
        vec![json!("query_time_entry_point_scan")],
        json!({"entry_point_count": total}),
    )];
    let hints = guidance_actions_to_hints(&guidance);
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("entry_point_count", total)
            .put("kinds", Value::Object(count_kinds(&all)))
            .put("units", Value::Array(units))
            .put("answerability", answerability.full())
            .put("missingness", json!(missingness))
            .put("guidance", Value::Array(guidance))
            .put("_hints", hints),
    )
}
