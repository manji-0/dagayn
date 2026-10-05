//! `flow_tool` (`dagayn.tools.flow_dispatcher.flow_func`): `list_flows` and
//! `get_flow` from `dagayn.tools.flows_tools`.

use std::collections::HashSet;
use std::path::Path;

use dagayn_graph::GraphStore;
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::coverage::splitlines;
use crate::review::guidance_actions_to_hints;
use crate::review_summary::guidance_item;
use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo, seal_dispatch};

const DECLARED: &[&str] = &[
    "mode",
    "sort_by",
    "limit",
    "kind",
    "detail_level",
    "flow_id",
    "flow_name",
    "include_source",
    "repo_root",
];
const SORT_KEYS: &[&str] = &["criticality", "depth", "node_count", "file_count", "name"];
/// `get_flow`'s per-step source cap, in characters, and its budget.
const SOURCE_MAX_CHARS: usize = 2000;
const FLOW_BUDGET: usize = 8000;

/// Python truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(n) => n.as_f64().is_some_and(|v| v != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `items[:limit]`.
fn py_prefix<T>(mut items: Vec<T>, limit: i64) -> Vec<T> {
    let len = items.len() as i64;
    let end = if limit < 0 {
        (len + limit).max(0)
    } else {
        limit.min(len)
    };
    items.truncate(end as usize);
    items
}

pub(crate) fn flow(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    let text = |key: &str, default: &'static str, allowed: &[&str]| -> Option<String> {
        match arguments.get(key) {
            None => Some(default.to_string()),
            Some(Value::String(value)) if allowed.contains(&value.as_str()) => Some(value.clone()),
            Some(_) => None,
        }
    };
    let mode = text("mode", "list", &["list", "get"])?;
    let sort_by = text("sort_by", "criticality", SORT_KEYS)?;
    let detail_level = text("detail_level", "standard", &["minimal", "standard"])?;
    let limit = args.integer("limit", 50)?;
    let kind = args.optional_string("kind")?;
    let flow_id = match arguments.get("flow_id") {
        None | Some(Value::Null) => None,
        Some(value) if value.is_i64() => value.as_i64(),
        Some(_) => return None,
    };
    let flow_name = args.optional_string("flow_name")?;
    let include_source = match arguments.get("include_source") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return None,
    };
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    if mode == "get" && flow_id.is_none() && flow_name.is_none_or(str::is_empty) {
        // `FlowGetRequest.require_selector`.
        return crate::dispatcher_error(
            context,
            &root,
            &mode,
            "Value error, mode=\"get\" requires flow_id or flow_name.",
        );
    }
    let runtime = context.runtime.clone()?;
    let graph = open_graph(&root)?;
    let stats = graph.store.get_stats().ok()?;
    let answerability = Answerability::recorded(&graph.store, &stats)?;
    let exposed = |tool: &str| {
        context
            .allowed_tools
            .as_ref()
            .is_none_or(|allowed| allowed.contains(tool))
    };
    let (subtool, out) = if mode == "list" {
        let out = list_flows(
            &graph.store,
            &answerability,
            &sort_by,
            limit,
            kind,
            &detail_level,
        )?;
        ("list_flows", out)
    } else {
        let out = get_flow(
            &graph.store,
            &root,
            &answerability,
            flow_id,
            flow_name,
            include_source,
        )?;
        ("get_flow", out)
    };
    Some(seal_dispatch(
        out,
        crate::Dispatch {
            mode: &mode,
            subtool,
            hints_tool: "flow",
            runtime,
            trailing: Vec::new(),
            repo: graph.repo_context(),
        },
        &exposed,
    ))
}

