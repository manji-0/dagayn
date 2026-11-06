//! The sentences a `query_graph` reply says about what the graph holds,
//! the Go `callers_of` sentences among them.

use std::collections::HashSet;
use std::path::Path;

use dagayn_graph::{GraphEdge, GraphNode, GraphStore};
use serde_json::{Value, json};

use super::MAX_DEPTH;
use super::rows::{VALUE_REFERENCE, get};
use super::walk::{Found, Reaching, TRANSITIVE_ROW_LIMIT};
use crate::go_module::GoScope;

/// The zero-result reason for a node the graph has but that no edge of the
/// kind asked for reaches (#195).
pub(super) const NO_MATCHING_EDGES: &str = "node_has_no_matching_edges";
/// The zero-result reason for a Go node whose only rows are where the Go
/// tools building its module do not look (#325).
pub(super) const OUTSIDE_MODULE_ONLY: &str = "outside_module_only";

/// `guidance` for a Go answer whose every row is in `outside_module`: the
/// node has relationships, none in the directories its module builds.
pub(super) fn outside_guidance(pattern: &str, target: &str, exact_count: i64) -> Value {
    json!([{
        "claim": format!(
            "'{target}' has no {pattern} relationship in the directories the Go tools build for \
             its module; outside_module lists the ones elsewhere."
        ),
        "evidence": [{
            "type": "computed",
            "pattern": pattern,
            "target": target,
            "result_count": 0,
            "exact_match_count": exact_count,
        }],
        "confidence": "medium",
        "missingness": [{
            "reason_code": OUTSIDE_MODULE_ONLY,
            "severity": "low",
            "claim_effect": "rows in _ or . directories, testdata, or another module are not the module's as go list ./... sees it",
        }],
        "action": "read outside_module -- the relationships outside the module's built directories",
        "reason_codes": ["zero_result", OUTSIDE_MODULE_ONLY],
        "counts": {"result_count": 0},
    }])
}

/// The sentence for the rows `outside_module` lists: `count` of them by
/// node, `listed` shown, in `places` (place, why, rows), as
/// `13 callers are in directories the Go tools do not build for this
/// module, listed in outside_module and not counted in results:
/// _examples/ (11, name starts with _) and _examples/rest/ (2, its own
/// module)`.
pub(super) fn outside_note(
    pattern: &str,
    count: usize,
    listed: usize,
    places: &[(String, &str, usize)],
) -> String {
    let noun = match pattern {
        "callers_of" => "caller",
        "callees_of" => "callee",
        "importers_of" => "importer",
        "tests_for" => "test",
        _ => "row",
    };
    let (noun, verb) = if count == 1 {
        (noun.to_string(), "is in a directory")
    } else {
        (format!("{noun}s"), "are in directories")
    };
    let first = if listed < count {
        format!(" (the first {listed})")
    } else {
        String::new()
    };
    let places: Vec<String> = places
        .iter()
        .map(|(place, why, rows)| format!("{place} ({rows}, {why})"))
        .collect();
    format!(
        "{count} {noun} {verb} the Go tools do not build for this module, \
         listed in outside_module{first} and not counted in results: {}",
        capped_list(&places, 5)
    )
}

/// `query_graph_guidance` for a result set, sealed as `GuidanceItem` dumps it.
pub(super) fn guidance(
    pattern: &str,
    target: &str,
    result_count: usize,
    exact_count: i64,
) -> Value {
    if result_count == 0 && exact_count > 0 && !matches!(pattern, "source_of" | "file_summary") {
        return json!([{
            "claim": format!("'{target}' is in the graph; no {pattern} relationship reaches it there."),
            "evidence": [{
                "type": "computed",
                "pattern": pattern,
                "target": target,
                "result_count": 0,
                "exact_match_count": exact_count,
            }],
            "confidence": "medium",
            "missingness": [{
                "reason_code": NO_MATCHING_EDGES,
                "severity": "low",
                "claim_effect": "the node is indexed; relationships made at run time (dispatch, values passed around) are not edges",
            }],
            "action": format!("query_graph_tool pattern=\"source_of\" target=\"{target}\" -- read how it is used"),
            "reason_codes": ["zero_result"],
            "counts": {"result_count": 0},
        }]);
    }
    if result_count == 0 {
        return json!([{
            "claim": format!("No graph relationships matched '{pattern}' for '{target}'."),
            "evidence": [{"type": "computed", "pattern": pattern, "target": target, "result_count": 0}],
            "confidence": "low",
            "missingness": [{
                "reason_code": "not_found_in_current_graph",
                "severity": "medium",
                "claim_effect": "absence is graph-limited, not proof the relationship does not exist",
            }],
            "action": "semantic_search_nodes_tool -- verify the target and refresh the graph",
            "reason_codes": ["zero_result"],
            "counts": {"result_count": 0},
        }]);
    }
    if pattern == "source_of" {
        return json!([{
            "claim": format!("Live source span for '{target}'."),
            "evidence": [{
                "type": "computed",
                "pattern": pattern,
                "target": target,
                "result_count": result_count,
                "exact_match_count": exact_count,
            }],
            "confidence": "medium",
            "missingness": [{
                "reason_code": "live_source_span",
                "severity": "low",
                "claim_effect": "text is a worktree slice of the graph span; surrounding helpers and imports are omitted",
            }],
            "action": "query_graph_tool pattern=\"callers_of\" -- inspect callers after reading the span",
            "reason_codes": ["live_source_fetch"],
            "counts": {"result_count": result_count},
        }]);
    }
    json!([{
        "claim": format!(
            "Graph query '{pattern}' returned {result_count} related node(s) for '{target}'."
        ),
        "evidence": [{
            "type": "computed",
            "pattern": pattern,
            "target": target,
            "result_count": result_count,
            "exact_match_count": exact_count,
        }],
        "confidence": "medium",
        "missingness": [{
            "reason_code": "relationship_query_not_runtime_proof",
            "severity": "low",
            "claim_effect": "graph edges are static extraction, not runtime traces",
        }],
        "action": format!("query_graph_tool pattern=\"{pattern}\" -- drill into a relationship"),
        "reason_codes": ["graph_relationship_query"],
        "counts": {"result_count": result_count},
    }])
}

