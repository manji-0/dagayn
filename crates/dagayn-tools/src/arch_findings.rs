//! `architecture_analysis_tool(mode="overview")` findings: structural facts
//! worth acting on, each naming the place to change and the edges behind it
//! (docs/plans/ARCHITECTURE-TOOL-TARGET.md#finding-kinds). A repository with
//! nothing to act on yields none.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use dagayn_graph::{GraphEdge, GraphNode};
use serde_json::{Value, json};

use crate::findings::{is_production_code, is_test_node};

/// Findings each kind lists; the rest are counted in `findings_omitted`.
const MAX_PER_KIND: usize = 10;
/// Import edges an `import_cycle` finding lists as evidence.
const MAX_CYCLE_EDGES: usize = 20;
/// Caller hops `untested_core` walks looking for a test, as
/// `review_tool`'s `untested_change` does.
const CALLER_TEST_DEPTH: usize = 4;
/// Other files that must use a symbol before it counts as core, whatever
/// the repository's p95.
const MIN_CORE_USERS: usize = 5;
/// Languages whose module-level imports run when the module loads, so a
/// cycle among them can fail or see half-initialised modules.
const LOAD_ORDER_LANGUAGES: &[&str] = &["python", "javascript", "typescript", "tsx"];
/// `import_kind`s that do not load the module when the importer loads:
/// erased type imports, and `require` / `import()` calls, which may sit in
/// a function.
const NON_LOADING_IMPORT_KINDS: &[&str] = &["type", "require", "dynamic"];

/// The overview's findings and the per-kind count of those not listed.
pub(crate) fn architecture_findings(
    root: &Path,
    nodes: &[GraphNode],
    edges: &[GraphEdge],
) -> (Vec<Value>, BTreeMap<&'static str, usize>) {
    let mut findings = Vec::new();
    let mut omitted = BTreeMap::new();
    for (kind, mut found) in [
        ("import_cycle", import_cycles(nodes, edges)),
        ("untested_core", untested_core(nodes, edges)),
        ("broken_doc_link", broken_doc_links(root, nodes, edges)),
    ] {
        if found.len() > MAX_PER_KIND {
            omitted.insert(kind, found.len() - MAX_PER_KIND);
            found.truncate(MAX_PER_KIND);
        }
        findings.extend(found);
    }
    (findings, omitted)
}

/// Files the graph indexes, by path, with their language.
fn file_languages(nodes: &[GraphNode]) -> HashMap<&str, &str> {
    nodes
        .iter()
        .filter(|node| node.kind == "File")
        .map(|node| (node.file_path.as_str(), node.language.as_str()))
        .collect()
}