/// `get_flows`: the stored rows with `_annotate_flow_rows_liveness`.
fn get_flows(store: &GraphStore, sort_by: &str, limit: i64) -> Option<Vec<Value>> {
    let mut flows: Vec<Value> =
        serde_json::from_str(&store.get_flows_json(sort_by, limit).ok()?).ok()?;
    let ids_of = |flow: &Value| -> Vec<i64> {
        flow.get("path")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .collect()
    };
    let mut all: HashSet<i64> = HashSet::new();
    for flow in &flows {
        all.extend(ids_of(flow));
        if let Some(entry) = flow.get("entry_point_id").and_then(Value::as_i64) {
            all.insert(entry);
        }
    }
    if all.is_empty() {
        return Some(flows);
    }
    let mut sorted: Vec<i64> = all.into_iter().collect();
    sorted.sort_unstable();
    let Ok(nodes) = store.get_nodes_by_ids(&sorted) else {
        return Some(flows);
    };
    for flow in &mut flows {
        let path = ids_of(flow);
        let Some(object) = flow.as_object_mut() else {
            continue;
        };
        if !object.contains_key("entry_point")
            && let Some(node) = object
                .get("entry_point_id")
                .and_then(Value::as_i64)
                .and_then(|id| nodes.get(&id))
        {
            object.insert("entry_point".into(), json!(node.qualified_name));
        }
        let resolved = path.iter().filter(|id| nodes.contains_key(id)).count();
        object.insert("resolved_node_count".into(), json!(resolved));
        object.insert("missing_node_count".into(), json!(path.len() - resolved));
        if !object.contains_key("files") {
            let mut files: Vec<String> = Vec::new();
            for id in &path {
                if let Some(node) = nodes.get(id)
                    && !node.file_path.is_empty()
                    && !files.contains(&node.file_path)
                {
                    files.push(node.file_path.clone());
                }
            }
            object.insert("files".into(), json!(files));
        }
    }
    Some(flows)
}

/// `list_flows`.
fn list_flows(
    store: &GraphStore,
    answerability: &Answerability,
    sort_by: &str,
    limit: i64,
    kind: Option<&str>,
    detail_level: &str,
) -> Option<Ordered> {
    let filter = kind.filter(|k| !k.is_empty());
    let fetch = if filter.is_some() { limit * 10 } else { limit };
    let mut flows = get_flows(store, sort_by, fetch)?;
    if let Some(kind) = filter {
        let entries: Vec<i64> = flows
            .iter()
            .filter_map(|f| f.get("entry_point_id").and_then(Value::as_i64))
            .collect();
        let nodes = store.get_nodes_by_ids(&entries).ok()?;
        let filtered: Vec<Value> = flows
            .into_iter()
            .filter(|f| {
                f.get("entry_point_id")
                    .and_then(Value::as_i64)
                    .and_then(|id| nodes.get(&id))
                    .is_some_and(|node| node.kind == kind)
            })
            .collect();
        flows = py_prefix(filtered, limit);
    }
    if detail_level == "minimal" {
        flows = flows
            .into_iter()
            .map(|f| {
                let kind = match f.get("kind") {
                    Some(value) if truthy(value) => value.clone(),
                    _ => json!("reachable_set"),
                };
                json!({
                    "name": f["name"],
                    "criticality": f["criticality"],
                    "node_count": f["node_count"],
                    "kind": kind,
                    "truncated": f.get("truncated").is_some_and(truthy),
                })
            })
            .collect();
    }
    let truncated = flows
        .iter()
        .filter(|f| f.get("truncated").is_some_and(truthy))
        .count();
    let count = flows.len();
    let mut summary = format!("Found {count} reachable-set flow(s)");
    if truncated > 0 {
        summary.push_str(&format!(" ({truncated} truncated)"));
    }
    let ranking = json!({
        "reason_code": "flow_criticality_is_ranking_signal",
        "severity": "low",
        "claim_effect": "flow ranking is not a coverage guarantee",
    });
    let mut missingness = answerability.missingness();
    missingness.push(ranking.clone());
    if truncated > 0 {
        missingness.push(json!({
            "reason_code": "truncated_flow",
            "severity": "medium",
            "claim_effect": "one or more reachable sets were capped; omitted callees are not absent from the program",
        }));
    }
    let guidance = vec![guidance_item(
        if count > 0 {
            format!("Returned {count} ranked reachable-set flow(s) from stored flow extraction.")
        } else {
            "No flows matched the current filters.".to_string()
        },
        json!({
            "type": "computed",
            "returned_flow_count": count,
            "limit": limit,
            "sort_by": sort_by,
            "kind_filter": kind,
        }),
        if count > 0 { "medium" } else { "low" },
        vec![ranking],
        if count > 0 {
            "flow_tool mode=\"get\" -- inspect a specific reachable set"
        } else {
            "flow_tool mode=\"list\" -- broaden kind filter or increase limit"
        },
        vec![json!("stored_flow_extraction")],
        json!({"returned_flow_count": count}),
    )];
    let hints = guidance_actions_to_hints(&guidance);
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("flows", Value::Array(flows))
            .put(
                "flow_coverage",
                json!({
                    "source": "stored_flow_extraction",
                    "returned_flow_count": count,
                    "limit": limit,
                    "kind_filter": kind,
                    "truncated_count": truncated,
                    "coverage_guarantee": false,
                }),
            )
            .put("answerability", answerability.full())
            .put("missingness", json!(missingness))
            .put("guidance", Value::Array(guidance))
            .put("_hints", hints),
    )
}

