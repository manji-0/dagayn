//! `query_graph_tool` (`dagayn.tools.query.query_graph`): every pattern
//! (`tests_for` through [`crate::coverage`]), at every detail level and
//! depth, on a node named exactly or found by name.
//!
//! The Python tool opens the graph and leaves every answer to this.

mod notes;
mod resolve;
mod rows;
mod walk;

use serde_json::{Map, Value, json};

use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo};
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
use rows::{compact, get, merge, object, project_minimal, row_file};
use walk::run_pattern;

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
    // Python treats any other level as `standard`.
    let (full, minimal) = (detail_level == "full", detail_level == "minimal");
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
        return Some(
            Ordered::default()
                .put("status", "error")
                .put("error", message)
                .put("_repo", graph.repo_context())
                .into_payload(),
        );
    }
    let depth = depth.min(MAX_DEPTH);
    let offset = usize::try_from(offset).unwrap_or(0);
    let store = &graph.store;
    let answerability = graph.answerability()?;

    if pattern == "callers_of" && !target.contains("::") && BUILTIN_CALL_NAMES.contains(&target) {
        return Some(
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
                .into_payload(),
        );
    }

    let (node, resolution) =
        match resolve(&graph, &root, arguments, pattern, target, &answerability)? {
            Ok(resolved) => resolved,
            Err(answer) => return Some(answer),
        };
    // A name-resolved target is reported by its qualified name.
    let original_target = target;
    let target: &str = match (&node, resolution) {
        (Some(node), Resolution::ExactName | Resolution::Fuzzy) => &node.qualified_name,
        _ => target,
    };
    let exact_count = i64::from(
        matches!(resolution, Resolution::Exact | Resolution::ExactName) && node.is_some(),
    );
    let mut found = run_pattern(
        store,
        &root,
        pattern,
        target,
        node.as_ref(),
        depth,
        import_kind,
    )?;
    // Rows outside the scope asked for go after the walk, so a chain through
    // a file outside it still reaches the files inside.
    if let Some(prefix) = path_prefix {
        found.rows.retain(|row| {
            let file = if pattern == "imports_of" {
                get(row, "import_target").and_then(Value::as_str)
            } else {
                row_file(row)
            };
            file.is_some_and(|file| file.starts_with(prefix))
        });
    }

    // `query_zero_result_fields`.
    let raw_count = found.rows.len();
    // A node the graph has, that no edge of the kind asked for reaches.
    let mut empty_note = node
        .as_ref()
        .filter(|_| raw_count == 0 && found.unresolved.is_empty() && exact_count > 0)
        .and_then(|node| no_edges_note(pattern, &node.language, import_kind));
    // A node no test calls can still run under the tests that call its
    // callers: say so, and name the walk that lists them, instead of "no
    // test reaches it" (eval/g8_go/RESULTS.md, defect 4).
    let mut tests_through_callers = false;
    if pattern == "tests_for" && empty_note.is_some() {
        let callers = run_pattern(store, &root, "callers_of", target, node.as_ref(), 1, None)?;
        if !callers.rows.is_empty() {
            tests_through_callers = true;
            let nearest = found
                .test_reach
                .as_ref()
                .and_then(|reach| Some((reach["nearest_test"].as_str()?, reach["hops"].as_u64()?)));
            empty_note = Some(tests_through_callers_note(nearest, callers.rows.len()));
        }
    }
    let (confidence, zero_result_reason) = if raw_count > 0 {
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
    } else if empty_note.is_some() {
        ("low", json!(NO_MATCHING_EDGES))
    } else {
        ("low", json!("not_found_in_current_graph"))
    };
    let guidance = guidance(pattern, target, raw_count, exact_count);
    let summary = found_summary(
        raw_count,
        pattern,
        target,
        depth,
        path_prefix,
        empty_note.as_deref(),
    );

    let mut payload = Ordered::default()
        .put("status", "ok")
        .put("pattern", pattern)
        .put("target", target)
        .put("unresolved_count", found.unresolved.len())
        .put("unresolved_targets", json!(found.unresolved))
        .put("confidence", confidence)
        .put("zero_result_reason", zero_result_reason)
        .put("next", json!([]))
        .put("resolution", resolution.as_str())
        .put("exact_match_count", exact_count);
    if matches!(resolution, Resolution::ExactName | Resolution::Fuzzy) {
        payload = payload.put("resolved_target", target);
        payload = payload.put("original_target", original_target);
    }
    if let Some(reachability) = &found.reachability {
        payload = payload
            .put("depth", depth)
            .put("reachability", reachability.clone());
    }
    let budget;
    if full {
        let edges: Vec<Value> = found.edges.iter().cloned().map(Value::Object).collect();
        payload = payload
            .put("description", description(pattern).unwrap_or(""))
            .put("summary", summary)
            .put("result_count", raw_count)
            .put("answerability", answerability.full())
            .put(
                "results",
                json!(
                    found
                        .rows
                        .iter()
                        .skip(offset)
                        .cloned()
                        .map(object)
                        .collect::<Vec<_>>()
                ),
            )
            .put("edges", json!(edges))
            .put("guidance", guidance);
        budget = 8000;
    } else {
        let mut rows: Vec<Row> = found.rows.iter().map(|row| compact(row, minimal)).collect();
        if MERGED.contains(&pattern) {
            rows = merge(rows);
        }
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|row| {
                if minimal {
                    project_minimal(row)
                } else {
                    object(row)
                }
            })
            .collect();
        let summary = summary.replacen(
            &format!("Found {raw_count} "),
            &format!("Found {} ", rows.len()),
            1,
        );
        let rows_len = rows.len();
        let rows: Vec<Value> = rows.into_iter().skip(offset).collect();
        payload = payload
            .put("summary", summary)
            .put("result_count", rows_len)
            .put("answerability", answerability.compact())
            .put("results", json!(rows));
        if !minimal {
            // One count per reply: the rows, after `merge` folded the edges
            // of one node into one row.
            payload = payload.put(
                "guidance",
                self::guidance(pattern, target, rows_len, exact_count),
            );
        }
        budget = if minimal {
            crate::MINIMAL_BUDGET
        } else {
            crate::STANDARD_BUDGET
        };
    }
    payload = payload.apply_output_budget(budget, &["results", "edges"]);
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
    let page = cut.or_else(|| (offset > 0).then_some((shown_rows, shown_rows)));
    let mut page_kept = 0;
    if let Some((kept, rest)) = page
        && let Some(summary) = payload
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_string)
    {
        // The files of every row, cut or not: a question about which files
        // gets its whole answer even when the rows do not fit.
        let mut files: Vec<&str> = found.rows.iter().filter_map(row_file).collect();
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
        page_kept = kept;
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
        let summary = format!("{summary}. {}", page_note(&page, minimal));
        payload = payload.replace("summary", json!(summary));
    }
    // The walk's own row limit bounds what any page can show.
    if found
        .reachability
        .as_ref()
        .is_some_and(|reach| reach.get("truncated") == Some(&Value::Bool(true)))
        && let Some(summary) = payload.get("summary").and_then(Value::as_str)
    {
        let summary = format!("{summary}. {}", walk_cut_note());
        payload = payload.replace("summary", json!(summary));
    }
    // Say that a closed answer is the whole answer: agents re-checked lists
    // the graph had already completed
    // (docs/plans/AGENT-WORKFLOW-TARGET.md#order-of-work, step 10).
    let closed = results_complete
        && raw_count > 0
        && found.unresolved.is_empty()
        && found.reachability.as_ref().is_none_or(|reach| {
            reach.get("state") == Some(&json!("complete"))
                && reach.get("depth_limit_reached") != Some(&Value::Bool(true))
        });
    let mut unbound_caller = None;
    if let Some((note, read_first)) =
        ending(store, pattern, node.as_ref(), &found, closed, import_kind)
        && let Some(summary) = payload.get("summary").and_then(Value::as_str)
    {
        let summary = format!("{summary}. {note}");
        payload = payload.replace("summary", json!(summary));
        unbound_caller = read_first;
    }
    let rows = payload
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut next = crate::next::after_query(pattern, target, &rows);
    // This call's arguments, for a call that goes on from it.
    let this_call = |pattern: &str, depth: Option<i64>| {
        let mut args = json!({"pattern": pattern, "target": target});
        if let Some(depth) = depth {
            args["depth"] = json!(depth);
        }
        if let Some(kind) = import_kind {
            args["import_kind"] = json!(kind);
        }
        if let Some(prefix) = path_prefix {
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
    if cut.is_some() && minimal {
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
    } else if cut.is_some() && page_kept > 0 {
        let mut args = this_call(pattern, walked);
        if detail_level != "standard" {
            args["detail_level"] = json!(detail_level);
        }
        let after = offset as u64 + page_kept;
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
    payload = payload.replace("next", next);
    if let Some(reach) = found.test_reach.clone() {
        payload = payload.put("test_reach", reach);
    }
    let mut missingness = answerability.missingness();
    // `_attach_source_of_coverage`.
    if let Some(coverage) = found.source
        && raw_count > 0
    {
        missingness.extend(coverage.missingness());
        if coverage.read_error.is_some() || coverage.stale {
            payload = payload.replace("status", json!("degraded"));
        }
        payload = payload.put("source_coverage", coverage.value());
    }
    Some(
        payload
            .put("missingness", json!(missingness))
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}
