//! `query_graph_tool` (`dagayn.tools.query.query_graph`): every pattern
//! (`tests_for` through [`crate::coverage`]), at every detail level and
//! depth, on a node named exactly or found by name.
//!
//! The Python tool opens the graph and leaves every answer to this.

mod notes;
mod resolve;
mod rows;
mod walk;

use std::path::Path;

use dagayn_graph::{GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::{Args, Context, OpenGraph, Ordered, Payload, open_graph, resolve_repo};
use notes::{
    NO_MATCHING_EDGES, Page, ending, found_summary, guidance, no_edges_note, page_note,
    tests_through_callers_note, walk_cut_note,
};
pub(crate) use resolve::looks_like_file_target;
use resolve::{Resolution, resolve};
pub(crate) use rows::{
    Row, cross_artifact_role, edge_dict, edge_import_kind, import_kind_matches, node_dict,
    node_row, python_dumps_len, sanitize,
};
use rows::{compact, merge, object, project_minimal, row_file};
use walk::{Found, run_pattern};

/// `_BUILTIN_CALL_NAMES`: bare names `callers_of` skips.
const BUILTIN_CALL_NAMES: &[&str] = &[
    "abs",
    "addEventListener",
    "afterAll",
    "afterEach",
    "aggregate",
    "all",
    "allSettled",
    "any",
    "appendChild",
    "apply",
    "assert",
    "assign",
    "at",
    "beforeAll",
    "beforeEach",
    "bind",
    "call",
    "catch",
    "ceil",
    "charAt",
    "charCodeAt",
    "clear",
    "clearInterval",
    "clearTimeout",
    "close",
    "concat",
    "count",
    "create",
    "createElement",
    "createMany",
    "debug",
    "decodeURIComponent",
    "defineProperty",
    "delete",
    "deleteMany",
    "describe",
    "destroy",
    "dir",
    "emit",
    "encodeURIComponent",
    "end",
    "endsWith",
    "entries",
    "error",
    "every",
    "expect",
    "fetch",
    "fill",
    "filter",
    "finally",
    "find",
    "findFirst",
    "findIndex",
    "findMany",
    "findUnique",
    "flat",
    "flatMap",
    "floor",
    "forEach",
    "freeze",
    "from",
    "fromEntries",
    "get",
    "getAttribute",
    "getElementById",
    "getFullYear",
    "getOwnPropertyNames",
    "getTime",
    "groupBy",
    "has",
    "hasOwnProperty",
    "includes",
    "indexOf",
    "info",
    "is",
    "isArray",
    "isNaN",
    "it",
    "join",
    "json",
    "keys",
    "lastIndexOf",
    "log",
    "map",
    "match",
    "matchAll",
    "max",
    "min",
    "mock",
    "next",
    "now",
    "of",
    "off",
    "on",
    "once",
    "padEnd",
    "padStart",
    "parse",
    "parseFloat",
    "parseInt",
    "pipe",
    "pop",
    "pow",
    "preventDefault",
    "push",
    "querySelector",
    "querySelectorAll",
    "race",
    "random",
    "read",
    "redirect",
    "reduce",
    "reduceRight",
    "reject",
    "removeChild",
    "removeEventListener",
    "repeat",
    "replace",
    "replaceAll",
    "require",
    "resolve",
    "reverse",
    "round",
    "search",
    "send",
    "set",
    "setAttribute",
    "setInterval",
    "setTimeout",
    "shift",
    "slice",
    "some",
    "sort",
    "splice",
    "split",
    "spyOn",
    "sqrt",
    "startsWith",
    "status",
    "stopPropagation",
    "stringify",
    "substr",
    "substring",
    "table",
    "test",
    "then",
    "time",
    "timeEnd",
    "toFixed",
    "toISOString",
    "toJSON",
    "toLowerCase",
    "toString",
    "toUpperCase",
    "trace",
    "transaction",
    "trim",
    "trimEnd",
    "trimStart",
    "unshift",
    "update",
    "updateMany",
    "upsert",
    "valueOf",
    "values",
    "warn",
    "write",
];

/// `QUERY_PATTERNS`.
/// `QUERY_PATTERNS`' keys, in Python's order.
const PATTERNS: [&str; 12] = [
    "callers_of",
    "callees_of",
    "imports_of",
    "importers_of",
    "docs_for",
    "implementations_of",
    "bridges_from",
    "children_of",
    "tests_for",
    "inheritors_of",
    "file_summary",
    "source_of",
];

/// The error `query_graph` answers for an unknown pattern or a depth it
/// does not apply, checked in Python's order.
fn argument_error(pattern: &str, depth: i64) -> Option<String> {
    if description(pattern).is_none() {
        let listed: Vec<String> = PATTERNS.iter().map(|name| format!("'{name}'")).collect();
        return Some(format!(
            "Unknown pattern '{pattern}'. Available: [{}]",
            listed.join(", ")
        ));
    }
    if depth != 1 && !matches!(pattern, "callers_of" | "importers_of" | "imports_of") {
        return Some(format!(
            "depth applies only to ['callers_of', 'importers_of', 'imports_of']; '{pattern}' \
             returns direct relationships only."
        ));
    }
    (depth < 1).then(|| format!("depth must be 1 or more, got {depth}."))
}

/// The error for an `import_kind` the pattern does not take or that is not
/// one of the Markdown import kinds.
fn import_kind_error(pattern: &str, import_kind: Option<&str>) -> Option<String> {
    let kind = import_kind?;
    if !matches!(pattern, "importers_of" | "imports_of") {
        return Some(format!(
            "import_kind applies only to ['importers_of', 'imports_of']; '{pattern}' does not \
             take it."
        ));
    }
    (!matches!(
        kind,
        "directive" | "link" | "lazy_export" | "path" | "statement"
    ))
    .then(|| {
        format!(
            "import_kind must be 'directive', 'link', 'lazy_export', 'path', or 'statement', \
             got '{kind}'."
        )
    })
}

fn description(pattern: &str) -> Option<&'static str> {
    Some(match pattern {
        "callers_of" => "Find all functions that call a given function",
        "callees_of" => "Find all functions called by a given function",
        "imports_of" => "Find all imports of a given file or module",
        "importers_of" => "Find all files that import a given file or module",
        "docs_for" => "Find documentation linked to a code, Terraform, or artifact node",
        "implementations_of" => "Find implementation artifacts linked to a document node",
        "bridges_from" => {
            "Find high-confidence CROSS_ARTIFACT bridges from a node (Terraform maps_entrypoint / invokes_binary, and similar)"
        }
        "children_of" => "Find all nodes contained in a file or class",
        "tests_for" => "Find all tests for a given function or class",
        "inheritors_of" => "Find all classes that inherit from a given class",
        "file_summary" => "Get a summary of all nodes in a file",
        "source_of" => {
            "Fetch the live worktree source span for one node (function, class, section, or file)"
        }
        _ => return None,
    })
}