/// Whether an import runs when the importing module loads.
fn loads_on_import(edge: &GraphEdge) -> bool {
    let extra = &edge.extra;
    extra.get("import_scope").is_none()
        && extra.get("lazy_export").and_then(Value::as_bool) != Some(true)
        && !extra
            .get("import_kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| NON_LOADING_IMPORT_KINDS.contains(&kind))
}

/// Production code files: not tests, fixtures, examples, or vendored code.
fn is_production_file(path: &str) -> bool {
    let probe = GraphNode {
        id: 0,
        kind: "File".to_string(),
        name: String::new(),
        qualified_name: path.to_string(),
        file_path: path.to_string(),
        line_start: 0,
        line_end: 0,
        language: "python".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        is_test: false,
        file_hash: None,
        extra: Value::Null,
        signature: None,
    };
    is_production_code(&probe, path)
}

/// Strongly connected sets of modules among the runtime imports of
/// load-order languages, each with the imports to cut.
fn import_cycles(nodes: &[GraphNode], edges: &[GraphEdge]) -> Vec<Value> {
    let languages = file_languages(nodes);
    let file_of: HashMap<&str, &str> = nodes
        .iter()
        .map(|node| (node.qualified_name.as_str(), node.file_path.as_str()))
        .collect();
    let in_scope = |file: &str| {
        languages
            .get(file)
            .is_some_and(|language| LOAD_ORDER_LANGUAGES.contains(language))
            && is_production_file(file)
    };
    // (from, to) -> import lines
    let mut imports: BTreeMap<(&str, &str), Vec<i64>> = BTreeMap::new();
    for edge in edges {
        if edge.kind != "IMPORTS_FROM" || !loads_on_import(edge) {
            continue;
        }
        let from = edge.file_path.as_str();
        let to = file_of
            .get(edge.target_qualified.as_str())
            .copied()
            .or_else(|| {
                languages
                    .contains_key(edge.target_qualified.as_str())
                    .then_some(edge.target_qualified.as_str())
            });
        let Some(to) = to else {
            continue;
        };
        if from == to || !in_scope(from) || !in_scope(to) {
            continue;
        }
        imports.entry((from, to)).or_default().push(edge.line);
    }
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (from, to) in imports.keys() {
        adjacency.entry(from).or_default().insert(to);
        adjacency.entry(to).or_default();
    }
    let mut findings: Vec<(usize, Value)> = strongly_connected(&adjacency)
        .into_iter()
        .filter(|component| component.len() > 1)
        .map(|component| {
            let members: BTreeSet<&str> = component.iter().copied().collect();
            let inner: Vec<((&str, &str), &Vec<i64>)> = imports
                .iter()
                .filter(|((from, to), _)| members.contains(from) && members.contains(to))
                .map(|(pair, lines)| (*pair, lines))
                .collect();
            let cut = cycle_cut(&members, &inner);
            let evidence: Vec<Value> = inner
                .iter()
                .take(MAX_CYCLE_EDGES)
                .flat_map(|((from, to), lines)| {
                    lines
                        .iter()
                        .map(move |line| json!({"from": from, "to": to, "line": line}))
                })
                .collect();
            let modules: Vec<&str> = members.iter().copied().collect();
            let size = modules.len();
            let mut finding = json!({
                "kind": "import_cycle",
                "file": modules[0],
                "targets": modules,
                "evidence": evidence,
                "cut": cut
                    .iter()
                    .map(|(from, to, line)| json!({"file": from, "line": line, "imports": to}))
                    .collect::<Vec<_>>(),
                "action": "Break the cycle at the listed import(s): move what both sides need into a module neither imports, or import it inside the function that uses it.",
            });
            if inner.len() > MAX_CYCLE_EDGES {
                finding["evidence_omitted"] = json!(inner.len() - MAX_CYCLE_EDGES);
            }
            (size, finding)
        })
        .collect();
    findings.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1["file"].as_str().cmp(&b.1["file"].as_str()))
    });
    findings.into_iter().map(|(_, finding)| finding).collect()
}

/// Tarjan's algorithm, iteratively, over a deterministic adjacency.
fn strongly_connected<'a>(adjacency: &BTreeMap<&'a str, BTreeSet<&'a str>>) -> Vec<Vec<&'a str>> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut low: HashMap<&str, usize> = HashMap::new();
    let mut on_stack: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = Vec::new();
    let mut components = Vec::new();
    let mut next = 0;
    for &start in adjacency.keys() {
        if index.contains_key(start) {
            continue;
        }
        let mut work: Vec<(&str, Vec<&str>)> = Vec::new();
        index.insert(start, next);
        low.insert(start, next);
        next += 1;
        stack.push(start);
        on_stack.insert(start);
        work.push((start, adjacency[start].iter().rev().copied().collect()));
        while let Some((node, pending)) = work.last_mut() {
            let node = *node;
            if let Some(child) = pending.pop() {
                if !index.contains_key(child) {
                    index.insert(child, next);
                    low.insert(child, next);
                    next += 1;
                    stack.push(child);
                    on_stack.insert(child);
                    work.push((child, adjacency[child].iter().rev().copied().collect()));
                } else if on_stack.contains(child) {
                    let value = low[node].min(index[child]);
                    low.insert(node, value);
                }
                continue;
            }
            work.pop();
            if let Some((parent, _)) = work.last() {
                let value = low[parent].min(low[node]);
                low.insert(parent, value);
            }
            if low[node] == index[node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack.remove(member);
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                component.sort_unstable();
                components.push(component);
            }
        }
    }
    components
}

