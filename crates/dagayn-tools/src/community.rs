//! `architecture_analysis_tool`'s `communities`, `community`, and `overview`
//! modes (`dagayn.tools.community_tools`, `dagayn.communities`).

use std::collections::HashMap;

use dagayn_graph::GraphStore;
use serde_json::{Map, Value, json};

use crate::analysis::{
    Graph, find_bridges, find_hubs, find_knowledge_gaps, find_surprising_connections, py_prefix,
};
use crate::answerability::Answerability;
use crate::architecture::{
    Artifact, Profile, ScopeGraph, Snapshot, View, float_or, sap_metrics, sap_violations, truthy,
};
use crate::query::{node_dict, sanitize};
use crate::review::guidance_actions_to_hints;
use crate::review_summary::{guidance_item, stability_profiles};
use crate::{Ordered, hints};

fn test_community() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(^test[-/]|[-/]test([:/]|$)|it:should|describe:|spec[-/]|[-/]spec$)",
        )
        .expect("regex")
    })
}

/// `get_communities(store, sort_by, min_size)`.
fn get_communities(store: &GraphStore, sort_by: &str, min_size: i64) -> Option<Vec<Value>> {
    serde_json::from_str(&store.get_communities_json(sort_by, min_size).ok()?).ok()
}

/// `Counter.most_common(n)`: count descending, first-seen order for ties;
/// nothing for `n <= 0`.
fn most_common<K: Clone>(counts: &[(K, i64)], limit: Option<i64>) -> Vec<(K, i64)> {
    let mut ranked: Vec<(K, i64)> = counts.to_vec();
    ranked.sort_by_key(|item| std::cmp::Reverse(item.1));
    match limit {
        Some(n) if n <= 0 => Vec::new(),
        Some(n) => ranked.into_iter().take(n as usize).collect(),
        None => ranked,
    }
}

/// `list_communities_func(sort_by, min_size, detail_level, limit)`.
pub(crate) fn list_communities(
    store: &GraphStore,
    exposed: &dyn Fn(&str) -> bool,
    sort_by: &str,
    min_size: i64,
    detail_level: &str,
    limit: i64,
) -> Option<Ordered> {
    let communities: Vec<Value> = if detail_level == "minimal" {
        store
            .community_summaries(sort_by, min_size)
            .ok()?
            .into_iter()
            .map(|(name, size, cohesion)| json!({"name": name, "size": size, "cohesion": cohesion}))
            .collect()
    } else {
        get_communities(store, sort_by, min_size)?
    };
    let total = communities.len();
    let truncated = total as i64 > limit;
    let mut summary = format!("Found {total} communities");
    if truncated {
        summary.push_str(&format!(". Showing first {limit}."));
    }
    let out = Ordered::default()
        .put("status", "ok")
        .put("summary", summary)
        .put("communities", Value::Array(py_prefix(&communities, limit)))
        .put("total", total)
        .put("truncated", truncated)
        .apply_output_budget(4000, &["communities"]);
    let hints = hints::generate_hints(
        "list_communities",
        &out.value(),
        &mut hints::session(),
        exposed,
    );
    Some(out.put("_hints", hints))
}