/// Patterns whose rows are one per edge and merge by endpoint.
const MERGED: &[&str] = &["callers_of", "callees_of", "inheritors_of", "importers_of"];
/// `MAX_QUERY_DEPTH`.
const MAX_DEPTH: i64 = 6;
/// Files a cut answer lists in `files` (at `minimal`: fewer).
const MAX_CUT_FILES: usize = 200;
const MAX_CUT_FILES_MINIMAL: usize = 60;

/// What a call asks for, once its arguments are checked.
struct Query<'a> {
    pattern: &'a str,
    /// The target as the reply names it: a node found by name goes by its
    /// qualified name.
    target: &'a str,
    detail_level: &'a str,
    full: bool,
    minimal: bool,
    depth: i64,
    offset: usize,
    import_kind: Option<&'a str>,
    path_prefix: Option<&'a str>,
}

pub(crate) fn query_graph(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(
        arguments,
        &[
            "pattern",
            "target",
            "repo_root",
            "detail_level",
            "depth",
            "import_kind",
            "path_prefix",
            "offset",
        ],
    )?;
    let pattern = args.string("pattern")?;
    let target = args.string("target")?;
    let detail_level = match arguments.get("detail_level") {
        None => "standard",
        Some(Value::String(level)) => level.as_str(),
        // Python treats a level it does not know as `standard`.
        Some(_) => "standard",
    };
    let depth = args.integer("depth", 1)?;
    let offset = args.integer("offset", 0)?;
    let import_kind = args.optional_string("import_kind")?;
    let path_prefix = args
        .optional_string("path_prefix")?
        .filter(|prefix| !prefix.is_empty());
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    // Python checks the pattern and depth once the store is open, whatever
    // the checkout.
    let graph = open_graph(&root)?;
    if let Some(message) = argument_error(pattern, depth)
        .or_else(|| import_kind_error(pattern, import_kind))
        .or_else(|| (offset < 0).then(|| format!("offset must be 0 or more, got {offset}.")))
    {
        return Some(error_answer(message, &graph));
    }
    let store = &graph.store;
    let answerability = graph.answerability()?;
    if pattern == "callers_of" && !target.contains("::") && BUILTIN_CALL_NAMES.contains(&target) {
        return Some(builtin_answer(pattern, target, &answerability, &graph));
    }

    let (node, resolution) =
        match resolve(&graph, &root, arguments, pattern, target, &answerability)? {
            Ok(resolved) => resolved,
            Err(answer) => return Some(answer),
        };
    let query = Query {
        pattern,
        // A name-resolved target is reported by its qualified name.
        target: match (&node, resolution) {
            (Some(node), Resolution::ExactName | Resolution::Fuzzy) => &node.qualified_name,
            _ => target,
        },
        detail_level,
        // Python treats any other level as `standard`.
        full: detail_level == "full",
        minimal: detail_level == "minimal",
        depth: depth.min(MAX_DEPTH),
        offset: usize::try_from(offset).unwrap_or(0),
        import_kind,
        path_prefix,
    };
    let exact_count = i64::from(
        matches!(resolution, Resolution::Exact | Resolution::ExactName) && node.is_some(),
    );
    let node = node.as_ref();
    let mut found = run_pattern(
        store,
        &root,
        pattern,
        query.target,
        node,
        query.depth,
        import_kind,
    )?;
    // Rows outside the scope asked for go after the walk, so a chain through
    // a file outside it still reaches the files inside.
    if let Some(prefix) = path_prefix {
        found.keep_under(pattern, prefix);
    }

    // `query_zero_result_fields`.
    let (empty_note, tests_through_callers) =
        zero_result_note(store, &root, &query, node, &found, exact_count)?;
    let (confidence, zero_result_reason) = confidence(&found, empty_note.is_some());
    let mut payload = Ordered::default()
        .put("status", "ok")
        .put("pattern", pattern)
        .put("target", query.target)
        .put("unresolved_count", found.unresolved.len())
        .put("unresolved_targets", json!(found.unresolved))
        .put("confidence", confidence)
        .put("zero_result_reason", zero_result_reason)
        .put("next", json!([]))
        .put("resolution", resolution.as_str())
        .put("exact_match_count", exact_count);
    if matches!(resolution, Resolution::ExactName | Resolution::Fuzzy) {
        payload = payload.put("resolved_target", query.target);
        payload = payload.put("original_target", target);
    }
    if let Some(reachability) = &found.reachability {
        payload = payload
            .put("depth", query.depth)
            .put("reachability", reachability.clone());
    }
    let empty_note = empty_note.as_deref();
    let (payload, budget) = put_rows(
        payload,
        &query,
        &found,
        empty_note,
        &answerability,
        exact_count,
    );
    let (mut payload, paging) = page(payload, budget, &found.rows, query.offset, query.minimal);
    // The walk's own row limit bounds what any page can show.
    if found.walk_cut() {
        payload = append_summary(payload, &walk_cut_note());
    }
    // Say that a closed answer is the whole answer: agents re-checked lists
    // the graph had already completed
    // (docs/plans/AGENT-WORKFLOW-TARGET.md#order-of-work, step 10).
    let closed = paging.results_complete && found.closed();
    let mut unbound_caller = None;
    if let Some((note, read_first)) = ending(store, pattern, node, &found, closed, import_kind) {
        payload = append_summary(payload, &note);
        unbound_caller = read_first;
    }
    let rows = payload
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let next = next_calls(
        &query,
        &found,
        &rows,
        &paging,
        unbound_caller,
        tests_through_callers,
    );
    payload = payload.replace("next", next);
    Some(finish(payload, found, &answerability, &graph))
}