/// `get_flow_by_id`: the stored flow with its steps re-read from live nodes.
fn get_flow_by_id(store: &GraphStore, flow_id: i64) -> Option<Option<Value>> {
    let Some(raw) = store.get_flow_by_id_json(flow_id).ok()? else {
        return Some(None);
    };
    let mut flow: Value = serde_json::from_str(&raw).ok()?;
    let object = flow.as_object_mut()?;
    object.remove("steps");
    let path: Vec<i64> = object
        .get("path")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .collect();
    let nodes = if path.is_empty() {
        Default::default()
    } else {
        store.get_nodes_by_ids(&path).unwrap_or_default()
    };
    let steps: Vec<Value> = path
        .iter()
        .filter_map(|id| nodes.get(id))
        .map(|node| {
            json!({
                "id": node.id,
                "node_id": node.id,
                "qualified_name": node.qualified_name,
                "name": node.name,
                "file": node.file_path,
                "file_path": node.file_path,
                "kind": node.kind,
                "line_start": node.line_start,
                "line_end": node.line_end,
            })
        })
        .collect();
    object.insert("steps".into(), Value::Array(steps));
    store.annotate_flow_bridges(&mut flow).ok()?;
    Some(Some(flow))
}

fn as_int(value: Option<&Value>) -> i64 {
    match value {
        Some(value) if truthy(value) => value
            .as_i64()
            .or_else(|| value.as_f64().map(|v| v as i64))
            .unwrap_or(0),
        _ => 0,
    }
}

/// `"%.4f" % value` for the criticality.
fn four(value: &Value) -> String {
    format!("{:.4}", value.as_f64().unwrap_or(0.0))
}