/// The sentence a closed answer of `pattern` ends with: the list holds every
/// relation the graph records (`direct` ones at depth 1), and says what the
/// graph does not hold for the target's `language`.
fn closed_note(
    pattern: &str,
    direct: bool,
    language: &str,
    import_kind: Option<&str>,
) -> Option<String> {
    let relation = match pattern {
        "callers_of" => "caller",
        "callees_of" => "callee",
        "importers_of" => "importer",
        "imports_of" => "import",
        _ => return None,
    };
    let caveat = edge_caveat(pattern, language, import_kind);
    let direct = if direct { "direct " } else { "" };
    Some(format!(
        "No {direct}{relation} the graph records is missing; {caveat}"
    ))
}

/// What a static graph leaves out of `pattern`'s edges for `language`.
fn edge_caveat(pattern: &str, language: &str, import_kind: Option<&str>) -> &'static str {
    let calls = matches!(pattern, "callers_of" | "callees_of");
    match (language, calls, import_kind) {
        (_, false, Some("directive")) => "only Markdown dependency directives were followed",
        (_, false, Some("link")) => "only Markdown links were followed",
        (_, false, Some("lazy_export")) => "only imports a lazy-export table names were followed",
        (_, false, Some("path")) => "only Rust paths in code without a `use` were followed",
        (_, false, Some(_)) => "only import statements were followed",
        ("markdown", false, None) => "Markdown links and dependency directives both count",
        ("python", false, _) => {
            "imports built at run time are edges only when a lazy-export table names \
             them (import_kind lazy_export)"
        }
        ("python", true, _) => "calls made through strings, getattr, or registration are not edges",
        ("rust", true, _) => {
            "a method call is an edge only where the code gives its receiver's type (a typed \
             parameter or binding, a struct field, a declared return type); calls on closure \
             parameters, loop variables, or untyped locals are not"
        }
        ("rust", false, _) => {
            "a `use` or a path in code (import_kind path) counts; a path written only inside \
             a macro's arguments does not"
        }
        (_, false, _) => "modules loaded at run time are not edges",
        (_, true, _) => "calls dispatched at run time are not edges",
    }
}

/// What the graph knows of a Go call that goes to no definition.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum GoUnbound {
    /// On a value whose static type (`receiver_type`) is an interface the
    /// repository declares: the node `qualified`, at `place` (`file:line`).
    Interface {
        name: String,
        qualified: String,
        place: String,
    },
    /// On a value of a standard-library or dependency type
    /// (`external_symbol`): `owner` is that type (`bytes.Buffer`).
    /// `concrete` when the code writes the value so that the type is no
    /// interface (`receiver_concrete`: `t *testing.T`, `bytes.Buffer{}`), so
    /// the call is that type's method and cannot reach the repository's.
    Library {
        owner: String,
        stdlib: bool,
        concrete: bool,
    },
    /// On a value whose type the graph does not know, or of a type
    /// (`typed`) whose method of the name it does not find; `from` names
    /// the function that returned the value (`receiver_from`).
    Unknown {
        from: Option<String>,
        typed: Option<String>,
    },
}