/// The answer for arguments `query_graph` does not take.
fn error_answer(message: String, graph: &OpenGraph) -> Payload {
    Ordered::default()
        .put("status", "error")
        .put("error", message)
        .put("_repo", graph.repo_context())
        .into_payload()
}

/// The reply's last fields: what `tests_for` found of the nearest test,
/// what the graph and a `source_of` read leave out, and the repository.
fn finish(
    mut payload: Ordered,
    found: Found,
    answerability: &Answerability,
    graph: &OpenGraph,
) -> Payload {
    if let Some(reach) = found.test_reach.clone() {
        payload = payload.put("test_reach", reach);
    }
    let mut missingness = answerability.missingness();
    // `_attach_source_of_coverage`.
    if let Some(coverage) = found.source
        && !found.rows.is_empty()
    {
        missingness.extend(coverage.missingness());
        if coverage.read_error.is_some() || coverage.stale {
            payload = payload.replace("status", json!("degraded"));
        }
        payload = payload.put("source_coverage", coverage.value());
    }
    payload
        .put("missingness", json!(missingness))
        .put("_repo", graph.repo_context())
        .into_payload()
}

/// The answer for `callers_of` on a common builtin name: skipped.
fn builtin_answer(
    pattern: &str,
    target: &str,
    answerability: &Answerability,
    graph: &OpenGraph,
) -> Payload {
    Ordered::default()
        .put("status", "ok")
        .put("pattern", pattern)
        .put("target", target)
        .put("description", description(pattern).unwrap_or(""))
        .put(
            "summary",
            format!("'{target}' is a common builtin — callers_of skipped to avoid noise."),
        )
        .put("results", json!([]))
        .put("edges", json!([]))
        .put("answerability", answerability.full())
        .put("missingness", json!(answerability.missingness()))
        .put("_repo", graph.repo_context())
        .into_payload()
}