/// `get_community_func(community_name, community_id, include_members)`.
pub(crate) fn get_community(
    store: &GraphStore,
    exposed: &dyn Fn(&str) -> bool,
    name: Option<&str>,
    id: Option<i64>,
    include_members: bool,
) -> Option<Ordered> {
    let all = get_communities(store, "size", 0)?;
    let found = match (id, name) {
        (Some(id), _) => all.into_iter().find(|c| c["id"].as_i64() == Some(id)),
        (None, Some(name)) => {
            let wanted = name.to_lowercase();
            all.into_iter().find(|c| {
                c["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&wanted)
            })
        }
        (None, None) => None,
    };
    let Some(mut community) = found else {
        return Some(
            Ordered::default()
                .put("status", "not_found")
                .put("summary", "No community found matching the given criteria."),
        );
    };
    if include_members && let Some(cid) = community["id"].as_i64() {
        let members: Vec<Value> = store
            .get_nodes_by_community_id(cid)
            .ok()?
            .iter()
            .map(node_dict)
            .collect();
        let entries = community
            .as_object()?
            .iter()
            .fold(Ordered::default(), |o, (k, v)| o.put(k, v.clone()))
            .put("member_details", Value::Array(members));
        community = entries
            .apply_output_budget(5000, &["member_details"])
            .value();
    }
    let summary = format!(
        "Community '{}': {} nodes, cohesion {:.4}",
        community["name"].as_str().unwrap_or(""),
        community["size"],
        community["cohesion"].as_f64().unwrap_or(0.0)
    );
    let out = Ordered::default()
        .put("status", "ok")
        .put("summary", summary)
        .put("community", community);
    let hints = hints::generate_hints(
        "get_community",
        &out.value(),
        &mut hints::session(),
        exposed,
    );
    Some(out.put("_hints", hints))
}

/// `get_architecture_overview(store, detail_level, top_n)`.
fn architecture_overview(
    store: &GraphStore,
    edges: &[dagayn_graph::GraphEdge],
    detail_level: &str,
    top_n: i64,
) -> Option<Vec<(&'static str, Value)>> {
    let communities = get_communities(store, "size", 0)?;
    let mut node_community: HashMap<String, i64> = HashMap::new();
    for comm in &communities {
        let id = comm["id"].as_i64().unwrap_or(0);
        for qn in comm["members"].as_array().into_iter().flatten() {
            if let Some(qn) = qn.as_str() {
                node_community.insert(qn.to_string(), id);
            }
        }
    }
    let mut pairs: Vec<((i64, i64), i64)> = Vec::new();
    let mut kinds: HashMap<(i64, i64), Map<String, Value>> = HashMap::new();
    let mut cross_edges = Vec::new();
    for edge in edges {
        if edge.kind == "TESTED_BY" {
            continue;
        }
        let (Some(&s), Some(&t)) = (
            node_community.get(&edge.source_qualified),
            node_community.get(&edge.target_qualified),
        ) else {
            continue;
        };
        if s == t {
            continue;
        }
        let pair = (s.min(t), s.max(t));
        match pairs.iter_mut().find(|(p, _)| *p == pair) {
            Some(slot) => slot.1 += 1,
            None => pairs.push((pair, 1)),
        }
        let kind_counts = kinds.entry(pair).or_default();
        let count = kind_counts
            .get(&edge.kind)
            .and_then(Value::as_i64)
            .unwrap_or(0)
            + 1;
        kind_counts.insert(edge.kind.clone(), json!(count));
        if detail_level == "verbose" {
            cross_edges.push(json!({
                "source_community": s,
                "target_community": t,
                "edge_kind": edge.kind,
                "source": sanitize(&edge.source_qualified),
                "target": sanitize(&edge.target_qualified),
            }));
        }
    }
    let is_test = |name: &str| test_community().is_match(name);
    let mut warnings: Vec<String> = Vec::new();
    for comm in &communities {
        let stored = comm.get("size").cloned().unwrap_or(json!(0));
        let assigned = match comm.get("assigned_member_count") {
            Some(value) => value.clone(),
            None => json!(comm["members"].as_array().map_or(0, Vec::len)),
        };
        let name = comm["name"].as_str().unwrap_or("");
        if stored != assigned && !is_test(name) {
            warnings.push(format!(
                "Community '{name}' stored size ({stored}) differs from assigned members ({assigned}); run a full community refresh"
            ));
        }
    }
    let names: HashMap<i64, String> = communities
        .iter()
        .map(|c| {
            (
                c["id"].as_i64().unwrap_or(0),
                c["name"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let name_of = |id: i64| {
        names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("community-{id}"))
    };
    for ((c1, c2), count) in most_common(&pairs, None) {
        if count > 10 {
            let (n1, n2) = (name_of(c1), name_of(c2));
            if is_test(&n1) || is_test(&n2) {
                continue;
            }
            warnings.push(format!(
                "High coupling ({count} edges) between '{n1}' and '{n2}'"
            ));
        }
    }
    let limit = match detail_level {
        "verbose" => None,
        "minimal" => Some(5),
        _ => Some(top_n),
    };
    let coupling: Vec<Value> = most_common(&pairs, limit)
        .into_iter()
        .map(|((c1, c2), count)| {
            json!({
                "source_community_id": c1,
                "source_community_name": name_of(c1),
                "target_community_id": c2,
                "target_community_name": name_of(c2),
                "edge_count": count,
                "edge_kinds": kinds.get(&(c1, c2)).cloned().unwrap_or_default(),
            })
        })
        .collect();
    let out_communities: Vec<Value> = match detail_level {
        "minimal" => communities
            .iter()
            .map(|c| {
                let assigned = c.get("assigned_member_count").cloned().unwrap_or_else(|| json!(c["members"].as_array().map_or(0, Vec::len)));
                json!({"name": c["name"], "size": c["size"], "assigned_member_count": assigned, "cohesion": c["cohesion"]})
            })
            .collect(),
        "verbose" => communities.clone(),
        _ => communities
            .iter()
            .map(|c| {
                let mut map = c.as_object().cloned().unwrap_or_default();
                map.remove("members");
                Value::Object(map)
            })
            .collect(),
    };
    let mut result = vec![
        ("communities", Value::Array(out_communities)),
        ("cross_community_coupling", Value::Array(coupling)),
        ("warnings", json!(warnings)),
    ];
    if detail_level == "verbose" {
        result.push(("cross_community_edges", Value::Array(cross_edges)));
    }
    Some(result)
}

/// `stability_policy_summary(profiles, limit)`.
fn stability_policy_summary(profiles: &HashMap<String, Value>, limit: i64) -> Value {
    let flagged = |key: &str| {
        profiles
            .values()
            .filter(|p| truthy(&p[key]))
            .cloned()
            .collect::<Vec<_>>()
    };
    let (stable, should) = (flagged("stable"), flagged("should_be_stable"));
    let has_code = |p: &Value, code: &str| {
        p["reason_codes"]
            .as_array()
            .is_some_and(|c| c.iter().any(|x| x == code))
    };
    let pressure = profiles
        .values()
        .filter(|p| has_code(p, "stable_concrete_pressure"))
        .count();
    let mut examples: Vec<Value> = stable.iter().chain(&should).cloned().collect();
    examples.sort_by(|a, b| {
        float_or(&a["instability"], 1.0)
            .partial_cmp(&float_or(&b["instability"], 1.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b["ca"]
                    .as_i64()
                    .unwrap_or(0)
                    .cmp(&a["ca"].as_i64().unwrap_or(0))
            })
            .then_with(|| {
                a["scope_key"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(b["scope_key"].as_str().unwrap_or(""))
            })
    });
    let mut codes: Vec<String> = profiles
        .values()
        .flat_map(|p| p["reason_codes"].as_array().cloned().unwrap_or_default())
        .filter_map(|c| c.as_str().map(str::to_string))
        .collect();
    codes.sort();
    codes.dedup();
    let top: Vec<Value> = examples
        .iter()
        .take(limit.max(0) as usize)
        .map(|item| {
            json!({
                "scope_key": item["scope_key"],
                "instability": item["instability"],
                "ca": item["ca"],
                "ce": item["ce"],
                "reason_codes": item.get("reason_codes").cloned().unwrap_or(json!([])),
            })
        })
        .collect();
    json!({
        "thresholds": {
            "instability_max": 0.35,
            "afferent_coupling_should_be_stable_min": 3,
            "stable_expected_test_density": 0.8,
            "stable_expected_doc_density": 0.5,
            "default_expected_test_density": 0.5,
            "default_expected_doc_density": 0.25,
        },
        "counts": {
            "profiled_components": profiles.len(),
            "stable_components": stable.len(),
            "should_be_stable_components": should.len(),
            "stable_concrete_pressure_components": pressure,
        },
        "reason_codes": codes,
        "top_examples": top,
    })
}

fn drill(mode: &str, artifact: Option<&str>) -> Value {
    match artifact {
        Some(scope) => {
            json!({"tool": "architecture_analysis_tool", "mode": mode, "artifact_scope": scope})
        }
        None => json!({"tool": "architecture_analysis_tool", "mode": mode}),
    }
}

/// `_architecture_health_summary`; `None` where an ADP enumeration would
/// pass Python's cap.
fn architecture_health(
    store: &GraphStore,
    overview: &[(&'static str, Value)],
    top_n: i64,
    artifact_name: &str,
    artifact: Artifact,
) -> Option<Value> {
    let limit = top_n.clamp(1, 5);
    let include_tests = artifact_name != "code";
    let graph = Graph::read(store)?;
    let hubs = find_hubs(store, &graph, limit, artifact, include_tests);
    let bridges = find_bridges(store, &graph, limit, artifact, include_tests);
    let gaps = find_knowledge_gaps(store, &graph, limit, artifact, artifact_name, include_tests);
    let surprises = find_surprising_connections(&graph, limit, artifact, include_tests);
    let snapshot = Snapshot::read(store)?;
    let view = View {
        file_scopes: false,
        artifact,
        profile: Profile::StrictStatic,
    };
    let scopes = ScopeGraph::new(&snapshot.dependencies(&view));
    let adp = if scopes.is_empty() {
        Vec::new()
    } else {
        scopes.adp_violations(2, 10, view.profile)?
    };
    let adp = py_prefix(&adp, limit);
    let sdp = py_prefix(&scopes.sdp_violations(0.1, view.profile), limit);
    let sap = py_prefix(
        &sap_violations(&sap_metrics(&snapshot, &view, "package", None), 0.5),
        limit,
    );

    let keys = [
        "untested_hotspots",
        "single_file_communities",
        "isolated_nodes",
        "thin_communities",
    ];
    let raw = &gaps["_meta"]["raw_counts"];
    let gap_total: i64 = keys.iter().map(|k| raw[*k].as_i64().unwrap_or(0)).sum();
    let field = |key: &str| {
        overview
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or(json!([]))
    };
    let (communities, coupling, warnings) = (
        field("communities"),
        field("cross_community_coupling"),
        field("warnings"),
    );
    let length = |v: &Value| v.as_array().map_or(0, Vec::len);

    let mut reasons: Vec<&str> = Vec::new();
    let stale = communities.as_array().into_iter().flatten().any(|c| {
        let size = c.get("size").cloned().unwrap_or(Value::Null);
        c.get("assigned_member_count")
            .cloned()
            .unwrap_or_else(|| size.clone())
            != size
    });
    if stale {
        reasons.push("stale_community_membership");
    }
    if length(&warnings) > 0 {
        reasons.push("high_cross_community_coupling");
    }
    if !hubs.is_empty() {
        reasons.push("hub_nodes");
    }
    if !bridges.is_empty() {
        reasons.push("bridge_nodes");
    }
    if gap_total > 0 {
        reasons.push("knowledge_gaps");
    }
    if !surprises.is_empty() {
        reasons.push("surprising_connections");
    }
    if !adp.is_empty() {
        reasons.push("adp_violations");
    }
    if !sdp.is_empty() {
        reasons.push("sdp_violations");
    }
    if !sap.is_empty() {
        reasons.push("sap_violations");
    }
    let first3 = |items: &[Value]| json!(items.iter().take(3).cloned().collect::<Vec<_>>());
    let mut guidance = Vec::new();
    if !hubs.is_empty() {
        guidance.push(guidance_item(
            "Hub nodes are review leads because many edges meet there.".into(),
            json!({"type": "computed", "metric": "degree", "examples": first3(&hubs)}),
            "medium",
            vec![json!({"reason_code": "hub_score_is_degree_rank", "severity": "low", "claim_effect": "high degree is a lead, not proof of bad design"})],
            "architecture_analysis_tool mode=\"hubs\" -- inspect high-degree nodes",
            vec![json!("hub_nodes")],
            json!({"hub_nodes": hubs.len()}),
        ));
    }
    if !bridges.is_empty() {
        guidance.push(guidance_item(
            "Betweenness bridge nodes are interoperability chokepoints.".into(),
            json!({"type": "computed", "metric": "betweenness", "examples": first3(&bridges)}),
            "medium",
            vec![json!({"reason_code": "bridge_score_is_betweenness_rank", "severity": "low", "claim_effect": "high betweenness is a lead, not proof of bad design"})],
            "architecture_analysis_tool mode=\"bridges\" -- inspect chokepoints; query_graph_tool pattern=\"docs_for\" -- follow nearby contracts",
            vec![json!("bridge_nodes")],
            json!({"bridge_nodes": bridges.len()}),
        ));
    }
    let stats = store.get_stats().ok()?;
    let cross_artifact = stats
        .edges_by_kind
        .get("CROSS_ARTIFACT")
        .copied()
        .unwrap_or(0);
    if cross_artifact > 0 {
        reasons.push("cross_artifact_edges_present");
        guidance.push(guidance_item(
            format!("Graph contains {cross_artifact} CROSS_ARTIFACT edge(s); treat bridges as first-class transitions when reviewing coupling."),
            json!({"type": "extracted", "cross_artifact_edge_count": cross_artifact}),
            "medium",
            vec![json!({"reason_code": "cross_artifact_bridge_is_static_evidence", "severity": "low", "claim_effect": "prefer docs_for / implementations_of / reportable bridges; treat low-confidence bridges as caveats"})],
            "query_graph_tool pattern=\"docs_for\" -- follow documentation bridges; pattern=\"implementations_of\" -- follow implementation bridges",
            vec![json!("cross_artifact_edges_present")],
            json!({"cross_artifact_edges": cross_artifact}),
        ));
    }
    if !adp.is_empty() || !sdp.is_empty() || !sap.is_empty() {
        guidance.push(guidance_item(
            "Architecture metric violations should be reviewed as ranked leads.".into(),
            json!({"type": "computed", "adp_violations": adp.len(), "sdp_violations": sdp.len(), "sap_violations": sap.len(), "artifact_scope": artifact_name}),
            "medium",
            vec![json!({"reason_code": "metric_warning_not_verdict", "severity": "low", "claim_effect": "ADP/SDP/SAP signals need source-level review"})],
            "architecture_analysis_tool mode=\"sdp_violations\" -- drill into metric leads",
            vec![json!("adp_violations"), json!("sdp_violations"), json!("sap_violations")],
            json!({"adp_violations": adp.len(), "sdp_violations": sdp.len(), "sap_violations": sap.len()}),
        ));
    }
    let gap_examples: Map<String, Value> = keys
        .iter()
        .map(|k| {
            (
                k.to_string(),
                json!(
                    gaps[*k]
                        .as_array()
                        .map(|g| g
                            .iter()
                            .take(3.min(limit as usize))
                            .cloned()
                            .collect::<Vec<_>>())
                        .unwrap_or_default()
                ),
            )
        })
        .collect();
    let sap_examples: Vec<Value> = sap
        .iter()
        .map(|v| json!({"scope_key": v.get("scope_key"), "display_name": v.get("display_name"), "distance": v.get("distance"), "zone": v.get("zone")}))
        .collect();
    let a = Some(artifact_name);
    Some(json!({
        "status": "ok",
        "scoring_policy": {
            "version": "architecture-health-v1",
            "artifact_scope": artifact_name,
            "signals": ["community_coupling", "hub_nodes", "bridge_nodes", "cross_artifact_edges", "knowledge_gaps", "surprising_connections", "adp", "sdp", "sap"],
            "bounded_top_n": limit,
            "formulas": {
                "adp": "cycles in package dependency graph",
                "sdp": "dependencies should point toward lower instability",
                "sap": "distance from main sequence D=|A+I-1|",
            },
            "thresholds": {"sap_violation_distance_min": 0.5, "artifact_scope_default": "code", "code_scope_includes_tests": false},
        },
        "counts": {
            "communities": length(&communities),
            "coupled_pairs_shown": length(&coupling),
            "warnings": length(&warnings),
            "hub_nodes": hubs.len(),
            "bridge_nodes": bridges.len(),
            "cross_artifact_edges": cross_artifact,
            "knowledge_gaps": gap_total,
            "surprising_connections": surprises.len(),
            "adp_violations": adp.len(),
            "sdp_violations": sdp.len(),
            "sap_violations": sap.len(),
        },
        "reason_codes": reasons,
        "guidance": guidance,
        "top_examples": {
            "hub_nodes": hubs,
            "bridge_nodes": bridges,
            "knowledge_gaps": gap_examples,
            "surprising_connections": surprises,
            "adp_violations": adp,
            "sdp_violations": sdp,
            "sap_violations": sap_examples,
        },
        "drill_downs": {
            "communities": drill("communities", None),
            "coupling": drill("overview", None),
            "hubs": drill("hubs", None),
            "bridges": drill("bridges", None),
            "knowledge_gaps": drill("knowledge_gaps", None),
            "surprising_connections": drill("surprising_connections", None),
            "adp": drill("adp_violations", a),
            "sdp": drill("sdp_violations", a),
            "sap": drill("sap_violations", a),
        },
    }))
}

/// `get_architecture_overview_func(detail_level, top_n, artifact_scope)`.
pub(crate) fn overview(
    store: &GraphStore,
    answerability: &Answerability,
    exposed: &dyn Fn(&str) -> bool,
    detail_level: &str,
    top_n: i64,
    artifact_name: &str,
    artifact: Artifact,
) -> Option<Ordered> {
    let edges = store.get_all_edges().ok()?;
    let overview = architecture_overview(store, &edges, detail_level, top_n)?;
    let count = |key: &str| {
        overview
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.as_array())
            .map_or(0, Vec::len)
    };
    let (communities, coupling, warnings) = (
        count("communities"),
        count("cross_community_coupling"),
        count("warnings"),
    );
    let shown = if detail_level == "standard" {
        format!(" (top {coupling} shown)")
    } else {
        String::new()
    };
    let health = architecture_health(store, &overview, top_n, artifact_name, artifact)?;
    let snapshot = Snapshot::read(store)?;
    let review = View::review();
    let scopes = ScopeGraph::new(&snapshot.dependencies(&review));
    let profiles = stability_profiles(&scopes, &sap_metrics(&snapshot, &review, "package", None));
    let mut out = Ordered::default()
        .put("status", "ok")
        .put("summary", format!("Architecture: {communities} communities, {coupling} coupled pairs{shown}, {warnings} warning(s)"))
        .put("artifact_scope", artifact_name);
    for (key, value) in overview {
        out = out.put(key, value);
    }
    let out = out
        .put("architecture_health", health)
        .put(
            "stable_component_policy",
            stability_policy_summary(&profiles, top_n.clamp(1, 5)),
        )
        .put("answerability", answerability.full())
        .put("missingness", json!(answerability.missingness()))
        .apply_output_budget(
            4000,
            &[
                "architecture_health",
                "warnings",
                "communities",
                "cross_community_coupling",
                "cross_community_edges",
            ],
        );
    let guidance = out
        .get("architecture_health")
        .and_then(|h| h.get("guidance"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut hint = guidance_actions_to_hints(&guidance);
    if hint["next_steps"].as_array().is_none_or(Vec::is_empty) {
        hint = hints::generate_hints(
            "get_architecture_overview",
            &out.value(),
            &mut hints::session(),
            exposed,
        );
    }
    Some(out.put("_hints", hint))
}
