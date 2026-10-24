//! `architecture_analysis_tool(mode="import_cycles")`: every simple cycle
//! the graph's resolved imports form, between files, directories, or
//! declared units.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use dagayn_graph::GraphNode;
use serde_json::{Value, json};

use crate::Ordered;
use crate::architecture::{Artifact, Snapshot, View};

/// Cycles listed before the answer says it stopped.
const MAX_CYCLES: usize = 200;
/// Search steps before the enumeration gives up on being complete.
const MAX_STEPS: usize = 500_000;

pub(crate) struct CycleRequest<'a> {
    pub scope_kind: &'a str,
    pub unit_filter: Option<&'a [String]>,
    pub import_kind: Option<&'a str>,
    pub artifact: Artifact,
    pub artifact_scope: &'a str,
    pub detail_level: &'a str,
    pub root: &'a std::path::Path,
}

/// One import behind a hop between two scopes.
#[derive(Clone)]
struct Example<'a> {
    from: &'a str,
    to: &'a str,
    line: i64,
    kind: Option<&'a str>,
}

pub(crate) fn import_cycles(request: &CycleRequest<'_>, snapshot: &Snapshot) -> Ordered {
    let view = View {
        file_scopes: request.scope_kind == "file",
        artifact: request.artifact,
        units: (request.scope_kind == "package").then(|| snapshot.unit_scopes(request.root)),
    };
    let files: HashMap<&str, &GraphNode> = snapshot
        .all_nodes
        .iter()
        .filter(|node| node.kind == "File")
        .map(|node| (node.file_path.as_str(), node))
        .collect();
    let file_of: HashMap<&str, &str> = snapshot
        .all_nodes
        .iter()
        .map(|node| (node.qualified_name.as_str(), node.file_path.as_str()))
        .collect();
    let kept = |scope: &str| {
        request.unit_filter.is_none_or(|prefixes| {
            prefixes
                .iter()
                .any(|prefix| scope.starts_with(prefix.as_str()))
        })
    };
    let in_artifact = |file: &str| {
        files
            .get(file)
            .is_some_and(|node| request.artifact.includes(node))
    };
    // (from scope, to scope) -> the imports behind it, in file and line order.
    let mut hops: BTreeMap<(String, String), Vec<Example<'_>>> = BTreeMap::new();
    for edge in &snapshot.edges {
        if edge.kind != "IMPORTS_FROM"
            || !crate::query::import_kind_matches(edge, request.import_kind)
        {
            continue;
        }
        let from = edge.file_path.as_str();
        let target = edge.target_qualified.as_str();
        let to = file_of.get(target).copied().or_else(|| {
            let file = target.split_once("::").map_or(target, |(file, _)| file);
            files.contains_key(file).then_some(file)
        });
        let Some(to) = to else {
            continue;
        };
        if from == to || !in_artifact(from) || !in_artifact(to) {
            continue;
        }
        let (source, sink) = (view.scope_of(from), view.scope_of(to));
        if source == sink || !kept(&source) || !kept(&sink) {
            continue;
        }
        hops.entry((source, sink)).or_default().push(Example {
            from,
            to,
            line: edge.line,
            kind: crate::query::edge_import_kind(edge),
        });
    }
    for examples in hops.values_mut() {
        examples.sort_by(|a, b| (a.from, a.line, a.to).cmp(&(b.from, b.line, b.to)));
    }
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (source, sink) in hops.keys() {
        adjacency.entry(source).or_default().insert(sink);
        adjacency.entry(sink).or_default();
    }
    let groups: Vec<Vec<&str>> = crate::arch_findings::strongly_connected(&adjacency)
        .into_iter()
        .filter(|group| group.len() > 1)
        .collect();
    let (mut cycles, complete) = simple_cycles(&adjacency, &groups);
    cycles.sort();
    let found = cycles.len();
    let truncated = !complete || found > MAX_CYCLES;
    cycles.truncate(MAX_CYCLES);

    let detailed = request.detail_level != "minimal";
    let listed: Vec<Value> = cycles
        .iter()
        .map(|cycle| {
            if !detailed {
                return json!(cycle);
            }
            let imports: Vec<Value> = cycle
                .iter()
                .zip(cycle.iter().cycle().skip(1))
                .map(|(source, sink)| {
                    let examples = &hops[&(source.to_string(), sink.to_string())];
                    let first = &examples[0];
                    let mut hop = json!({
                        "from": first.from,
                        "to": first.to,
                        "line": first.line,
                        "imports": examples.len(),
                    });
                    if let Some(kind) = first.kind {
                        hop["import_kind"] = json!(kind);
                    }
                    hop
                })
                .collect();
            json!({"scopes": cycle, "imports": imports})
        })
        .collect();
    let scopes: usize = groups.iter().map(Vec::len).sum();
    let mut out = Ordered::default()
        .put("status", "ok")
        .put("summary", "")
        .put("cycles", json!(listed))
        .put("groups", json!(groups))
        .put("scope_kind", request.scope_kind)
        .put("artifact_scope", request.artifact_scope);
    if let Some(kind) = request.import_kind {
        out = out.put("import_kind", kind);
    }
    if let Some(prefixes) = request.unit_filter {
        out = out.put("unit_filter", json!(prefixes));
    }
    // Cycles go first: `groups` says where to narrow with unit_filter.
    out = out.apply_output_budget(
        if detailed {
            crate::STANDARD_BUDGET
        } else {
            crate::MINIMAL_BUDGET
        },
        &["groups", "cycles"],
    );
    let shown = out
        .get("cycles")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let complete = !truncated && shown == found;
    let filter = request
        .import_kind
        .map_or(String::new(), |kind| format!(", import_kind={kind}"));
    let count = if truncated {
        format!("More than {MAX_CYCLES}")
    } else {
        found.to_string()
    };
    let mut summary = format!(
        "{count} import cycle(s) among {scopes} {}(s) in {} strongly connected group(s) \
         (scope_kind={}, artifact_scope={}{filter}).",
        request.scope_kind,
        groups.len(),
        request.scope_kind,
        request.artifact_scope,
    );
    if complete {
        summary.push_str(
            " This is every simple cycle the graph's resolved imports form; an import that \
             resolves to no indexed file (standard library, third-party, compiled extension) \
             is not an edge.",
        );
    } else {
        summary.push_str(&format!(
            " Showing {shown}; narrow it with unit_filter (a scope prefix from groups)."
        ));
    }
    if !truncated {
        out = out.put("cycle_count", found);
    }
    out.set("summary", json!(summary))
        .put("results_complete", complete)
}

