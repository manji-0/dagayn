//! `architecture_analysis_tool`
//! (`dagayn.tools.architecture_analysis.architecture_analysis_func`): the
//! ADP, SDP, and SAP modes. The other modes stay Python's.

use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::architecture::{
    Artifact, Profile, ScopeGraph, Snapshot, View, sap_metrics, sap_violations,
};
use crate::{
    Args, Context, Ordered, Payload, explicit_repo, open_graph, seal_dispatch, suggestions,
};

const DECLARED: &[&str] = &[
    "mode",
    "detail_level",
    "top_n",
    "sort_by",
    "min_size",
    "community_name",
    "community_id",
    "include_members",
    "granularity",
    "scope_kind",
    "unit_filter",
    "min_cycle_size",
    "max_cycle_length",
    "min_delta",
    "min_distance",
    "repo_root",
    "artifact_scope",
    "dependency_profile",
];
const MODES: &[&str] = &[
    "overview",
    "communities",
    "community",
    "hubs",
    "bridges",
    "knowledge_gaps",
    "surprising_connections",
    "adp_violations",
    "sdp_metrics",
    "sdp_violations",
    "sap_metrics",
    "sap_violations",
];
/// `detect_sap_violations_func`'s output budget, in tokens.
const SAP_VIOLATIONS_BUDGET: usize = 5000;

/// `repr(float)` where Python spells it without an exponent.
fn py_float(value: f64) -> Option<String> {
    if !value.is_finite() || (value != 0.0 && !(1e-4..1e16).contains(&value.abs())) {
        return None;
    }
    let text = value.to_string();
    Some(if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    })
}

/// `items[:limit]`.
fn py_prefix(items: &[Value], limit: i64) -> Vec<Value> {
    let len = items.len() as i64;
    let end = if limit < 0 {
        (len + limit).max(0)
    } else {
        limit.min(len)
    };
    items[..end as usize].to_vec()
}

/// The arguments once fastmcp and `parse_architecture_analysis_request`
/// accept them.
struct Request<'a> {
    mode: &'a str,
    detail_level: &'a str,
    top_n: i64,
    granularity: &'a str,
    scope_kind: &'a str,
    unit_filter: Option<Vec<String>>,
    min_cycle_size: i64,
    max_cycle_length: i64,
    min_delta: f64,
    min_distance: f64,
    artifact_scope: &'a str,
    dependency_profile: &'a str,
}

impl<'a> Request<'a> {
    fn parse(args: &Args<'a>, arguments: &'a Map<String, Value>) -> Option<Self> {
        let literal = |key: &str, default: &'a str, allowed: &[&str]| -> Option<&'a str> {
            match arguments.get(key) {
                None => Some(default),
                Some(Value::String(value)) if allowed.contains(&value.as_str()) => {
                    Some(value.as_str())
                }
                Some(_) => None,
            }
        };
        let float = |key: &str, default: f64| -> Option<f64> {
            match arguments.get(key) {
                None => Some(default),
                Some(value) if value.is_number() => value.as_f64(),
                Some(_) => None,
            }
        };
        let unit_filter = match arguments.get("unit_filter") {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => Some(
                items
                    .iter()
                    .map(|item| item.as_str().map(str::to_string))
                    .collect::<Option<Vec<_>>>()?,
            ),
            Some(_) => return None,
        };
        // Arguments the analysis modes ignore still pass fastmcp's checks.
        literal("sort_by", "size", &["size", "cohesion", "name"])?;
        args.integer("min_size", 0)?;
        args.optional_string("community_name")?;
        match arguments.get("community_id") {
            None | Some(Value::Null) => {}
            Some(value) if value.is_i64() => {}
            Some(_) => return None,
        }
        if !matches!(
            arguments.get("include_members"),
            None | Some(Value::Bool(_))
        ) {
            return None;
        }
        Some(Self {
            mode: literal("mode", "overview", MODES)?,
            detail_level: literal(
                "detail_level",
                "minimal",
                &["minimal", "standard", "verbose"],
            )?,
            top_n: args.integer("top_n", 10)?,
            granularity: literal("granularity", "package", &["file", "package"])?,
            scope_kind: literal("scope_kind", "package", &["file", "package", "directory"])?,
            unit_filter,
            min_cycle_size: args.integer("min_cycle_size", 2)?,
            max_cycle_length: args.integer("max_cycle_length", 10)?,
            min_delta: float("min_delta", 0.1)?,
            min_distance: float("min_distance", 0.5)?,
            artifact_scope: literal("artifact_scope", "code", &["code", "docs", "all"])?,
            dependency_profile: literal(
                "dependency_profile",
                "strict_static",
                &[
                    "strict_static",
                    "implementation",
                    "infra_dataflow",
                    "artifact_trace",
                ],
            )?,
        })
    }
}