/// The imports that break every cycle of a component: those pointing
/// backwards in the Eades-Lin-Smyth order, which keeps the edges carrying
/// the most imports and drops the fewest.
fn cycle_cut<'a>(
    members: &BTreeSet<&'a str>,
    inner: &[((&'a str, &'a str), &Vec<i64>)],
) -> Vec<(&'a str, &'a str, i64)> {
    let weight = |from: &str, to: &str| -> usize {
        inner
            .iter()
            .find(|((f, t), _)| *f == from && *t == to)
            .map_or(0, |(_, lines)| lines.len())
    };
    let mut remaining: BTreeSet<&str> = members.clone();
    let (mut head, mut tail): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    let degree = |node: &str, remaining: &BTreeSet<&str>, outgoing: bool| -> usize {
        remaining
            .iter()
            .map(|other| {
                if outgoing {
                    weight(node, other)
                } else {
                    weight(other, node)
                }
            })
            .sum()
    };
    while !remaining.is_empty() {
        let mut changed = true;
        while changed {
            changed = false;
            if let Some(sink) = remaining
                .iter()
                .copied()
                .find(|node| degree(node, &remaining, true) == 0)
            {
                remaining.remove(sink);
                tail.push(sink);
                changed = true;
            }
            if let Some(source) = remaining
                .iter()
                .copied()
                .find(|node| degree(node, &remaining, false) == 0)
            {
                remaining.remove(source);
                head.push(source);
                changed = true;
            }
        }
        if let Some(best) = remaining.iter().copied().max_by(|a, b| {
            let score = |node: &str| {
                degree(node, &remaining, true) as i64 - degree(node, &remaining, false) as i64
            };
            score(a).cmp(&score(b)).then_with(|| b.cmp(a))
        }) {
            remaining.remove(best);
            head.push(best);
        }
    }
    tail.reverse();
    head.extend(tail);
    let position: HashMap<&str, usize> = head.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut cut = Vec::new();
    for ((from, to), lines) in inner {
        if position[from] > position[to] {
            for line in lines.iter() {
                cut.push((*from, *to, *line));
            }
        }
    }
    cut
}

/// Widely used production symbols that no test reaches through their
/// callers.
fn untested_core(nodes: &[GraphNode], edges: &[GraphEdge]) -> Vec<Value> {
    if !edges.iter().any(|edge| edge.kind == "TESTED_BY") {
        // A repository without tests: every symbol would be one.
        return Vec::new();
    }
    let by_qn: HashMap<&str, &GraphNode> = nodes
        .iter()
        .map(|node| (node.qualified_name.as_str(), node))
        .collect();
    let candidate = |node: &GraphNode| {
        matches!(node.kind.as_str(), "Function" | "Class")
            && is_production_code(node, &node.file_path)
    };
    let mut users: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut callers: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut tested: HashSet<&str> = HashSet::new();
    for edge in edges {
        match edge.kind.as_str() {
            "TESTED_BY" => {
                tested.insert(edge.source_qualified.as_str());
            }
            "CALLS" | "REFERENCES" | "INHERITS" | "IMPLEMENTS" => {
                let (source, target) = (
                    edge.source_qualified.as_str(),
                    edge.target_qualified.as_str(),
                );
                callers.entry(target).or_default().push(source);
                if let Some(node) = by_qn.get(target)
                    && candidate(node)
                    && edge.file_path != node.file_path
                {
                    users
                        .entry(target)
                        .or_default()
                        .insert(edge.file_path.as_str());
                }
            }
            _ => {}
        }
    }
    let mut counts: Vec<usize> = users.values().map(HashSet::len).collect();
    if counts.is_empty() {
        return Vec::new();
    }
    counts.sort_unstable();
    let p95 = counts[(counts.len() * 95 / 100).min(counts.len() - 1)];
    let threshold = p95.max(MIN_CORE_USERS);
    let reached = |start: &str| -> bool {
        let mut frontier = vec![start];
        let mut seen: HashSet<&str> = HashSet::from([start]);
        for _ in 0..=CALLER_TEST_DEPTH {
            if frontier.iter().any(|qn| {
                tested.contains(qn)
                    || by_qn
                        .get(qn)
                        .is_some_and(|node| is_test_node(node, &node.file_path))
            }) {
                return true;
            }
            let mut next = Vec::new();
            for qn in frontier {
                for caller in callers.get(qn).into_iter().flatten() {
                    if seen.insert(caller) {
                        next.push(*caller);
                    }
                }
            }
            frontier = next;
        }
        false
    };
    let mut found: Vec<(usize, &GraphNode)> = users
        .iter()
        .filter(|(_, files)| files.len() >= threshold)
        .filter(|(qn, _)| !reached(qn))
        .map(|(qn, files)| (files.len(), by_qn[qn]))
        .collect();
    found.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.qualified_name.cmp(&b.1.qualified_name))
    });
    found
        .into_iter()
        .map(|(count, node)| {
            json!({
                "kind": "untested_core",
                "qualified_name": node.qualified_name,
                "file": node.file_path,
                "line": node.line_start,
                "evidence": {
                    "used_from_files": count,
                    "threshold": threshold,
                    "caller_hops_searched": CALLER_TEST_DEPTH,
                },
                "action": "Write a test that exercises it, or point to the test that does if the graph misses it.",
            })
        })
        .collect()
}