/// Every simple cycle of length 2 or more, each starting at its smallest
/// scope, and whether the search finished within [`MAX_STEPS`] and
/// [`MAX_CYCLES`] + 1 cycles.
fn simple_cycles<'a>(
    adjacency: &BTreeMap<&'a str, BTreeSet<&'a str>>,
    groups: &[Vec<&'a str>],
) -> (Vec<Vec<&'a str>>, bool) {
    let mut cycles = Vec::new();
    let mut steps = 0;
    for group in groups {
        let members: BTreeSet<&str> = group.iter().copied().collect();
        for &start in &members {
            // Paths from `start` through larger members only, so each cycle
            // is found once, from its smallest scope.
            let mut path = vec![start];
            let next = |node: &str| -> Vec<&'a str> {
                adjacency[node]
                    .iter()
                    .copied()
                    .filter(|other| members.contains(other) && *other >= start)
                    .collect()
            };
            let mut work: Vec<std::vec::IntoIter<&str>> = vec![next(start).into_iter()];
            while let Some(pending) = work.last_mut() {
                steps += 1;
                if steps > MAX_STEPS || cycles.len() > MAX_CYCLES {
                    return (cycles, false);
                }
                match pending.next() {
                    Some(child) if child == start => cycles.push(path.clone()),
                    Some(child) if !path.contains(&child) => {
                        path.push(child);
                        work.push(next(child).into_iter());
                    }
                    Some(_) => {}
                    None => {
                        work.pop();
                        path.pop();
                    }
                }
            }
        }
    }
    (cycles, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_each_simple_cycle_once_from_its_smallest_scope() {
        let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (from, to) in [("a", "b"), ("b", "a"), ("b", "c"), ("c", "a"), ("c", "d")] {
            adjacency.entry(from).or_default().insert(to);
            adjacency.entry(to).or_default();
        }
        let groups = vec![vec!["a", "b", "c"]];
        let (mut cycles, complete) = simple_cycles(&adjacency, &groups);
        cycles.sort();
        assert!(complete);
        assert_eq!(cycles, vec![vec!["a", "b"], vec!["a", "b", "c"]]);
    }
}