pub(crate) fn architecture(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    let request = Request::parse(&args, arguments)?;
    if !matches!(
        request.mode,
        "adp_violations" | "sdp_metrics" | "sdp_violations" | "sap_metrics" | "sap_violations"
    ) {
        return None;
    }
    let runtime = context.runtime.clone()?;
    let root = explicit_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let snapshot = Snapshot::read(&graph.store)?;
    let artifact = Artifact::parse(request.artifact_scope)?;
    let profile = Profile::parse(request.dependency_profile)?;
    let (subtool, out) = match request.mode {
        "adp_violations" => (
            "detect_adp_violations_func",
            adp(context, &request, &snapshot, artifact, profile)?,
        ),
        "sdp_metrics" => (
            "compute_sdp_metrics_func",
            sdp_metrics(context, &request, &snapshot, artifact, profile),
        ),
        "sdp_violations" => (
            "detect_sdp_violations_func",
            sdp_violations(context, &request, &snapshot, artifact, profile)?,
        ),
        "sap_metrics" => (
            "compute_sap_metrics_func",
            sap(context, &request, &snapshot, artifact, profile),
        ),
        _ => (
            "detect_sap_violations_func",
            sap_violation_list(context, &request, &snapshot, artifact, profile)?,
        ),
    };
    // `attach_answerability` for a subtool that reports none.
    let stats = graph.store.get_stats().ok()?;
    let answerability = Answerability::recorded(&graph.store, &stats)?;
    let exposed = |tool: &str| {
        context
            .allowed_tools
            .as_ref()
            .is_none_or(|allowed| allowed.contains(tool))
    };
    Some(seal_dispatch(
        out,
        crate::Dispatch {
            mode: request.mode,
            subtool,
            hints_tool: "architecture_analysis",
            runtime,
            trailing: vec![
                ("answerability", answerability.full()),
                ("missingness", json!(answerability.missingness())),
            ],
            repo: graph.repo_context(),
        },
        &exposed,
    ))
}

/// `make_response("ok", summary, **fields, next_tool_suggestions=...)`.
fn make_response(
    context: &Context,
    summary: String,
    fields: Vec<(&str, Value)>,
    next: &[&str],
) -> Ordered {
    let (hints, kept) = suggestions(context, next);
    let mut out = Ordered::default()
        .put("status", "ok")
        .put("summary", summary);
    for (key, value) in fields {
        out = out.put(key, value);
    }
    out.put("_hints", hints).put("next_tool_suggestions", kept)
}

fn graph_for(
    snapshot: &Snapshot,
    request: &Request,
    artifact: Artifact,
    profile: Profile,
) -> ScopeGraph {
    let view = View {
        file_scopes: request.granularity == "file",
        artifact,
        profile,
    };
    ScopeGraph::new(&snapshot.dependencies(&view))
}

/// `detect_adp_violations_func`.
fn adp(
    context: &Context,
    request: &Request,
    snapshot: &Snapshot,
    artifact: Artifact,
    profile: Profile,
) -> Option<Ordered> {
    let graph = graph_for(snapshot, request, artifact, profile);
    let violations = if graph.is_empty() {
        Vec::new()
    } else {
        graph.adp_violations(request.min_cycle_size, request.max_cycle_length, profile)?
    };
    let total = violations.len();
    let top_n = request.top_n;
    let truncated = total as i64 > top_n;
    let all = format!(
        "architecture_analysis_tool mode=\"adp_violations\" top_n={total} -- list every cycle"
    );
    let mut next = vec![
        "review_tool mode=\"impact\" -- check blast radius of a cyclic module",
        "query_graph_tool imports_of -- trace what a module imports",
        "architecture_analysis_tool mode=\"sdp_violations\" -- check stability direction",
    ];
    if truncated {
        next.insert(0, &all);
    }
    let mut summary = format!(
        "Found {total} ADP violation(s) at {} level (artifact_scope={}, dependency_profile={}).",
        request.granularity,
        request.artifact_scope,
        profile.name()
    );
    if truncated {
        summary.push_str(&format!(" Showing top {top_n} by severity."));
    }
    Some(make_response(
        context,
        summary,
        vec![
            ("violations", Value::Array(py_prefix(&violations, top_n))),
            ("count", json!(total)),
            ("truncated", json!(truncated)),
            ("granularity", json!(request.granularity)),
            ("artifact_scope", json!(request.artifact_scope)),
            ("dependency_profile", json!(profile.name())),
        ],
        &next,
    ))
}