impl GoUnbound {
    fn of(store: &GraphStore, edge: &GraphEdge, name: &str) -> Self {
        let text = |key: &str| edge.extra.get(key).and_then(Value::as_str);
        if let Some(owner) = text("external_symbol")
            .and_then(|symbol| symbol.strip_suffix(name))
            .and_then(|owner| owner.strip_suffix('.'))
        {
            return GoUnbound::Library {
                owner: owner.to_string(),
                stdlib: edge.extra.get("stdlib") == Some(&Value::Bool(true)),
                concrete: edge.extra.get("receiver_concrete") == Some(&Value::Bool(true)),
            };
        }
        if let Some(interface) = text("receiver_type").and_then(|type_name| {
            go_interface(store, type_name, &edge.file_path, text("receiver_package"))
        }) {
            return GoUnbound::Interface {
                place: format!("{}:{}", interface.file_path, interface.line_start),
                name: interface.name,
                qualified: interface.qualified_name,
            };
        }
        let from = edge
            .extra
            .get("receiver_from")
            .and_then(|from| from.get("call"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let typed = text("receiver_type").map(str::to_string);
        GoUnbound::Unknown { from, typed }
    }

    /// The call, as a summary names it: `through interface encoder
    /// (zstd/encoder.go:32)`.
    fn describe(&self) -> String {
        match self {
            GoUnbound::Interface { name, place, .. } => {
                format!("through interface {name} ({place})")
            }
            GoUnbound::Library {
                owner,
                stdlib: true,
                ..
            } => format!("on the standard-library type {owner}"),
            GoUnbound::Library { owner, .. } => format!("on the dependency type {owner}"),
            GoUnbound::Unknown {
                typed: Some(typed), ..
            } => format!(
                "on a value of type {typed}, whose method of the name the graph does not find"
            ),
            GoUnbound::Unknown {
                from: Some(from), ..
            } => format!("on what {from}() returned, of a type the graph does not know"),
            GoUnbound::Unknown { .. } => {
                "on a value whose type the graph does not know".to_string()
            }
        }
    }
}

/// The one Go interface named `type_name` in the package a call in
/// `call_file` names it from: the caller's own package, or the package of
/// the import path `package` (`receiver_package`).
fn go_interface(
    store: &GraphStore,
    type_name: &str,
    call_file: &str,
    package: Option<&str>,
) -> Option<GraphNode> {
    let caller_dir = Path::new(call_file).parent().and_then(Path::to_str);
    let mut found = store
        .get_nodes_by_name(type_name)
        .ok()?
        .into_iter()
        .filter(|node| node.kind == "Class" && node.language == "go")
        .filter(|node| node.extra.get("type_role") == Some(&json!("interface")))
        .filter(|node| match package {
            // The import paths a Go file's package is known by are its File
            // node's `namespaces`.
            Some(path) => store
                .get_node(&node.file_path)
                .ok()
                .flatten()
                .and_then(|file| file.extra.get("namespaces").cloned())
                .and_then(|namespaces| namespaces.as_array().cloned())
                .is_some_and(|namespaces| namespaces.contains(&json!(path))),
            None => Path::new(&node.file_path).parent().and_then(Path::to_str) == caller_dir,
        });
    let only = found.next()?;
    found.next().is_none().then_some(only)
}

/// The Go calls named like `node` that go to no definition in the
/// repository, with what the graph knows of each: made through an interface
/// the repository declares, on a standard-library or dependency type
/// (`w.Write(p)` on `w io.Writer`), or on a value whose type the graph does
/// not know, so any may call `node` at run time. A call on what a function
/// of another module returned (`zw := gzip.NewWriter(w); zw.Reset(w)`) is
/// that module's and is not one of them (#286), and a call on a value of a
/// known type is a method call, so it is not one for a function. An
/// unexported name is callable from its own package (its directory) only.
/// Calls from `node`'s package come first.
fn go_unbound_calls(store: &GraphStore, node: &GraphNode) -> Option<Vec<(GraphEdge, GoUnbound)>> {
    let package = Path::new(&node.file_path).parent();
    let exported = node.name.starts_with(char::is_uppercase);
    let mut edges: Vec<GraphEdge> = store
        .get_edges_by_target(&node.name)
        .ok()?
        .into_iter()
        .filter(|edge| edge.kind == "CALLS")
        .filter(|edge| edge.extra.get("external") != Some(&Value::Bool(true)))
        .filter(|edge| exported || Path::new(&edge.file_path).parent() == package)
        .collect();
    if exported && node.parent_name.is_some() {
        edges.extend(store.search_external_calls_by_method(&node.name).ok()?);
    }
    edges.retain(|edge| edge.file_path.ends_with(".go"));
    edges.sort_by_key(|edge| Path::new(&edge.file_path).parent() != package);
    Some(
        edges
            .into_iter()
            .map(|edge| {
                let kind = GoUnbound::of(store, &edge, &node.name);
                (edge, kind)
            })
            .filter(|(_, kind)| {
                node.parent_name.is_some() || matches!(kind, GoUnbound::Unknown { .. })
            })
            .collect(),
    )
}

/// Up to this many unbound calls, a Go `callers_of` gives each one's line.
const GO_LISTED_UNBOUND_CALLS: usize = 6;

/// `edges`' lines, sorted and once each, as `133, :182 and :199`.
fn go_call_lines(edges: &[&GraphEdge]) -> String {
    let mut lines: Vec<i64> = edges.iter().map(|edge| edge.line).collect();
    lines.sort_unstable();
    lines.dedup();
    let mut lines: Vec<String> = lines.iter().map(i64::to_string).collect();
    match lines.pop() {
        Some(last) if !lines.is_empty() => format!("{} and :{last}", lines.join(", :")),
        last => last.unwrap_or_default(),
    }
}

/// `items` as `a`, `a and b`, or `a, b and c`.
fn and_list(mut items: Vec<String>) -> String {
    match items.pop() {
        Some(last) if !items.is_empty() => format!("{} and {last}", items.join(", ")),
        last => last.unwrap_or_default(),
    }
}

/// `items`, at most `cap` of them, the rest counted: `a, b, c and 2 more`.
fn capped_list(items: &[String], cap: usize) -> String {
    let mut shown: Vec<String> = items.iter().take(cap).cloned().collect();
    if items.len() > cap {
        shown.push(format!("{} more", items.len() - cap));
    }
    and_list(shown)
}

/// `node`'s unbound calls counted by what the graph knows of them:
/// interfaces first, then library types, then values of unknown type.
/// One kind alone reads `all through interface encoder
/// (zstd/encoder.go:32)`; several read `13 through interface Resetter
/// (flate/inflate.go:73), 41 on standard-library or dependency types
/// (bytes.Buffer, bufio.Reader and 6 more) and 16 on values whose type the
/// graph does not know`.
fn go_unbound_counts(unbound: &[(GraphEdge, GoUnbound)]) -> String {
    let mut interfaces: Vec<(&GoUnbound, usize)> = Vec::new();
    let mut owners: Vec<(&str, usize)> = Vec::new();
    let mut libraries = Vec::new();
    let mut unknown = Vec::new();
    for (_, kind) in unbound {
        match kind {
            GoUnbound::Interface { .. } => {
                match interfaces.iter_mut().find(|(seen, _)| *seen == kind) {
                    Some((_, count)) => *count += 1,
                    None => interfaces.push((kind, 1)),
                }
            }
            GoUnbound::Library { owner, .. } => {
                libraries.push(kind);
                match owners.iter_mut().find(|(seen, _)| seen == owner) {
                    Some((_, count)) => *count += 1,
                    None => owners.push((owner, 1)),
                }
            }
            GoUnbound::Unknown { .. } => unknown.push(kind),
        }
    }
    // One kind of call, described once.
    let first = &unbound[0].1;
    if unbound.iter().all(|(_, kind)| kind == first) {
        let all = if unbound.len() == 1 { "" } else { "all " };
        return format!("{all}{}", first.describe());
    }
    interfaces.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    owners.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    // Up to four interfaces by name and place; past four, three and the
    // rest by name.
    let named = if interfaces.len() <= 4 { 4 } else { 3 };
    let mut parts: Vec<String> = interfaces
        .iter()
        .take(named)
        .map(|(kind, count)| format!("{count} {}", kind.describe()))
        .collect();
    let other_interfaces = &interfaces[interfaces.len().min(named)..];
    if !other_interfaces.is_empty() {
        let count: usize = other_interfaces.iter().map(|(_, count)| count).sum();
        let names: Vec<String> = other_interfaces
            .iter()
            .filter_map(|(kind, _)| match kind {
                GoUnbound::Interface { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        parts.push(format!(
            "{count} through interfaces {}",
            capped_list(&names, 3)
        ));
    }
    if let [(_, count)] = owners.as_slice() {
        parts.push(format!("{count} {}", libraries[0].describe()));
    } else if !owners.is_empty() {
        let names: Vec<String> = owners.iter().map(|(owner, _)| owner.to_string()).collect();
        parts.push(format!(
            "{} on standard-library or dependency types ({})",
            libraries.len(),
            capped_list(&names, 3)
        ));
    }
    match unknown.as_slice() {
        [] => {}
        [only] => parts.push(format!("1 {}", only.describe())),
        many => parts.push(format!(
            "{} on values whose type the graph does not know",
            many.len()
        )),
    }
    and_list(parts)
}

/// How a call through an interface, or on a library type, can reach
/// `node`: through the method its value's type has, which is `node` only
/// where that type is `node`'s or embeds it. Empty when no unbound call is
/// either.
fn go_reach_clause(node: &GraphNode, unbound: &[(GraphEdge, GoUnbound)]) -> String {
    let Some(owner) = node.parent_name.as_deref() else {
        return String::new();
    };
    let through = unbound
        .iter()
        .any(|(_, kind)| matches!(kind, GoUnbound::Interface { .. }));
    let library = unbound
        .iter()
        .any(|(_, kind)| matches!(kind, GoUnbound::Library { .. }));
    let which = match (through, library) {
        (true, false) => "a call through an interface",
        (false, true) => "a call on a library type, if that type is an interface,",
        (true, true) => "a call through an interface, or on a library type that is one,",
        (false, false) => return String::new(),
    };
    format!(
        "; {which} reaches {owner}.{name} only where the value's dynamic type is {owner} or \
         embeds it",
        name = node.name
    )
}

/// The sentence a closed Go `callers_of` ends with: what the graph checked
/// for `node`'s name, and where the calls it left unbound are and what they
/// are. `rows` are the reply's rows by qualified name, and `by_name` says
/// that every row is a match of the name (`match: "bare_name"`).
fn go_callers_note(
    node: &GraphNode,
    unbound: &[(GraphEdge, GoUnbound)],
    rows: &HashSet<&str>,
    by_name: bool,
    built_only: bool,
) -> String {
    let name = &node.name;
    let scope = if built_only && name.starts_with(char::is_uppercase) {
        "the directories the Go tools build for this module".to_string()
    } else if name.starts_with(char::is_uppercase) {
        "the repository".to_string()
    } else {
        let package = Path::new(&node.file_path)
            .parent()
            .and_then(Path::to_str)
            .filter(|dir| !dir.is_empty())
            .unwrap_or(".");
        format!("package {package}")
    };
    // Calls on a library type the code writes as no interface are that
    // type's methods (#329): counted apart, never a caller that may be
    // missing.
    let (concrete, open): (Vec<_>, Vec<_>) = unbound
        .iter()
        .partition(|(_, kind)| matches!(kind, GoUnbound::Library { concrete: true, .. }));
    let concrete_owners = go_concrete_owners(&concrete);
    let unbound: Vec<(GraphEdge, GoUnbound)> = open.into_iter().cloned().collect();
    let unbound = unbound.as_slice();
    if unbound.is_empty() {
        // A Go method call is bound only on a receiver of a type the code
        // gives it (a parameter, a binding, a field, what a function
        // returns); a plain call by its package.
        let how = if node.parent_name.is_some() {
            ", a method call by its receiver's static type"
        } else {
            ""
        };
        if concrete.is_empty() {
            return format!(
                "No direct caller the graph records is missing; every call named {name} in \
                 {scope}, _test.go files included, is bound to a definition{how}"
            );
        }
        let (left, verb) = match concrete.len() {
            1 => ("the 1 that goes".to_string(), "is"),
            count => (format!("the {count} that go"), "are"),
        };
        return format!(
            "No direct caller the graph records is missing; every other call named {name} in \
             {scope}, _test.go files included, is bound to a definition{how}; {left} to no \
             definition {verb} on {concrete_owners}, which cannot reach {}",
            go_target(node)
        );
    }
    let also = if concrete.is_empty() {
        String::new()
    } else {
        format!(
            "; another {} {} to no definition on {concrete_owners}, which cannot reach {}",
            concrete.len(),
            if concrete.len() == 1 { "goes" } else { "go" },
            go_target(node)
        )
    };
    let reach = go_reach_clause(node, unbound);
    let unknown: Vec<&GraphEdge> = unbound
        .iter()
        .filter(|(_, kind)| matches!(kind, GoUnbound::Unknown { .. }))
        .map(|(edge, _)| edge)
        .collect();
    let mut sources: Vec<&str> = Vec::new();
    for (edge, _) in unbound {
        if !sources.contains(&edge.source_qualified.as_str()) {
            sources.push(&edge.source_qualified);
        }
    }
    // Every unbound call is in a function the reply lists: the list holds
    // every caller there is, and what is left open is whether those rows
    // call it.
    if sources.iter().all(|source| rows.contains(source)) {
        let rows_are = if by_name {
            "the rows are matched by name (match \"bare_name\"), not bound by type: "
        } else {
            ""
        };
        let count = match unbound.len() {
            1 => format!("their 1 call of {name} that goes to no definition is"),
            all => format!("their {all} calls of {name} that go to no definition are"),
        };
        let read = match unknown.len() {
            0 => String::new(),
            1 => format!(
                "; read the line on a value of unknown type to decide whether that row calls {name}"
            ),
            _ => format!(
                "; read the lines on values of unknown type to decide whether those rows call {name}"
            ),
        };
        return format!(
            "Every call named {name} in {scope}, _test.go files included, is bound to a \
             definition or lies in a function listed here, so no caller is missing from the \
             list; {rows_are}{count} {}{reach}{read}{also}",
            go_unbound_counts(unbound)
        );
    }
    // `go_unbound_calls` holds every call of the name in scope that no
    // definition takes, so a short list of its lines is all there is to
    // read; a long one is counted instead.
    if unbound.len() <= GO_LISTED_UNBOUND_CALLS {
        // By kind, then by the function the calls are in.
        let mut groups: Vec<(&GoUnbound, &str, Vec<&GraphEdge>)> = Vec::new();
        for (edge, kind) in unbound {
            match groups
                .iter_mut()
                .find(|(seen, source, _)| *seen == kind && *source == edge.source_qualified)
            {
                Some((_, _, edges)) => edges.push(edge),
                None => groups.push((kind, &edge.source_qualified, vec![edge])),
            }
        }
        let mut kinds: Vec<&GoUnbound> = Vec::new();
        for (kind, _, _) in &groups {
            if !kinds.contains(kind) {
                kinds.push(kind);
            }
        }
        let places = kinds
            .iter()
            .map(|kind| {
                let at = groups
                    .iter()
                    .filter(|(seen, _, _)| seen == kind)
                    .map(|(_, source, edges)| {
                        let function = source.split_once("::").map_or(*source, |(_, name)| name);
                        let itself = if *source == node.qualified_name {
                            " itself"
                        } else {
                            ""
                        };
                        format!(
                            "{}:{} in {function}{itself}",
                            edges[0].file_path,
                            go_call_lines(edges)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("at {at}, {}", kind.describe())
            })
            .collect::<Vec<_>>()
            .join("; and ");
        let (left, verb) = match unbound.len() {
            1 => ("the 1 that goes".to_string(), "is"),
            count => (format!("the {count} that go"), "are"),
        };
        let read = match (unknown.len(), unknown.len() == unbound.len()) {
            (0, _) => String::new(),
            (1, true) => format!(": read that line to decide whether it calls {name}"),
            (_, true) => format!(": read those lines to decide whether they call {name}"),
            (1, false) => format!(
                "; read the line on a value of unknown type to decide whether it calls {name}"
            ),
            (_, false) => format!(
                "; read the lines on values of unknown type to decide whether they call {name}"
            ),
        };
        return format!(
            "Every other call named {name} in {scope}, _test.go files included, is bound to a \
             definition; {left} to no definition {verb} {places}{reach}{read}{also}"
        );
    }
    // The functions the reply does not list, those with calls on values of
    // unknown type first.
    let mut to_read: Vec<&str> = sources
        .iter()
        .copied()
        .filter(|source| !rows.contains(source))
        .collect();
    to_read.sort_by_key(|source| !unknown.iter().any(|edge| edge.source_qualified == *source));
    let shown = to_read
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let more = match to_read.len().saturating_sub(3) {
        0 => String::new(),
        rest => format!(" and {rest} more"),
    };
    let calls = match unbound.len() {
        1 => "1 call named".to_string(),
        count => format!("{count} calls named"),
    };
    let verb = if unbound.len() == 1 { "goes" } else { "go" };
    let scope = if built_only {
        scope
    } else {
        "the repository".to_string()
    };
    let read = if to_read.is_empty() {
        String::new()
    } else {
        format!(", so callers may be missing: read {shown}{more}")
    };
    format!(
        "{calls} {name} {verb} to no definition in {scope}, {}{reach}{read}{also}",
        go_unbound_counts(unbound)
    )
}

/// `Context.Run` for a method, `Run` for a function.
fn go_target(node: &GraphNode) -> String {
    match node.parent_name.as_deref() {
        Some(owner) => format!("{owner}.{}", node.name),
        None => node.name.clone(),
    }
}

/// The library types of calls on values the code writes as no interface,
/// most calls first: `the concrete standard-library type testing.T`, or
/// `concrete library types (testing.T and testing.B)`.
fn go_concrete_owners(concrete: &[&(GraphEdge, GoUnbound)]) -> String {
    let mut owners: Vec<(&str, bool, usize)> = Vec::new();
    for (_, kind) in concrete {
        if let GoUnbound::Library { owner, stdlib, .. } = kind {
            match owners.iter_mut().find(|(seen, _, _)| seen == owner) {
                Some((_, _, count)) => *count += 1,
                None => owners.push((owner, *stdlib, 1)),
            }
        }
    }
    owners.sort_by_key(|(_, _, count)| std::cmp::Reverse(*count));
    match owners.as_slice() {
        [(owner, true, _)] => format!("the concrete standard-library type {owner}"),
        [(owner, false, _)] => format!("the concrete dependency type {owner}"),
        many => {
            let names: Vec<String> = many.iter().map(|(owner, _, _)| owner.to_string()).collect();
            format!("concrete library types ({})", capped_list(&names, 3))
        }
    }
}

/// The first `next` call of a closed Go `callers_of` with unbound calls:
/// the first function outside `rows` with a call on a value of unknown type,
/// else the first interface a call goes through, with why.
fn go_unbound_next(
    node: &GraphNode,
    unbound: &[(GraphEdge, GoUnbound)],
    rows: &HashSet<&str>,
) -> Option<(String, String)> {
    let unknown = unbound
        .iter()
        .filter(|(_, kind)| matches!(kind, GoUnbound::Unknown { .. }))
        .find(|(edge, _)| !rows.contains(edge.source_qualified.as_str()));
    if let Some((first, _)) = unknown {
        let why = if unbound.len() <= GO_LISTED_UNBOUND_CALLS {
            let edges: Vec<&GraphEdge> = unbound
                .iter()
                .filter(|(edge, kind)| {
                    edge.source_qualified == first.source_qualified
                        && matches!(kind, GoUnbound::Unknown { .. })
                })
                .map(|(edge, _)| edge)
                .collect();
            format!(
                "the calls named {} at {}:{} go to no definition and may call it",
                node.name,
                first.file_path,
                go_call_lines(&edges)
            )
        } else {
            "a call there is bound to no definition and may call it".to_string()
        };
        return Some((first.source_qualified.clone(), why));
    }
    let owner = node.parent_name.as_deref()?;
    unbound.iter().find_map(|(_, kind)| match kind {
        GoUnbound::Interface {
            name, qualified, ..
        } => Some((
            qualified.clone(),
            format!(
                "whether {owner}, or a type that embeds it, implements {name} decides whether \
                 the calls through {name} reach it"
            ),
        )),
        _ => None,
    })
}

/// The summary sentence for a node the graph has but that no edge of
/// `pattern`'s kind reaches (#195): the node is there, the edges are not.
pub(super) fn no_edges_note(
    pattern: &str,
    language: &str,
    import_kind: Option<&str>,
) -> Option<String> {
    let caveat = edge_caveat(pattern, language, import_kind);
    Some(match pattern {
        "callers_of" => format!(
            "The node is in the graph and nothing in it calls it; {caveat}. A use as a \
             value (a callback, a stored method) is not a call edge, so \
             callers may be missing: read where the name is used"
        ),
        "callees_of" => {
            format!("The node is in the graph and calls nothing it records; {caveat}")
        }
        "importers_of" => {
            format!("The node is in the graph and nothing in it imports it; {caveat}")
        }
        "imports_of" => {
            format!("The node is in the graph and imports nothing it records; {caveat}")
        }
        "tests_for" => "The node is in the graph and no test in it reaches it".to_string(),
        "source_of" | "file_summary" => return None,
        _ => format!("The node is in the graph, which records no {pattern} edges for it"),
    })
}

/// The summary's first sentence: how many rows `pattern` found for
/// `target`, within how many hops and under which path, then the sentence
/// for a node no edge reaches.
pub(super) fn found_summary(
    count: usize,
    pattern: &str,
    target: &str,
    depth: i64,
    path_prefix: Option<&str>,
    empty_note: Option<&str>,
) -> String {
    let mut summary = format!("Found {count} result(s) for {pattern}('{target}')");
    if depth > 1 {
        summary.push_str(&format!(" within {depth} hops"));
    }
    if let Some(prefix) = path_prefix {
        summary.push_str(&format!(" under '{prefix}'"));
    }
    if let Some(note) = empty_note {
        summary.push_str(&format!(". {note}"));
    }
    summary
}

/// What `tests_for` says of a node no test calls but whose callers run
/// under tests, instead of "no test reaches it": the tests through them,
/// from the walk `test_reach` counts with (eval/g8_go/RESULTS.md, defect 4;
/// G10 H2, #326). A depth `callers_of` is no way to list them: it does not
/// walk past a value use, which this walk counts as a call.
pub(super) fn tests_through_callers_note(reaching: &Reaching, target: &str) -> String {
    let at_least = if reaching.truncated { "at least " } else { "" };
    format!(
        "No test calls it directly; {at_least}{} test(s) reach it through its callers \
         (test_reach.through_callers): {}",
        reaching.tests.len(),
        named_through_callers(reaching, target)
    )
}

/// What `tests_for` says of a node no test calls and no test reaches
/// through its `callers` callers: the nearest code in a test file that
/// does (a helper, which the walk does not go through), if any.
pub(super) fn no_tests_through_callers_note(
    nearest: Option<(&str, u64)>,
    callers: usize,
) -> String {
    match nearest {
        Some((helper, hops)) => format!(
            "No test calls it directly; {helper}, in a test file, reaches it through \
             {hops} caller hop(s), and the tests that call it are not walked"
        ),
        None => format!(
            "No test calls it directly, and none reaches it through its {callers} \
             caller(s) as the graph records them; calls dispatched at run time are not edges"
        ),
    }
}

/// What a walk says of a `depth` above the maximum it walks.
pub(super) fn depth_capped_note(requested: i64) -> String {
    format!(
        "depth {requested} is above the maximum, {MAX_DEPTH}, so the walk went {MAX_DEPTH} hops"
    )
}

/// Tests through callers the summary names; `test_reach.through_callers`
/// lists more.
const THROUGH_CALLERS_NAMED: usize = 3;

/// The first tests through callers by name, each with its hops and the
/// node it reaches the target through, and the first value use on its
/// path; then how many more, where the walk stopped early, and, if a named
/// path passes a value use, that the walk counted it as a call.
fn named_through_callers(reaching: &Reaching, target: &str) -> String {
    let count = reaching.tests.len();
    let mut value_use = false;
    let named: Vec<String> = reaching
        .tests
        .iter()
        .take(THROUGH_CALLERS_NAMED)
        .map(|test| {
            let hops = test.hops;
            let name = &test.test;
            let used = test.value_use.as_ref();
            value_use |= used.is_some();
            match used {
                _ if test.via == target => format!("{name} ({hops} hop(s), uses it)"),
                Some((user, value)) if user == name => {
                    format!("{name} ({hops} hop(s), uses {value} as a value)")
                }
                Some((user, value)) => format!(
                    "{name} ({hops} hop(s), through {}; {user} uses {value} as a value)",
                    test.via
                ),
                None => format!("{name} ({hops} hop(s), through {})", test.via),
            }
        })
        .collect();
    let rest = count.saturating_sub(named.len());
    let rest = if rest > 0 {
        format!(", and {rest} more")
    } else {
        String::new()
    };
    let stopped = if reaching.truncated {
        format!(
            "; the walk stopped at {} nodes",
            crate::findings::REACH_NODE_LIMIT
        )
    } else {
        String::new()
    };
    let counted = if value_use {
        ". The walk counts a value use as a call, though a stored value may never be \
         called: read where it goes"
    } else {
        ""
    };
    format!("{}{rest}{stopped}{counted}", named.join(", "))
}

/// What `tests_for` with rows says of the tests that are not rows but reach
/// the target through its callers (G10 H1, #327): the rows are the tests
/// linked to the target itself; these run it through a caller.
pub(super) fn through_callers_note(reaching: &Reaching, target: &str) -> String {
    let count = reaching.tests.len();
    let limit = reaching.limit;
    let at_least = if reaching.truncated { "at least " } else { "" };
    format!(
        "The rows are the tests linked to it directly; {at_least}{count} more test(s) reach \
         it through its callers within {limit} hop(s) and are not rows (test_reach.through_callers): \
         {}",
        named_through_callers(reaching, target)
    )
}

/// What `tests_for` says when every row is chosen by name (G10 H2, #328):
/// nothing in the graph links them, so the list is not the whole answer.
pub(super) fn by_name_note() -> &'static str {
    "Every row is chosen by name (coverage_source \"heuristic\"): no call, value use, or \
     dagayn: tests directive in the graph links one to it, so tests may be missing and \
     results_complete is false"
}

/// The page of rows a reply shows, where the budget cut them or `offset`
/// skipped some.
pub(super) struct Page {
    pub(super) offset: usize,
    /// Rows this page shows.
    pub(super) kept: u64,
    /// Rows there are, from the first.
    pub(super) total: u64,
    /// Whether the budget cut the rows.
    pub(super) cut: bool,
    /// Files the rows lie in, and how many of them `files` lists.
    pub(super) files: usize,
    pub(super) files_shown: usize,
}

/// The sentence for a page of rows: which rows it shows, how to get more,
/// and how many files all of them lie in.
pub(super) fn page_note(page: &Page, minimal: bool) -> String {
    let Page {
        offset,
        kept,
        total,
        cut,
        files,
        files_shown,
    } = *page;
    let shown = if offset == 0 {
        format!("Showing {kept} of {total} rows")
    } else if kept == 0 {
        format!("Showing no rows: offset {offset} is past the {total} rows")
    } else {
        format!(
            "Showing rows {}-{} of {total}",
            offset + 1,
            offset as u64 + kept
        )
    };
    let rest = if minimal && cut {
        "; detail_level=\"standard\" lists more in one call"
    } else if cut {
        "; the offset call in next lists the rows after these"
    } else {
        ""
    };
    let more = if files > files_shown {
        format!(" (first {files_shown} in files)")
    } else {
        " (files)".to_string()
    };
    format!("{shown}{rest}; all {total} lie in {files} files{more}")
}

/// The sentence for a walk its own row limit stopped: what is past the
/// limit is on no page.
pub(super) fn walk_cut_note() -> String {
    format!(
        "The walk stopped at {TRANSITIVE_ROW_LIMIT} rows past the first hop, so nodes past \
         them are on no page; a smaller depth may give a closed list"
    )
}

/// The sentence a reply ends with, if any: that rows use the target as a
/// value, so calls through that value are missing; else, for a `closed`
/// answer (every row shown, nothing unresolved, the walk complete), that
/// the list is whole and what the graph leaves out for the target's
/// language. A Go `callers_of` says instead whether every call of its name
/// is bound, else where the unbound ones are (G8 defect 5,
/// eval/g8_go/RESULTS.md), and names the function to read first: the
/// `source_of` target and why.
pub(super) fn ending(
    store: &GraphStore,
    pattern: &str,
    node: Option<&GraphNode>,
    found: &Found,
    closed: bool,
    import_kind: Option<&str>,
    scope: Option<&mut GoScope>,
) -> Option<(String, Option<(String, String)>)> {
    // Counted by node, as `merge` folds the rows.
    let value_uses = found
        .rows
        .iter()
        .filter(|row| get(row, "relation") == Some(&json!(VALUE_REFERENCE)))
        .filter_map(|row| get(row, "qualified_name"))
        .collect::<HashSet<_>>()
        .len();
    if value_uses > 0 {
        // Past the first hop, a value use is of a caller (#326).
        let deeper = found.rows.iter().any(|row| {
            get(row, "relation") == Some(&json!(VALUE_REFERENCE))
                && get(row, "depth").and_then(Value::as_i64) > Some(1)
        });
        let note = if deeper {
            format!(
                "{value_uses} use it or a caller as a value (relation \"{VALUE_REFERENCE}\"); the \
                 walk does not go past them, and calls through that value are not edges, so \
                 callers may be missing: read where the value goes"
            )
        } else {
            format!(
                "{value_uses} use it as a value (relation \"{VALUE_REFERENCE}\"); calls through that \
                 value are not edges, so callers may be missing: read where the value goes"
            )
        };
        return Some((note, None));
    }
    if !closed {
        return None;
    }
    let walked = found.reachability.is_some();
    let language = node.map_or("", |node| node.language.as_str());
    // Go direct callers: the calls of the name no definition takes.
    let unbound = match language {
        "go" if pattern == "callers_of" && !walked => {
            node.and_then(|node| Some((node, go_unbound_calls(store, node)?)))
        }
        _ => None,
    };
    if let Some((node, mut unbound)) = unbound {
        // Calls where the Go tools do not look are not the module's (#325):
        // the summary neither counts them nor names them to read.
        let before = unbound.len();
        if let Some(scope) = scope {
            unbound.retain(|(edge, _)| scope.outside(&edge.file_path).is_none());
        }
        let built_only = unbound.len() < before;
        let rows: HashSet<&str> = found
            .rows
            .iter()
            .filter_map(|row| get(row, "qualified_name").and_then(Value::as_str))
            .collect();
        let by_name = !found.rows.is_empty()
            && found
                .rows
                .iter()
                .all(|row| get(row, "match") == Some(&json!("bare_name")));
        return Some((
            go_callers_note(node, &unbound, &rows, by_name, built_only),
            go_unbound_next(node, &unbound, &rows),
        ));
    }
    closed_note(pattern, !walked, language, import_kind).map(|note| (note, None))
}
