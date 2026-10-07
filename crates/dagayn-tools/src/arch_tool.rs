//! `architecture_analysis_tool`
//! (`dagayn.tools.architecture_analysis.architecture_analysis_func`), in
//! every mode; the Python function opens the graph and returns this answer.

use serde_json::{Map, Value, json};

use crate::analysis::{
    Graph, find_bridges, find_hubs, find_knowledge_gaps, find_surprising_connections, py_prefix,
};
use crate::answerability::Answerability;
use crate::architecture::{
    Artifact, Profile, ScopeGraph, Snapshot, View, sap_metrics, sap_violations,
};
use crate::review::guidance_actions_to_hints;
use crate::review_summary::guidance_item;
use crate::{
    Args, Context, Ordered, Payload, open_graph, resolve_repo, seal_dispatch, suggestions,
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

/// Python's `repr(float)` (`float_repr_style='short'`): the shortest
/// round-trip digits, in exponent form (`1e-05`, `1.5e+16`) when the decimal
/// exponent is below -4 or at least 16, otherwise positional with at least
/// one fractional digit.
fn py_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let sci = format!("{value:e}");
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if value != 0.0 && !(-4..16).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.unsigned_abs());
    }
    let text = value.to_string();
    if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    }
}

/// The arguments once fastmcp has validated them against the Python
/// signature.
struct Request<'a> {
    mode: &'a str,
    sort_by: &'a str,
    min_size: i64,
    community_name: Option<&'a str>,
    community_id: Option<i64>,
    include_members: bool,
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
    /// The repository, for the manifests that declare units.
    root: std::path::PathBuf,
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
        let community_id = match arguments.get("community_id") {
            None | Some(Value::Null) => None,
            Some(value) if value.is_i64() => value.as_i64(),
            Some(_) => return None,
        };
        let include_members = match arguments.get("include_members") {
            None => false,
            Some(Value::Bool(flag)) => *flag,
            Some(_) => return None,
        };
        Some(Self {
            sort_by: literal("sort_by", "size", &["size", "cohesion", "name"])?,
            min_size: args.integer("min_size", 0)?,
            community_name: args.optional_string("community_name")?,
            community_id,
            include_members,
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
            root: std::path::PathBuf::new(),
        })
    }
}