/// `compute_sdp_metrics_func`.
fn sdp_metrics(
    context: &Context,
    request: &Request,
    snapshot: &Snapshot,
    artifact: Artifact,
    profile: Profile,
) -> Ordered {
    let graph = graph_for(snapshot, request, artifact, profile);
    let metrics: Vec<Value> = graph
        .sdp_metrics()
        .into_iter()
        .map(|(name, ca, ce, instability)| {
            json!({"name": name, "ca": ca, "ce": ce, "instability": instability, "dependency_profile": profile.name()})
        })
        .collect();
    let shown = request.top_n.min(metrics.len() as i64);
    make_response(
        context,
        format!(
            "Computed SDP instability for {} {}(s) (artifact_scope={}, dependency_profile={}). Showing top {shown} most unstable.",
            metrics.len(),
            request.granularity,
            request.artifact_scope,
            profile.name()
        ),
        vec![
            ("metrics", Value::Array(py_prefix(&metrics, request.top_n))),
            ("total", json!(metrics.len())),
            ("granularity", json!(request.granularity)),
            ("artifact_scope", json!(request.artifact_scope)),
            ("dependency_profile", json!(profile.name())),
        ],
        &[
            "architecture_analysis_tool mode=\"sdp_violations\" -- find stability violations",
            "architecture_analysis_tool mode=\"adp_violations\" -- find cyclic dependencies",
            "architecture_analysis_tool mode=\"hubs\" -- find most connected nodes",
        ],
    )
}

/// `detect_sdp_violations_func`.
fn sdp_violations(
    context: &Context,
    request: &Request,
    snapshot: &Snapshot,
    artifact: Artifact,
    profile: Profile,
) -> Option<Ordered> {
    let graph = graph_for(snapshot, request, artifact, profile);
    let violations = graph.sdp_violations(request.min_delta, profile);
    let total = violations.len();
    let truncated = total as i64 > request.top_n;
    let mut summary = format!(
        "Found {total} SDP violation(s) at {} level (artifact_scope={}, dependency_profile={}, min_delta={}).",
        request.granularity,
        request.artifact_scope,
        profile.name(),
        py_float(request.min_delta)?
    );
    if truncated {
        summary.push_str(&format!(
            " Showing top {} by instability gap.",
            request.top_n
        ));
    }
    Some(make_response(
        context,
        summary,
        vec![
            (
                "violations",
                Value::Array(py_prefix(&violations, request.top_n)),
            ),
            ("count", json!(total)),
            ("total", json!(total)),
            ("truncated", json!(truncated)),
            ("granularity", json!(request.granularity)),
            ("artifact_scope", json!(request.artifact_scope)),
            ("dependency_profile", json!(profile.name())),
        ],
        &[
            "architecture_analysis_tool mode=\"sdp_metrics\" -- see instability scores",
            "architecture_analysis_tool mode=\"adp_violations\" -- check cyclic dependencies",
            "review_tool mode=\"impact\" -- check blast radius of a violating module",
        ],
    ))
}

fn sap_view(request: &Request, artifact: Artifact, profile: Profile) -> View {
    View {
        file_scopes: request.scope_kind == "file",
        artifact,
        profile,
    }
}