/// The summary sentence for a node the graph has that no edge of the kind
/// asked for reaches, and whether it is the `tests_for` one that points at
/// the tests through the node's callers.
fn zero_result_note(
    store: &GraphStore,
    root: &Path,
    query: &Query,
    node: Option<&GraphNode>,
    found: &Found,
    exact_count: i64,
) -> Option<(Option<String>, bool)> {
    let empty_note = node
        .filter(|_| found.rows.is_empty() && found.unresolved.is_empty() && exact_count > 0)
        .and_then(|node| no_edges_note(query.pattern, &node.language, query.import_kind));
    // A node no test calls can still run under the tests that call its
    // callers: say so, and name the walk that lists them.
    if query.pattern == "tests_for" && empty_note.is_some() {
        let callers = run_pattern(store, root, "callers_of", query.target, node, 1, None)?;
        if !callers.rows.is_empty() {
            let nearest = found
                .test_reach
                .as_ref()
                .and_then(|reach| Some((reach["nearest_test"].as_str()?, reach["hops"].as_u64()?)));
            let note = tests_through_callers_note(nearest, callers.rows.len());
            return Some((Some(note), true));
        }
    }
    Some((empty_note, false))
}

/// The reply's `confidence` and `zero_result_reason`.
fn confidence(found: &Found, no_edges: bool) -> (&'static str, Value) {
    if !found.rows.is_empty() {
        let high = !found.edges.is_empty()
            && found.edges.iter().all(|edge| {
                matches!(
                    edge.get("confidence_tier")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                    "EXACT" | "EXTRACTED" | "HIGH"
                )
            });
        (if high { "high" } else { "medium" }, Value::Null)
    } else if !found.unresolved.is_empty() {
        ("medium", json!("unresolved_endpoints_only"))
    } else if no_edges {
        ("low", json!(NO_MATCHING_EDGES))
    } else {
        ("low", json!("not_found_in_current_graph"))
    }
}