/// The step's source lines, as `get_flow` reads them.
fn step_source(step: &Value, root: &Path) -> Option<String> {
    let raw = match step.get("file") {
        Some(value) if truthy(value) => value,
        _ => step.get("file_path").filter(|v| truthy(v))?,
    };
    let raw = raw.as_str()?;
    let path = if Path::new(raw).is_absolute() {
        Path::new(raw).to_path_buf()
    } else {
        root.join(raw)
    };
    if !path.is_file() {
        return None;
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return Some("(could not read file)".to_string());
    };
    let decoded = String::from_utf8_lossy(&bytes);
    let lines = splitlines(&decoded);
    let first = match step.get("line_start") {
        Some(value) if truthy(value) => value.as_i64().unwrap_or(1),
        _ => 1,
    };
    let start = (first - 1).max(0) as usize;
    let last = match step.get("line_end") {
        Some(value) if truthy(value) => value.as_i64().unwrap_or(lines.len() as i64),
        _ => lines.len() as i64,
    };
    let end = (last.max(0) as usize).min(lines.len());
    let mut source = if start < end {
        lines[start..end]
            .iter()
            .enumerate()
            .map(|(offset, line)| format!("{}: {line}", start + offset + 1))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    if source.chars().count() > SOURCE_MAX_CHARS {
        source = source.chars().take(SOURCE_MAX_CHARS).collect::<String>() + "\n... (truncated)";
    }
    Some(source)
}

/// `get_flow`.
fn get_flow(
    store: &GraphStore,
    root: &Path,
    answerability: &Answerability,
    flow_id: Option<i64>,
    flow_name: Option<&str>,
    include_source: bool,
) -> Option<Ordered> {
    let mut flow = match (flow_id, flow_name) {
        (Some(id), _) => get_flow_by_id(store, id)?,
        (None, Some(name)) => {
            let wanted = name.to_lowercase();
            let mut found = None;
            for candidate in get_flows(store, "criticality", 500)? {
                let title = candidate["name"].as_str().unwrap_or("").to_lowercase();
                if title.contains(&wanted) {
                    let id = candidate["id"].as_i64()?;
                    found = get_flow_by_id(store, id)?;
                    break;
                }
            }
            found
        }
        (None, None) => None,
    };
    let Some(flow) = flow.as_mut() else {
        let mut missingness = answerability.missingness();
        missingness.push(json!({
            "reason_code": "flow_not_found_in_current_graph",
            "severity": "medium",
            "claim_effect": "absence is graph-limited, not proof the flow cannot exist",
        }));
        return Some(
            Ordered::default()
                .put("status", "not_found")
                .put("summary", "No flow found matching the given criteria.")
                .put("answerability", answerability.full())
                .put("missingness", json!(missingness)),
        );
    };
    let steps_len = flow
        .get("steps")
        .and_then(Value::as_array)
        .map_or(0, Vec::len) as i64;
    let resolved = match as_int(flow.get("resolved_step_count")) {
        0 => steps_len,
        n => n,
    };
    let missing = as_int(flow.get("missing_step_count"));
    let stored = match as_int(flow.get("node_count")) {
        0 => resolved,
        n => n,
    };
    let stale = missing > 0;
    let truncated = flow.get("truncated").is_some_and(truthy);
    let reason = flow
        .get("truncation_reason")
        .cloned()
        .unwrap_or(Value::Null);
    let reason_text = reason.as_str().filter(|r| !r.is_empty());
    let bridge_steps = as_int(flow.get("bridge_step_count"));

    if include_source && let Some(steps) = flow.get_mut("steps").and_then(Value::as_array_mut) {
        for step in steps.iter_mut() {
            if let Some(source) = step_source(step, root)
                && let Some(object) = step.as_object_mut()
            {
                object.insert("source".into(), json!(source));
            }
        }
    }

    let name = flow["name"].as_str().unwrap_or("").to_string();
    let depth = match &flow["depth"] {
        Value::Null => "None".to_string(),
        other => other.to_string(),
    };
    let criticality = four(&flow["criticality"]);
    let mut summary = if stale {
        format!(
            "Flow '{name}': {resolved}/{stored} members resolved ({missing} missing), depth {depth}, criticality {criticality}"
        )
    } else {
        format!(
            "Flow '{name}' (reachable_set): {stored} members, depth {depth}, criticality {criticality}"
        )
    };
    if truncated {
        summary.push_str(&format!(
            " [truncated:{}]",
            reason_text.unwrap_or("unspecified")
        ));
    }
    let stale_missing: Vec<Value> = if stale {
        vec![json!({
            "reason_code": "stale_flow",
            "severity": "medium",
            "claim_effect": format!("{missing} stored member(s) no longer resolve to live graph nodes"),
        })]
    } else {
        Vec::new()
    };
    let truncated_missing: Vec<Value> = if truncated {
        let detail = reason_text.map(|r| format!(" ({r})")).unwrap_or_default();
        vec![json!({
            "reason_code": "truncated_flow",
            "severity": "medium",
            "claim_effect": format!("reachable set was capped{detail}; omitted callees are not absent from the program"),
        })]
    } else {
        Vec::new()
    };
    let mut missingness = answerability.missingness();
    missingness.extend(stale_missing.iter().cloned());
    missingness.extend(truncated_missing.iter().cloned());
    missingness.push(json!({
        "reason_code": "source_inclusion_explicit",
        "severity": "low",
        "claim_effect": format!("source snippets included: {}", if include_source { "True" } else { "False" }),
    }));

    let claim = if stale {
        format!(
            "Flow '{name}' resolves {resolved} of {stored} stored step(s); {missing} step(s) are missing from the live graph."
        )
    } else if bridge_steps != 0 {
        format!(
            "Flow '{name}' has {stored} step(s) with criticality {criticality}, including {} bridge step(s).",
            flow.get("bridge_step_count").cloned().unwrap_or(json!(0))
        )
    } else {
        format!("Flow '{name}' has {stored} step(s) with criticality {criticality}.")
    };
    let mut guidance_missing = stale_missing.clone();
    guidance_missing.extend(truncated_missing.iter().cloned());
    guidance_missing.push(json!({
        "reason_code": "flow_path_is_stored_extraction",
        "severity": "low",
        "claim_effect": "flow members are a BFS reachable set, not a runtime call sequence",
    }));
    if bridge_steps != 0 {
        guidance_missing.push(json!({
            "reason_code": "cross_artifact_bridge_is_static_evidence",
            "severity": "low",
            "claim_effect": "bridge steps mark CROSS_ARTIFACT transitions distinctly",
        }));
    }
    let mut reasons = vec![json!("stored_flow_extraction"), json!("reachable_set")];
    if stale {
        reasons.push(json!("stale_flow"));
    }
    if truncated {
        reasons.push(json!("truncated_flow"));
    }
    if bridge_steps != 0 {
        reasons.push(json!("cross_artifact_bridge_step"));
    }
    let field = |key: &str| flow.get(key).cloned().unwrap_or(Value::Null);
    let guidance = vec![guidance_item(
        claim,
        json!({
            "type": "computed",
            "flow_id": field("id"),
            "name": field("name"),
            "node_count": stored,
            "resolved_step_count": resolved,
            "missing_step_count": missing,
            "depth": field("depth"),
            "criticality": field("criticality"),
            "bridge_step_count": flow.get("bridge_step_count").cloned().unwrap_or(json!(0)),
            "source_included": include_source,
        }),
        if stale { "low" } else { "medium" },
        guidance_missing,
        if stale {
            "dagayn build --local-embedding none -- refresh stored flows; review_tool mode=\"impact\" -- check blast radius along resolved steps"
        } else {
            "review_tool mode=\"impact\" -- check blast radius along this flow; query_graph_tool pattern=\"docs_for\" -- follow bridge docs when present"
        },
        reasons,
        json!({}),
    )];
    let mut flow_value = flow.clone();
    if include_source && let Some(object) = flow_value.as_object() {
        let entries = object
            .iter()
            .fold(Ordered::default(), |o, (k, v)| o.put(k, v.clone()));
        flow_value = entries.apply_output_budget(FLOW_BUDGET, &["steps"]).value();
    }
    let hints = guidance_actions_to_hints(&guidance);
    Some(
        Ordered::default()
            .put("status", if stale || truncated { "degraded" } else { "ok" })
            .put("summary", summary)
            .put("flow", flow_value)
            .put(
                "flow_coverage",
                json!({
                    "source_included": include_source,
                    "step_count": resolved,
                    "stored_node_count": stored,
                    "resolved_step_count": resolved,
                    "missing_step_count": missing,
                    "truncated": truncated,
                    "truncation_reason": reason,
                    "coverage_guarantee": false,
                }),
            )
            .put("answerability", answerability.full())
            .put("missingness", json!(missingness))
            .put("guidance", Value::Array(guidance))
            .put("_hints", hints),
    )
}