pub(crate) fn architecture(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    let mut request = Request::parse(&args, arguments)?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    request.root = root.path.clone();
    if request.mode == "community"
        && request.community_id.is_none()
        && request.community_name.is_none_or(str::is_empty)
    {
        // fastmcp accepts a community request with neither selector.
        return crate::dispatcher_error(
            context,
            &root,
            request.mode,
            "Value error, mode=\"community\" requires community_id or community_name.",
        );
    }
    let runtime = context.runtime.clone()?;
    let graph = open_graph(&root)?;
    let artifact = Artifact::parse(request.artifact_scope)?;
    let profile = Profile::parse(request.dependency_profile)?;
    let answerability = graph.answerability()?;
    let include_tests = request.artifact_scope != "code";
    let analysis = |mode: &str| -> Option<Ordered> {
        let read = Graph::read(&graph.store)?;
        let store = &graph.store;
        Some(match mode {
            "hubs" => hubs(
                context,
                &request,
                &answerability,
                find_hubs(store, &read, request.top_n, artifact, include_tests),
                include_tests,
            ),
            "bridges" => bridges(
                context,
                &request,
                &answerability,
                find_bridges(store, &read, request.top_n, artifact, include_tests),
                include_tests,
            ),
            "knowledge_gaps" => knowledge_gaps(
                context,
                &request,
                &answerability,
                find_knowledge_gaps(
                    store,
                    &read,
                    request.top_n,
                    artifact,
                    request.artifact_scope,
                    include_tests,
                ),
                include_tests,
            ),
            _ => surprising(
                context,
                &request,
                &answerability,
                find_surprising_connections(&read, request.top_n, artifact, include_tests),
                include_tests,
            ),
        })
    };
    let exposed = |tool: &str| context.exposes(tool);
    let (subtool, out, trailing) = match request.mode {
        "overview" => (
            "get_architecture_overview_func",
            with_unit_map(
                &root.path,
                &graph.store,
                request.detail_level,
                crate::community::overview(
                    &graph.store,
                    &answerability,
                    &exposed,
                    request.detail_level,
                    request.top_n,
                    request.artifact_scope,
                    artifact,
                )?,
            )?,
            false,
        ),
        "communities" => (
            "list_communities_func",
            crate::community::list_communities(
                &graph.store,
                &exposed,
                request.sort_by,
                request.min_size,
                request.detail_level,
                request.top_n,
            )?,
            true,
        ),
        "community" => (
            "get_community_func",
            crate::community::get_community(
                &graph.store,
                &exposed,
                request.community_name,
                request.community_id,
                request.include_members,
            )?,
            true,
        ),
        "adp_violations" => (
            "detect_adp_violations_func",
            adp(
                context,
                &request,
                &Snapshot::read(&graph.store)?,
                artifact,
                profile,
            )?,
            true,
        ),
        "sdp_metrics" => (
            "compute_sdp_metrics_func",
            sdp_metrics(
                context,
                &request,
                &Snapshot::read(&graph.store)?,
                artifact,
                profile,
            ),
            true,
        ),
        "sdp_violations" => (
            "detect_sdp_violations_func",
            sdp_violations(
                context,
                &request,
                &Snapshot::read(&graph.store)?,
                artifact,
                profile,
            )?,
            true,
        ),
        "sap_metrics" => (
            "compute_sap_metrics_func",
            sap(
                context,
                &request,
                &Snapshot::read(&graph.store)?,
                artifact,
                profile,
            ),
            true,
        ),
        "sap_violations" => (
            "detect_sap_violations_func",
            sap_violation_list(
                context,
                &request,
                &Snapshot::read(&graph.store)?,
                artifact,
                profile,
            )?,
            true,
        ),
        "hubs" => ("get_hub_nodes_func", analysis("hubs")?, false),
        "bridges" => ("get_bridge_nodes_func", analysis("bridges")?, false),
        "knowledge_gaps" => (
            "get_knowledge_gaps_func",
            analysis("knowledge_gaps")?,
            false,
        ),
        _ => (
            "get_surprising_connections_func",
            analysis("surprising_connections")?,
            false,
        ),
    };
    let out = match DEPRECATED_MODES.iter().find(|(mode, _)| *mode == request.mode) {
        Some((_, replacement)) => out.put(
            "deprecated",
            json!({"replacement": format!("architecture_analysis_tool {replacement}"), "removal": "next release"}),
        ),
        None => out,
    };
    // `attach_answerability` for a subtool that reports none.
    let trailing = if trailing {
        vec![
            ("answerability", answerability.full()),
            ("missingness", json!(answerability.missingness())),
        ]
    } else {
        Vec::new()
    };
    Some(seal_dispatch(
        out,
        crate::Dispatch {
            mode: request.mode,
            subtool,
            hints_tool: "architecture_analysis",
            runtime,
            trailing,
            repo: graph.repo_context(),
        },
        &exposed,
    ))
}

/// Units the overview lists; the rest are counted in `units_omitted`.
const MAX_UNITS: usize = 40;
/// Unit edges the overview lists, heaviest first.
const MAX_UNIT_EDGES: usize = 30;