/// The rows at the detail level asked for, from `offset` on, with the
/// summary (ending in `empty_note`) and guidance for how many there are,
/// and the token budget the reply is cut to.
fn put_rows(
    payload: Ordered,
    query: &Query,
    found: &Found,
    empty_note: Option<&str>,
    answerability: &Answerability,
    exact_count: i64,
) -> (Ordered, usize) {
    let (pattern, target, offset) = (query.pattern, query.target, query.offset);
    let summary = |count| {
        let (depth, prefix) = (query.depth, query.path_prefix);
        found_summary(count, pattern, target, depth, prefix, empty_note)
    };
    let raw_count = found.rows.len();
    if query.full {
        let edges: Vec<Value> = found.edges.iter().cloned().map(Value::Object).collect();
        let rows: Vec<Value> = found
            .rows
            .iter()
            .skip(offset)
            .cloned()
            .map(object)
            .collect();
        let payload = payload
            .put("description", description(pattern).unwrap_or(""))
            .put("summary", summary(raw_count))
            .put("result_count", raw_count)
            .put("answerability", answerability.full())
            .put("results", json!(rows))
            .put("edges", json!(edges))
            .put(
                "guidance",
                guidance(pattern, target, raw_count, exact_count),
            );
        return (payload, 8000);
    }
    let mut rows: Vec<Row> = found
        .rows
        .iter()
        .map(|row| compact(row, query.minimal))
        .collect();
    if MERGED.contains(&pattern) {
        rows = merge(rows);
    }
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            if query.minimal {
                project_minimal(row)
            } else {
                object(row)
            }
        })
        .collect();
    let rows_len = rows.len();
    let rows: Vec<Value> = rows.into_iter().skip(offset).collect();
    let mut payload = payload
        .put("summary", summary(rows_len))
        .put("result_count", rows_len)
        .put("answerability", answerability.compact())
        .put("results", json!(rows));
    if query.minimal {
        return (payload, crate::MINIMAL_BUDGET);
    }
    // One count per reply: the rows, after `merge` folded the edges of one
    // node into one row.
    payload = payload.put("guidance", guidance(pattern, target, rows_len, exact_count));
    (payload, crate::STANDARD_BUDGET)
}

/// What the budget and `offset` left of the rows.
struct Paging {
    /// The rows kept and the rows there were, where the budget cut them.
    cut: Option<(u64, u64)>,
    /// The rows a cut or offset page shows.
    kept: u64,
    results_complete: bool,
}

/// Cut the reply to `budget`. Where that cuts the rows, or `offset` skips
/// some, list the files of every row and say which rows this page shows.
fn page(
    payload: Ordered,
    budget: usize,
    rows: &[Row],
    offset: usize,
    minimal: bool,
) -> (Ordered, Paging) {
    let mut payload = payload.apply_output_budget(budget, &["results", "edges"]);
    // A page after the first leaves out the rows before it.
    let results_complete = offset == 0
        && !payload
            .get("_truncation")
            .and_then(Value::as_object)
            .is_some_and(|truncation| truncation.contains_key("results"));
    payload = payload.put("results_complete", results_complete);
    // Say that a cut list is cut, and how to get the rest: agents given a
    // cut minimal list walked the graph one hop at a time instead
    // (docs/plans/AGENT-WORKFLOW-TARGET.md#order-of-work, step 10).
    let cut = payload
        .get("_truncation")
        .and_then(|truncation| truncation.get("results"))
        .and_then(|results| {
            Some((
                results.get("kept")?.as_u64()?,
                results.get("total")?.as_u64()?,
            ))
        });
    let shown_rows = payload
        .get("results")
        .and_then(Value::as_array)
        .map_or(0, |rows| rows.len() as u64);
    let mut paging = Paging {
        cut,
        kept: 0,
        results_complete,
    };
    let Some((kept, rest)) = cut.or_else(|| (offset > 0).then_some((shown_rows, shown_rows)))
    else {
        return (payload, paging);
    };
    if payload.get("summary").and_then(Value::as_str).is_none() {
        return (payload, paging);
    }
    // The files of every row, cut or not: a question about which files
    // gets its whole answer even when the rows do not fit.
    let mut files: Vec<&str> = rows.iter().filter_map(row_file).collect();
    files.sort_unstable();
    files.dedup();
    let cap = if minimal {
        MAX_CUT_FILES_MINIMAL
    } else {
        MAX_CUT_FILES
    };
    let shown = files.len().min(cap);
    let count = files.len();
    files.truncate(cap);
    // The file list comes before more rows: cut the rows again to fit it.
    payload = payload
        .put("files", json!(files))
        .apply_output_budget(budget, &["results", "edges"]);
    let kept = payload
        .get("results")
        .and_then(Value::as_array)
        .map_or(kept, |rows| rows.len() as u64);
    paging.kept = kept;
    let total = offset as u64 + rest;
    let mut record = json!({"kept": kept, "total": total});
    if offset > 0 {
        record["offset"] = json!(offset);
    }
    let mut truncation = payload
        .get("_truncation")
        .cloned()
        .unwrap_or_else(|| json!({}));
    truncation["results"] = record;
    payload = payload.set("_truncation", truncation);
    let page = Page {
        offset,
        kept,
        total,
        cut: cut.is_some(),
        files: count,
        files_shown: shown,
    };
    (append_summary(payload, &page_note(&page, minimal)), paging)
}