/// `compute_sap_metrics_func`.
fn sap(
    context: &Context,
    request: &Request,
    snapshot: &Snapshot,
    artifact: Artifact,
    profile: Profile,
) -> Ordered {
    let view = sap_view(request, artifact, profile);
    let raw = sap_metrics(
        snapshot,
        &view,
        request.scope_kind,
        request.unit_filter.as_deref(),
    );
    let applicable_flag = |m: &Value| {
        m.get("sap_applicable")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    };
    let applicable: Vec<Value> = raw.iter().filter(|m| applicable_flag(m)).cloned().collect();
    let inapplicable: Vec<Value> = raw
        .iter()
        .filter(|m| !applicable_flag(m))
        .cloned()
        .collect();
    let verbose = request.detail_level == "verbose";
    let visible = if verbose { &raw } else { &applicable };
    let mut by_reason = Map::new();
    for metric in &inapplicable {
        let reason = metric["applicability_reason"]
            .as_str()
            .filter(|r| !r.is_empty())
            .unwrap_or("inapplicable");
        let count = by_reason.get(reason).and_then(Value::as_i64).unwrap_or(0) + 1;
        by_reason.insert(reason.to_string(), json!(count));
    }
    let top_n = request.top_n;
    let truncated = visible.len() as i64 > top_n || inapplicable.len() as i64 > top_n;
    let mut summary = format!(
        "Computed SAP metrics for {} {}(s) (artifact_scope={}, dependency_profile={}); {} applicable and {} inapplicable. Showing top {} by distance.",
        raw.len(),
        request.scope_kind,
        request.artifact_scope,
        profile.name(),
        applicable.len(),
        inapplicable.len(),
        top_n.min(visible.len() as i64)
    );
    if truncated {
        summary.push_str(" Results truncated.");
    }
    make_response(
        context,
        summary,
        vec![
            ("metrics", Value::Array(py_prefix(visible, top_n))),
            (
                "inapplicable_metrics",
                Value::Array(py_prefix(&inapplicable, top_n)),
            ),
            ("total", json!(raw.len())),
            ("visible_total", json!(visible.len())),
            ("applicable_count", json!(applicable.len())),
            ("inapplicable_count", json!(inapplicable.len())),
            ("truncated", json!(truncated)),
            ("inapplicable_by_reason", Value::Object(by_reason)),
            (
                "inapplicable_visibility",
                json!(if verbose {
                    "included_in_metrics"
                } else {
                    "separate_bucket"
                }),
            ),
            ("scope_kind", json!(request.scope_kind)),
            ("artifact_scope", json!(request.artifact_scope)),
            ("dependency_profile", json!(profile.name())),
            ("detail_level", json!(request.detail_level)),
        ],
        &[
            "architecture_analysis_tool mode=\"sap_violations\" -- find far-from-sequence scopes",
            "architecture_analysis_tool mode=\"sdp_metrics\" -- check raw instability",
            "architecture_analysis_tool mode=\"community\" -- explore the scope as a community",
        ],
    )
}

/// `_classify_sap_zone`.
fn zone(violation: &Value) -> &'static str {
    let number = |key: &str| violation[key].as_f64().unwrap_or(0.0);
    let (abstractness, instability) = (number("abstractness"), number("instability"));
    if abstractness <= 0.5 && instability <= 0.5 {
        "pain"
    } else if abstractness >= 0.5 && instability >= 0.5 {
        "uselessness"
    } else {
        "off-main-sequence"
    }
}

/// `detect_sap_violations_func`.
fn sap_violation_list(
    context: &Context,
    request: &Request,
    snapshot: &Snapshot,
    artifact: Artifact,
    profile: Profile,
) -> Option<Ordered> {
    let view = sap_view(request, artifact, profile);
    let raw = sap_metrics(snapshot, &view, request.scope_kind, None);
    let violations: Vec<Value> = sap_violations(&raw, request.min_distance)
        .iter()
        .map(|v| {
            json!({
                "scope_key": v["scope_key"],
                "display_name": v["display_name"],
                "distance": v["distance"],
                "zone": zone(v),
            })
        })
        .collect();
    let total = violations.len();
    let truncated = total as i64 > request.top_n;
    let min_distance = py_float(request.min_distance)?;
    let mut summary = format!(
        "Found {total} SAP violation(s) at {} level (artifact_scope={}, dependency_profile={}, min_distance={min_distance}). sap_violations suppresses test and fixture scopes; inspect sap_metrics notes for raw values.",
        request.scope_kind,
        request.artifact_scope,
        profile.name()
    );
    if truncated {
        summary.push_str(&format!(" Showing top {} by distance.", request.top_n));
    }
    let out = make_response(
        context,
        summary,
        vec![
            (
                "violations",
                Value::Array(py_prefix(&violations, request.top_n)),
            ),
            ("count", json!(total)),
            ("total", json!(total)),
            ("truncated", json!(truncated)),
            ("scope_kind", json!(request.scope_kind)),
            ("artifact_scope", json!(request.artifact_scope)),
            ("dependency_profile", json!(profile.name())),
            ("min_distance", json!(request.min_distance)),
            (
                "excluded_scope_categories",
                json!(["test-scope", "fixture-scope"]),
            ),
            (
                "exclusion_reason",
                json!(
                    "test and fixture scopes are retained in sap_metrics notes but omitted from sap_violations"
                ),
            ),
        ],
        &[
            "architecture_analysis_tool mode=\"sap_metrics\" -- see full A/I/D scores",
            "architecture_analysis_tool mode=\"adp_violations\" -- check cyclic dependencies",
            "review_tool mode=\"impact\" -- check blast radius of a violating scope",
        ],
    );
    Some(out.apply_output_budget(SAP_VIOLATIONS_BUDGET, &["violations"]))
}