/// Authored doc directives that point inside the repository at a file,
/// section, or symbol that does not exist.
fn broken_doc_links(root: &Path, nodes: &[GraphNode], edges: &[GraphEdge]) -> Vec<Value> {
    let qualified: HashSet<&str> = nodes
        .iter()
        .map(|node| node.qualified_name.as_str())
        .collect();
    let indexed: HashSet<&str> = nodes.iter().map(|node| node.file_path.as_str()).collect();
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for edge in edges {
        let directive = match edge.kind.as_str() {
            "DEPENDS_ON" => edge
                .extra
                .get("markdown_directive_kind")
                .and_then(Value::as_str),
            "CROSS_ARTIFACT"
                if edge.extra.get("evidence_source").and_then(Value::as_str)
                    == Some("dagayn_directive") =>
            {
                edge.extra
                    .get("dagayn_directive_kind")
                    .and_then(Value::as_str)
                    .or(Some("dagayn"))
            }
            _ => None,
        };
        let Some(directive) = directive else {
            continue;
        };
        let target = edge.target_qualified.as_str();
        let (file, member) = match target.split_once("::") {
            Some((file, member)) => (file, Some(member)),
            None => (target, None),
        };
        if !is_repo_path(file) {
            continue;
        }
        let missing = if !root.join(file).exists() {
            Some("file")
        } else if let Some(member) = member
            && indexed.contains(file)
            && !qualified.contains(target)
        {
            Some(if file.ends_with(".md") && !member.contains('.') {
                "section"
            } else {
                "symbol"
            })
        } else {
            None
        };
        let Some(missing) = missing else {
            continue;
        };
        if !seen.insert((edge.file_path.as_str(), edge.line, target)) {
            continue;
        }
        found.push(json!({
            "kind": "broken_doc_link",
            "file": edge.file_path,
            "line": edge.line,
            "target": target,
            "evidence": {"directive": directive, "missing": missing},
            "action": "Point the directive at what replaced the target, or remove it.",
        }));
    }
    found.sort_by(|a, b| {
        (a["file"].as_str(), a["line"].as_i64()).cmp(&(b["file"].as_str(), b["line"].as_i64()))
    });
    found
}

/// A repo-relative path, not one outside the repository or a URL.
fn is_repo_path(path: &str) -> bool {
    !(path.is_empty()
        || path.starts_with('~')
        || path.starts_with('/')
        || path.starts_with('$')
        || path.starts_with("<unresolved:")
        || path.contains("://")
        || path.split('/').any(|part| part == ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_keeps_the_heavier_direction() {
        let members: BTreeSet<&str> = ["a.py", "b.py"].into_iter().collect();
        let many = vec![1, 2, 3];
        let one = vec![7];
        let inner = vec![(("a.py", "b.py"), &many), (("b.py", "a.py"), &one)];
        assert_eq!(cycle_cut(&members, &inner), vec![("b.py", "a.py", 7)]);
    }

    #[test]
    fn components_are_found_without_recursion() {
        let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (from, to) in [("a", "b"), ("b", "c"), ("c", "a"), ("c", "d"), ("d", "e")] {
            adjacency.entry(from).or_default().insert(to);
            adjacency.entry(to).or_default();
        }
        let mut components = strongly_connected(&adjacency);
        components.sort();
        assert_eq!(components, vec![vec!["a", "b", "c"], vec!["d"], vec!["e"]]);
    }

    #[test]
    fn outside_paths_are_not_judged() {
        for path in [
            "~/.pi/AGENTS.md",
            "/etc/hosts",
            "https://x.dev/a",
            "../other/a.md",
        ] {
            assert!(!is_repo_path(path), "{path}");
        }
        assert!(is_repo_path("docs/a.md"));
    }
}