/// `payload` with `sentence` added to its summary.
fn append_summary(payload: Ordered, sentence: &str) -> Ordered {
    match payload.get("summary").and_then(Value::as_str) {
        Some(summary) => {
            let summary = format!("{summary}. {sentence}");
            payload.replace("summary", json!(summary))
        }
        None => payload,
    }
}

/// `next`: the calls `crate::next::after_query` picks for the rows shown,
/// after the ones this reply leaves open, the last of these first: the
/// tests through the target's callers, the function a Go call left unbound
/// is in, the rows past a cut page, and the walk past the depth asked for.
fn next_calls(
    query: &Query,
    found: &Found,
    rows: &[Value],
    paging: &Paging,
    unbound_caller: Option<(String, String)>,
    tests_through_callers: bool,
) -> Value {
    let Query {
        pattern,
        target,
        depth,
        offset,
        ..
    } = *query;
    let mut next = crate::next::after_query(pattern, target, rows);
    // This call's arguments, for a call that goes on from it.
    let this_call = |pattern: &str, depth: Option<i64>| {
        let mut args = json!({"pattern": pattern, "target": target});
        if let Some(depth) = depth {
            args["depth"] = json!(depth);
        }
        if let Some(kind) = query.import_kind {
            args["import_kind"] = json!(kind);
        }
        if let Some(prefix) = query.path_prefix {
            args["path_prefix"] = json!(prefix);
        }
        args
    };
    let mut first = |call: Value| {
        if let Some(calls) = next.as_array_mut() {
            calls.insert(0, call);
            calls.truncate(crate::next::MAX_NEXT);
        }
    };
    // A chain cut at the depth asked for goes on: the deeper call first.
    if let Some(reachability) = &found.reachability
        && reachability.get("depth_limit_reached") == Some(&Value::Bool(true))
        && reachability.get("truncated") != Some(&Value::Bool(true))
        && depth < MAX_DEPTH
    {
        first(crate::next::call(
            "query_graph_tool",
            this_call(pattern, Some(MAX_DEPTH)),
            format!("nodes beyond {depth} hops may reach it"),
        ));
    }
    let walked = found.reachability.is_some().then_some(depth);
    if paging.cut.is_some() && query.minimal {
        let mut args = this_call(pattern, walked);
        args["detail_level"] = json!("standard");
        if offset > 0 {
            args["offset"] = json!(offset);
        }
        first(crate::next::call(
            "query_graph_tool",
            args,
            "the rows minimal cut to fit",
        ));
    } else if paging.cut.is_some() && paging.kept > 0 {
        let mut args = this_call(pattern, walked);
        if query.detail_level != "standard" {
            args["detail_level"] = json!(query.detail_level);
        }
        let after = offset as u64 + paging.kept;
        args["offset"] = json!(after);
        first(crate::next::call(
            "query_graph_tool",
            args,
            format!("the rows after the first {after}"),
        ));
    }
    if let Some((caller, why)) = unbound_caller {
        first(crate::next::call(
            "query_graph_tool",
            json!({"pattern": "source_of", "target": caller}),
            why,
        ));
    }
    if tests_through_callers {
        first(crate::next::call(
            "query_graph_tool",
            this_call("callers_of", Some(MAX_DEPTH)),
            "the tests that reach it through its callers",
        ));
    }
    next
}