/// The overview led by the map of declared units
/// (docs/plans/ARCHITECTURE-TOOL-TARGET.md#the-map); `surface` from
/// `standard` up.
fn with_unit_map(
    root: &std::path::Path,
    store: &dagayn_graph::GraphStore,
    detail_level: &str,
    rest: Ordered,
) -> Option<Ordered> {
    let snapshot = crate::architecture::Snapshot::read(store)?;
    let (mut units, mut unit_edges) = crate::units::unit_map(
        root,
        &snapshot.all_nodes,
        &snapshot.edges,
        detail_level != "minimal",
    );
    let (findings, findings_omitted) = crate::arch_findings::architecture_findings(root, &snapshot);
    let (unit_count, edge_count) = (units.len(), unit_edges.len());
    units.truncate(MAX_UNITS);
    unit_edges.truncate(MAX_UNIT_EDGES);
    let mut by_kind: Vec<(&str, usize)> = Vec::new();
    for finding in &findings {
        let kind = finding["kind"].as_str().unwrap_or("");
        match by_kind.iter_mut().find(|(known, _)| *known == kind) {
            Some((_, count)) => *count += 1,
            None => by_kind.push((kind, 1)),
        }
    }
    for (kind, count) in &mut by_kind {
        *count += findings_omitted.get(kind).copied().unwrap_or(0);
    }
    let findings_summary = if by_kind.is_empty() {
        "No structural findings.".to_string()
    } else {
        format!(
            "Findings: {}.",
            by_kind
                .iter()
                .map(|(kind, count)| format!("{count} {kind}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut summary = format!(
        "{unit_count} unit(s), {edge_count} dependency pair(s) between them. {findings_summary}"
    );
    if detail_level == "verbose"
        && let Some(legacy) = rest.get("summary").and_then(Value::as_str)
    {
        summary = format!("{summary} {legacy}");
    }
    let mut out = Ordered::default()
        .put("status", "ok")
        .put("summary", summary);
    out = out.put("units", json!(units));
    if unit_count > units.len() {
        out = out.put("units_omitted", unit_count - units.len());
    }
    out = out.put("unit_edges", json!(unit_edges));
    if edge_count > unit_edges.len() {
        out = out.put("unit_edges_omitted", edge_count - unit_edges.len());
    }
    out = out
        .put("findings", json!(findings))
        .put("next", crate::next::from_findings(&findings));
    if !findings_omitted.is_empty() {
        out = out.put("findings_omitted", json!(findings_omitted));
    }
    if detail_level == "verbose" {
        // The community-based health report the overview gave before the
        // map, for one release.
        let mut deprecated = Vec::new();
        for (key, value) in rest.into_entries() {
            if key == "status" || key == "summary" {
                continue;
            }
            if !matches!(key.as_str(), "artifact_scope" | "missingness" | "_hints") {
                deprecated.push(key.clone());
            }
            out = out.put(&key, value);
        }
        return Some(out.put("deprecated_fields", json!(deprecated)));
    }
    if let Some(scope) = rest.get("artifact_scope") {
        out = out.put("artifact_scope", scope.clone());
    }
    // The map reads no community: the community report's gaps stay with it.
    if let Some(Value::Array(missingness)) = rest.get("missingness") {
        let kept: Vec<&Value> = missingness
            .iter()
            .filter(|item| {
                !item["reason_code"]
                    .as_str()
                    .is_some_and(crate::answerability::is_derived_structure_code)
            })
            .collect();
        out = out.put("missingness", json!(kept));
    }
    let mut next_steps = Vec::new();
    if !findings.is_empty() {
        next_steps.push(json!({
            "tool": "query_graph_tool",
            "suggestion": "pattern=\"source_of\" or \"importers_of\" -- open the place each finding names",
        }));
    }
    if detail_level == "minimal" {
        next_steps.push(json!({
            "tool": "architecture_analysis_tool",
            "suggestion": "detail_level=\"standard\" -- the symbols other units use most (surface)",
        }));
    }
    Some(out.put(
        "_hints",
        json!({"next_steps": next_steps, "related": [], "warnings": []}),
    ))
}

/// Modes the overview's map and findings replace
/// (docs/plans/ARCHITECTURE-TOOL-TARGET.md#what-happens-to-the-current-modes),
/// kept for one release.
const DEPRECATED_MODES: &[(&str, &str)] = &[
    (
        "hubs",
        "mode=\"overview\" detail_level=\"standard\" (each unit's surface)",
    ),
    (
        "bridges",
        "mode=\"overview\" detail_level=\"standard\" (each unit's surface)",
    ),
    (
        "knowledge_gaps",
        "mode=\"overview\" findings (untested_core); refactor_tool mode=\"suggest\" for unused code",
    ),
    ("surprising_connections", "mode=\"overview\" unit_edges"),
    (
        "adp_violations",
        "mode=\"overview\" findings (import_cycle)",
    ),
];

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
    let file_scopes = request.granularity == "file";
    let view = View {
        file_scopes,
        artifact,
        profile,
        units: (!file_scopes).then(|| snapshot.unit_scopes(&request.root)),
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
        py_float(request.min_delta)
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

/// `scope_kind="package"` scopes by declared unit, `"directory"` by the
/// file's directory, `"file"` by file.
fn sap_view(request: &Request, snapshot: &Snapshot, artifact: Artifact, profile: Profile) -> View {
    View {
        file_scopes: request.scope_kind == "file",
        artifact,
        profile,
        units: (request.scope_kind == "package").then(|| snapshot.unit_scopes(&request.root)),
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
    let view = sap_view(request, snapshot, artifact, profile);
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
    let view = sap_view(request, snapshot, artifact, profile);
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
    let min_distance = py_float(request.min_distance);
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

/// `make_response` for an analysis subtool: its own answerability, its
/// guidance, and `_hints` from that guidance.
pub(crate) fn analysis_response(
    context: &Context,
    answerability: &Answerability,
    summary: String,
    mut fields: Vec<(&str, Value)>,
    guidance: Value,
    next: &[&str],
) -> Ordered {
    fields.push(("answerability", answerability.full()));
    fields.push((
        "missingness",
        json!(answerability.missingness_with_derived()),
    ));
    let hints = guidance_actions_to_hints(std::slice::from_ref(&guidance));
    fields.push(("guidance", json!([guidance])));
    make_response(context, summary, fields, next).replace("_hints", hints)
}

fn first(items: &[Value], count: usize) -> Value {
    json!(items.iter().take(count).cloned().collect::<Vec<_>>())
}

/// `get_hub_nodes_func`.
fn hubs(
    context: &Context,
    request: &Request,
    answerability: &Answerability,
    hubs: Vec<Value>,
    include_tests: bool,
) -> Ordered {
    let guidance = guidance_item(
        "Hub nodes are review leads because many edges meet there.".to_string(),
        json!({"type": "computed", "metric": "degree", "examples": first(&hubs, 3)}),
        if hubs.is_empty() { "low" } else { "medium" },
        vec![
            json!({"reason_code": "hub_score_is_degree_rank", "severity": "low", "claim_effect": "high degree is a lead, not proof of bad design"}),
        ],
        "review_tool mode=\"impact\" -- check blast radius of a hub",
        vec![json!("hub_nodes")],
        json!({"hub_nodes": hubs.len()}),
    );
    analysis_response(
        context,
        answerability,
        format!(
            "Found {} hub node(s) with highest connectivity.",
            hubs.len()
        ),
        vec![
            ("hub_nodes", Value::Array(hubs.clone())),
            ("count", json!(hubs.len())),
            ("artifact_scope", json!(request.artifact_scope)),
            ("include_tests", json!(include_tests)),
        ],
        guidance,
        &[
            "review_tool mode=\"impact\" -- check blast radius of a hub",
            "query_graph_tool callers_of -- see what calls a hub",
            "architecture_analysis_tool mode=\"bridges\" -- find architectural chokepoints",
        ],
    )
}

/// `get_bridge_nodes_func`.
fn bridges(
    context: &Context,
    request: &Request,
    answerability: &Answerability,
    bridges: Vec<Value>,
    include_tests: bool,
) -> Ordered {
    let guidance = guidance_item(
        "Bridge nodes are architectural chokepoints on many shortest paths.".to_string(),
        json!({"type": "computed", "metric": "betweenness", "examples": first(&bridges, 3)}),
        if bridges.is_empty() { "low" } else { "medium" },
        vec![
            json!({"reason_code": "betweenness_is_heuristic_lead", "severity": "low", "claim_effect": "betweenness ranks review priority, not runtime failure"}),
        ],
        "architecture_analysis_tool mode=\"hubs\" -- compare with high-degree nodes",
        vec![json!("bridge_nodes")],
        json!({"bridge_nodes": bridges.len()}),
    );
    analysis_response(
        context,
        answerability,
        format!(
            "Found {} bridge node(s) (high betweenness centrality).",
            bridges.len()
        ),
        vec![
            ("bridge_nodes", Value::Array(bridges.clone())),
            ("count", json!(bridges.len())),
            ("artifact_scope", json!(request.artifact_scope)),
            ("include_tests", json!(include_tests)),
        ],
        guidance,
        &[
            "architecture_analysis_tool mode=\"hubs\" -- find most connected nodes",
            "review_tool mode=\"impact\" -- check blast radius",
            "review_tool mode=\"changes\" -- see if bridges are affected",
        ],
    )
}

/// `get_surprising_connections_func`.
fn surprising(
    context: &Context,
    request: &Request,
    answerability: &Answerability,
    found: Vec<Value>,
    include_tests: bool,
) -> Ordered {
    let guidance = guidance_item(
        "Surprising connections are ranked coupling leads, not verdicts.".to_string(),
        json!({"type": "computed", "examples": first(&found, 3), "count": found.len()}),
        if found.is_empty() { "low" } else { "medium" },
        vec![
            json!({"reason_code": "surprise_score_is_heuristic", "severity": "low", "claim_effect": "scores prioritize review, not proof of bad design"}),
        ],
        "architecture_analysis_tool mode=\"overview\" -- inspect community structure",
        vec![json!("surprising_connections")],
        json!({"surprising_connections": found.len()}),
    );
    analysis_response(
        context,
        answerability,
        format!("Found {} surprising connection(s).", found.len()),
        vec![
            ("surprising_connections", Value::Array(found.clone())),
            ("count", json!(found.len())),
            ("artifact_scope", json!(request.artifact_scope)),
            ("include_tests", json!(include_tests)),
        ],
        guidance,
        &[
            "architecture_analysis_tool mode=\"overview\" -- community structure",
            "query_graph_tool callers_of -- trace the coupling",
            "architecture_analysis_tool mode=\"bridges\" -- find chokepoints",
        ],
    )
}

const GAP_KEYS: [&str; 4] = [
    "untested_hotspots",
    "single_file_communities",
    "isolated_nodes",
    "thin_communities",
];

/// `get_knowledge_gaps_func`.
fn knowledge_gaps(
    context: &Context,
    request: &Request,
    answerability: &Answerability,
    gaps: Value,
    include_tests: bool,
) -> Ordered {
    let meta = gaps["_meta"].clone();
    let raw = &meta["raw_counts"];
    let counts = |g: &Value| -> Value {
        Value::Object(
            GAP_KEYS
                .iter()
                .map(|k| (k.to_string(), json!(g[*k].as_array().map_or(0, Vec::len))))
                .collect(),
        )
    };
    let raw_counts: Value = Value::Object(
        GAP_KEYS
            .iter()
            .map(|k| (k.to_string(), raw[*k].clone()))
            .collect(),
    );
    let total: i64 = GAP_KEYS.iter().map(|k| raw[*k].as_i64().unwrap_or(0)).sum();
    let before = counts(&gaps);
    let guidance = guidance_item(
        format!("Found {total} knowledge-gap signal(s) across four structural categories."),
        json!({"type": "computed", "gap_counts": before, "thresholds": meta["thresholds"]}),
        if total > 0 { "medium" } else { "low" },
        vec![
            json!({"reason_code": "knowledge_gap_is_review_lead", "severity": "low", "claim_effect": "gaps highlight review targets, not automatic defects"}),
        ],
        "refactor_tool mode=\"dead_code\" -- cross-check unused symbols",
        vec![json!("knowledge_gaps")],
        json!({"total_gaps": total}),
    );
    let out = analysis_response(
        context,
        answerability,
        format!("Found {total} knowledge gaps across 4 categories."),
        vec![
            ("gaps", gaps.clone()),
            ("total_gaps", json!(total)),
            ("gap_counts", before.clone()),
            ("raw_gap_counts", raw_counts),
            ("thresholds", meta["thresholds"].clone()),
            ("degree_distribution", meta["degree_distribution"].clone()),
            ("artifact_scope", json!(request.artifact_scope)),
            ("include_tests", json!(include_tests)),
            ("scoped_counts", meta["scoped_counts"].clone()),
            (
                "truncated",
                json!(meta["truncated"].as_bool().unwrap_or(false)),
            ),
        ],
        guidance,
        &[
            "refactor dead_code -- find unused symbols",
            "architecture_analysis_tool mode=\"hubs\" -- find high-impact nodes",
            "get_suggested_questions -- review prompts",
        ],
    );
    // `apply_output_budget(payload["gaps"], 4000, ...)`.
    let entries = gaps
        .as_object()
        .into_iter()
        .flatten()
        .fold(Ordered::default(), |o, (k, v)| o.put(k, v.clone()));
    let trimmed = entries
        .apply_output_budget(
            4000,
            &[
                "isolated_nodes",
                "single_file_communities",
                "thin_communities",
                "untested_hotspots",
            ],
        )
        .value();
    let after = counts(&trimmed);
    let mut out = out.replace("gaps", trimmed.clone());
    if trimmed.get("truncated").and_then(Value::as_bool) == Some(true) {
        out = out
            .set("truncated", json!(true))
            .set(
                "budget_truncation",
                trimmed.get("_truncation").cloned().unwrap_or(json!({})),
            )
            .replace("gap_counts", after);
    } else if after != before {
        out = out
            .set("truncated", json!(true))
            .replace("gap_counts", after);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::py_float;

    /// Expected strings are CPython 3 `repr(x)` outputs.
    #[test]
    fn py_float_matches_python_repr() {
        let cases: &[(f64, &str)] = &[
            (0.1, "0.1"),
            (0.5, "0.5"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (2.0, "2.0"),
            (1e-4, "0.0001"),
            (123e-6, "0.000123"),
            (0.00012345, "0.00012345"),
            (1e-5, "1e-05"),
            (-1e-5, "-1e-05"),
            (1.5e-7, "1.5e-07"),
            (-2.5e-10, "-2.5e-10"),
            (5e-324, "5e-324"),
            (0.1 + 0.2, "0.30000000000000004"),
            (123456789.123, "123456789.123"),
            (1e15, "1000000000000000.0"),
            (9999999999999998.0, "9999999999999998.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (1e22, "1e+22"),
            (1e100, "1e+100"),
            (f64::MAX, "1.7976931348623157e+308"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
        ];
        for &(value, expected) in cases {
            assert_eq!(py_float(value), expected, "repr({value:?})");
        }
    }
}
