//! `dagayn.tools.review_helpers._change_analysis_summary` at
//! `detail_level="standard"`, with the architecture metrics it reads
//! (`dagayn.architecture`, `dagayn.sap`, `dagayn.stability_policy`).
//!
//! `None` where Python would compute something this does not (betweenness
//! without persisted scores, more ADP cycles than Python enumerates).

use std::collections::{HashMap, HashSet};

use dagayn_graph::{
    GraphNode, GraphStore, ImpactRadius, bridge_transition_value, is_low_confidence_bridge,
    is_low_confidence_unresolved_markdown_code_span, is_reportable_bridge,
};
use serde_json::{Map, Value, json};

use crate::architecture::{
    Artifact, ScopeGraph, Snapshot, View, file_name, float_or, sap_metrics, sap_violations,
    scope_key_for_file, str_of, truthy,
};
use crate::changes::HEURISTIC_GAP_NODE_LIMIT;
use crate::coverage::{ScanState, infer_tests_for_node, is_test_file_path};
use crate::query::cross_artifact_role;
use crate::suggestions::round_to;

const ARTIFACT_TO_DOC_ROLES: &[&str] = &[
    "implements_contract",
    "explained_by",
    "has_runbook",
    "problem_described_by",
    "discussed_by",
];
const DOC_TO_ARTIFACT_ROLES: &[&str] = &[
    "implemented_by",
    "describes_symbol",
    "discusses_artifact",
    "raises_issue_for",
];
const CONTRACT_DOC_ROLES: &[&str] = &[
    "implements_contract",
    "implemented_by",
    "has_runbook",
    "explained_by",
];
const LOW_SIGNAL_DOC_FILES: &[&str] = &[
    "AGENTS.md",
    "CHANGELOG.md",
    "CLAUDE.md",
    "GEMINI.md",
    "QODER.md",
];

const STABLE_INSTABILITY_MAX: f64 = 0.35;
const SHOULD_BE_STABLE_CA_MIN: i64 = 3;
const STABLE_TEST_DENSITY_TARGET: f64 = 0.8;
const STABLE_DOC_DENSITY_TARGET: f64 = 0.5;
const DEFAULT_TEST_DENSITY_TARGET: f64 = 0.5;
const DEFAULT_DOC_DENSITY_TARGET: f64 = 0.25;

fn scope_key_for_record(record: &Value) -> Option<String> {
    let file = match record.get("file_path") {
        Some(value) if truthy(value) => value,
        _ => record.get("file").unwrap_or(&Value::Null),
    };
    if truthy(file) {
        scope_key_for_file(str_of(file))
    } else {
        None
    }
}

fn is_markdown_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".mdx")
}

fn is_low_signal_doc_path(path: &str) -> bool {
    LOW_SIGNAL_DOC_FILES.contains(&file_name(&path.replace('\\', "/")))
}

fn stability_thresholds() -> Value {
    json!({
        "instability_max": STABLE_INSTABILITY_MAX,
        "afferent_coupling_should_be_stable_min": SHOULD_BE_STABLE_CA_MIN,
        "stable_expected_test_density": STABLE_TEST_DENSITY_TARGET,
        "stable_expected_doc_density": STABLE_DOC_DENSITY_TARGET,
        "default_expected_test_density": DEFAULT_TEST_DENSITY_TARGET,
        "default_expected_doc_density": DEFAULT_DOC_DENSITY_TARGET,
    })
}

/// `component_stability_profiles`.
pub(crate) fn stability_profiles(graph: &ScopeGraph, sap: &[Value]) -> HashMap<String, Value> {
    let mut profiles: HashMap<String, Value> = HashMap::new();
    for (scope, ca, ce, instability) in graph.sdp_metrics() {
        let mut reasons: Vec<&str> = Vec::new();
        if ca + ce > 0 && instability <= STABLE_INSTABILITY_MAX {
            reasons.push("observed_stable_component");
        }
        if ca >= SHOULD_BE_STABLE_CA_MIN || (ca >= 2 && ca > ce) {
            reasons.push("high_afferent_coupling_should_be_stable");
        }
        let flagged = !reasons.is_empty();
        profiles.insert(
            scope.clone(),
            json!({
                "scope_key": scope,
                "ca": ca,
                "ce": ce,
                "instability": round_to(instability, 4),
                "stable": reasons.contains(&"observed_stable_component"),
                "should_be_stable": reasons.contains(&"high_afferent_coupling_should_be_stable"),
                "reason_codes": reasons,
                "thresholds": stability_thresholds(),
                "expected_test_density": if flagged { STABLE_TEST_DENSITY_TARGET } else { DEFAULT_TEST_DENSITY_TARGET },
                "test_density_metric": "direct_test_density",
                "supplemental_test_density_metrics": ["heuristic_test_density", "transitive_test_density"],
                "expected_doc_density": if flagged { STABLE_DOC_DENSITY_TARGET } else { DEFAULT_DOC_DENSITY_TARGET },
            }),
        );
    }
    for metric in sap {
        let scope = str_of(&metric["scope_key"]).to_string();
        if scope.is_empty() {
            continue;
        }
        let profile = profiles.entry(scope.clone()).or_insert_with(|| {
            json!({
                "scope_key": scope,
                "ca": metric["ca"].as_i64().unwrap_or(0),
                "ce": metric["ce"].as_i64().unwrap_or(0),
                "instability": float_or(&metric["instability"], 0.0),
                "stable": false,
                "should_be_stable": false,
                "reason_codes": [],
                "thresholds": stability_thresholds(),
                "expected_test_density": DEFAULT_TEST_DENSITY_TARGET,
                "test_density_metric": "direct_test_density",
                "supplemental_test_density_metrics": ["heuristic_test_density", "transitive_test_density"],
                "expected_doc_density": DEFAULT_DOC_DENSITY_TARGET,
            })
        });
        profile["abstractness"] = metric["abstractness"].clone();
        profile["sap_distance"] = metric["distance"].clone();
        profile["sap_notes"] = metric.get("notes").cloned().unwrap_or(json!([]));
        profile["sap_applicable"] = metric["sap_applicable"].clone();
        profile["sap_applicability_reason"] = metric["applicability_reason"].clone();
        if !metric["sap_applicable"].as_bool().unwrap_or(true) {
            continue;
        }
        let distance = float_or(&metric["distance"], 0.0);
        let instability = float_or(&profile["instability"], 0.0);
        if distance >= 0.5 && instability <= STABLE_INSTABILITY_MAX {
            if let Some(codes) = profile["reason_codes"].as_array_mut()
                && !codes.iter().any(|code| code == "stable_concrete_pressure")
            {
                codes.push(json!("stable_concrete_pressure"));
            }
            profile["should_be_stable"] = json!(true);
            profile["expected_test_density"] = json!(STABLE_TEST_DENSITY_TARGET);
            profile["expected_doc_density"] = json!(STABLE_DOC_DENSITY_TARGET);
        }
    }
    profiles
}

fn is_production_code_node(node: &GraphNode) -> bool {
    matches!(node.kind.as_str(), "Function" | "Class")
        && !node.is_test
        && !is_test_file_path(&node.file_path)
        && node.language != "markdown"
        && !is_markdown_path(&node.file_path)
}

fn doc_evidence_type(role: Option<&str>, tier: &str) -> &'static str {
    if role.is_some_and(|role| CONTRACT_DOC_ROLES.contains(&role)) {
        return "authored";
    }
    if matches!(tier.to_uppercase().as_str(), "EXTRACTED" | "HIGH") {
        "extracted"
    } else {
        "heuristic_reachable"
    }
}

/// `_component_density_by_scope`; `supplemental` (`detail_level="verbose"`)
/// also samples the first [`HEURISTIC_GAP_NODE_LIMIT`] production
/// nodes of each scope, by qualified name, for heuristic and transitive tests.
fn component_density(
    store: &GraphStore,
    scan: &mut Option<ScanState>,
    snapshot_nodes: &[GraphNode],
    scopes: &HashSet<String>,
    supplemental: bool,
) -> Option<HashMap<String, Value>> {
    let mut densities = HashMap::new();
    if scopes.is_empty() {
        return Some(densities);
    }
    let mut scope_nodes: Vec<(String, Vec<&GraphNode>)> = Vec::new();
    let mut test_counts: HashMap<String, i64> = HashMap::new();
    for node in snapshot_nodes {
        let Some(scope) = scope_key_for_file(&node.file_path) else {
            continue;
        };
        if !scopes.contains(&scope) {
            continue;
        }
        if is_production_code_node(node) {
            match scope_nodes.iter_mut().find(|(s, _)| *s == scope) {
                Some((_, nodes)) => nodes.push(node),
                None => scope_nodes.push((scope, vec![node])),
            }
        } else if node.is_test || node.kind == "Test" {
            *test_counts.entry(scope).or_default() += 1;
        }
    }
    let qns: Vec<String> = scope_nodes
        .iter()
        .flat_map(|(_, nodes)| nodes.iter().map(|n| n.qualified_name.clone()))
        .collect();
    let (outgoing, incoming) = store.get_edges_by_endpoints(&qns).ok()?;
    for (scope, mut nodes) in scope_nodes {
        nodes.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
        let sampled = if supplemental {
            nodes.len().min(HEURISTIC_GAP_NODE_LIMIT)
        } else {
            0
        };
        let (mut tested, mut authored, mut extracted, mut heuristic) = (0, 0, 0, 0);
        let (mut heuristic_tested, mut transitive_tested) = (0, 0);
        for (index, node) in nodes.iter().enumerate() {
            let out = outgoing
                .get(&node.qualified_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let inc = incoming
                .get(&node.qualified_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if out.iter().any(|edge| edge.kind == "TESTED_BY") {
                tested += 1;
            }
            if index < sampled {
                if scan.is_none() {
                    *scan = Some(ScanState::build(store)?);
                }
                let state = scan.as_mut()?;
                let inferred = infer_tests_for_node(store, state, node, 1, "medium")?;
                if inferred.iter().any(|row| {
                    row.iter()
                        .any(|(key, value)| *key == "coverage_source" && value == "heuristic")
                }) {
                    heuristic_tested += 1;
                }
                let transitive = store.get_transitive_tests(&node.qualified_name, 1).ok()?;
                if transitive.iter().any(|test| truthy(&test["indirect"])) {
                    transitive_tested += 1;
                }
            }
            let mut types: HashSet<&str> = HashSet::new();
            for (edges, roles) in [(out, ARTIFACT_TO_DOC_ROLES), (inc, DOC_TO_ARTIFACT_ROLES)] {
                for edge in edges {
                    if is_low_confidence_unresolved_markdown_code_span(edge) {
                        continue;
                    }
                    let role = cross_artifact_role(edge);
                    if role.is_some_and(|role| roles.contains(&role)) {
                        types.insert(doc_evidence_type(role, edge.confidence_tier.as_str()));
                    }
                }
            }
            authored += i64::from(types.contains("authored"));
            extracted += i64::from(types.contains("extracted"));
            heuristic += i64::from(types.contains("heuristic_reachable"));
        }
        let prod = nodes.len() as i64;
        let documented = authored + extracted;
        let ratio_of = |count: i64, denominator: i64| {
            if denominator > 0 {
                round_to(count as f64 / denominator as f64, 4)
            } else {
                0.0
            }
        };
        let ratio = |count: i64| ratio_of(count, prod);
        let sampled = sampled as i64;
        let supplemental_denominator = if supplemental { sampled } else { prod };
        densities.insert(
            scope.clone(),
            json!({
                "production_node_count": prod,
                "test_node_count": test_counts.get(&scope).copied().unwrap_or(0),
                "tested_node_count": tested,
                "heuristic_tested_node_count": heuristic_tested,
                "transitive_tested_node_count": transitive_tested,
                "supplemental_test_density_evaluated": supplemental,
                "supplemental_test_density_sampled_node_count": sampled,
                "supplemental_test_density_truncated": supplemental && sampled < prod,
                "documented_node_count": documented,
                "authored_documented_node_count": authored,
                "extracted_documented_node_count": extracted,
                "heuristic_documented_node_count": heuristic,
                "direct_test_density": ratio(tested),
                "heuristic_test_density": ratio_of(heuristic_tested, supplemental_denominator),
                "transitive_test_density": ratio_of(transitive_tested, supplemental_denominator),
                "documentation_density": ratio(documented),
                "authored_documentation_density": ratio(authored),
                "extracted_documentation_density": ratio(extracted),
                "heuristic_documentation_density": ratio(heuristic),
            }),
        );
    }
    Some(densities)
}

fn stability_of(profile: Option<&Value>) -> (bool, Value) {
    let profile = profile.cloned().unwrap_or(json!({}));
    let get = |key: &str| profile.get(key).cloned().unwrap_or(Value::Null);
    let flagged = truthy(&get("stable")) || truthy(&get("should_be_stable"));
    (
        flagged,
        json!({
            "stable": truthy(&get("stable")),
            "should_be_stable": truthy(&get("should_be_stable")),
            "instability": get("instability"),
        }),
    )
}

/// `_dedupe_dicts_by_key(items, "qualified_name", limit)` after the shared
/// `(-score, qualified_name)` sort.
fn rank_and_dedupe(mut items: Vec<Value>, limit: usize) -> Vec<Value> {
    items.sort_by(|left, right| {
        let score = |v: &Value| float_or(&v["score"], 0.0);
        score(right)
            .partial_cmp(&score(left))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| str_of(&left["qualified_name"]).cmp(str_of(&right["qualified_name"])))
    });
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let Some(key) = item["qualified_name"].as_str().filter(|k| !k.is_empty()) else {
            continue;
        };
        if !seen.insert(key.to_string()) {
            continue;
        }
        out.push(item);
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// `_recommend_tests`.
fn recommend_tests(
    store: &GraphStore,
    scan: &mut Option<ScanState>,
    changed: &[Value],
    flows: &[Value],
    profiles: &HashMap<String, Value>,
) -> Option<Vec<Value>> {
    let mut recommendations = Vec::new();
    for func in changed {
        let Some(qn) = func["qualified_name"].as_str().filter(|q| !q.is_empty()) else {
            continue;
        };
        let scope = scope_key_for_record(func);
        let (flagged, stability) = stability_of(profiles.get(scope.as_deref().unwrap_or("")));
        let bonus = if flagged { 0.1 } else { 0.0 };
        let tests = store.get_transitive_tests(qn, 1).ok()?;
        for test in &tests {
            let Some(test_qn) = test["qualified_name"].as_str() else {
                continue;
            };
            let indirect = truthy(&test["indirect"]);
            let name = test
                .get("name")
                .cloned()
                .unwrap_or_else(|| json!(test_qn.rsplit("::").next().unwrap_or(test_qn)));
            recommendations.push(json!({
                "name": name,
                "qualified_name": test_qn,
                "file": test.get("file_path").cloned().unwrap_or(Value::Null),
                "reason": if indirect { "indirect coverage via changed dependency" } else { "direct coverage of changed code" },
                "source": qn,
                "scope_key": scope,
                "score": round_to((if indirect { 0.82_f64 } else { 0.95 } + bonus).min(1.0), 4),
                "evidence_level": if indirect { "graph_indirect" } else { "graph_direct" },
                "stability": stability,
            }));
        }
        if !tests.is_empty() {
            continue;
        }
        let Some(node) = store.get_node(qn).ok()? else {
            continue;
        };
        if scan.is_none() {
            *scan = Some(ScanState::build(store)?);
        }
        let state = scan.as_mut()?;
        for row in infer_tests_for_node(store, state, &node, 3, "medium")? {
            let test: Map<String, Value> =
                row.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
            let Some(test_qn) = test.get("qualified_name").and_then(Value::as_str) else {
                continue;
            };
            let confidence = test.get("confidence").cloned().unwrap_or(Value::Null);
            recommendations.push(json!({
                "name": test.get("name").cloned().unwrap_or(Value::Null),
                "qualified_name": test_qn,
                "file": test.get("file_path").cloned().unwrap_or(Value::Null),
                "reason": "heuristic coverage candidate",
                "source": qn,
                "confidence": confidence,
                "evidence": test.get("evidence").cloned().unwrap_or(json!([])),
                "scope_key": scope,
                "score": round_to((if confidence == "high" { 0.75_f64 } else { 0.65 } + bonus).min(1.0), 4),
                "evidence_level": "heuristic",
                "stability": stability,
            }));
        }
    }
    for flow in flows {
        let name = match &flow["name"] {
            Value::String(name) => name.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        let lower = name.to_lowercase();
        if !(lower.starts_with("test") || lower.starts_with("it:") || lower.starts_with("describe"))
        {
            continue;
        }
        recommendations.push(json!({
            "name": name,
            "qualified_name": name,
            "file": null,
            "reason": "affected test flow",
            "source": "affected_flows",
            "score": 0.55,
            "evidence_level": "flow",
        }));
    }
    Some(rank_and_dedupe(recommendations, 10))
}

fn doc_missingness(role: Option<&str>, tier: &str) -> Vec<Value> {
    let mut missing = Vec::new();
    if !role.is_some_and(|role| CONTRACT_DOC_ROLES.contains(&role)) {
        missing.push(json!({
            "reason_code": "not_contract_documentation_edge",
            "severity": "low",
            "claim_effect": "candidate may be explanatory rather than contract-bearing",
        }));
    }
    if matches!(tier.to_uppercase().as_str(), "LOW" | "UNKNOWN" | "") {
        missing.push(json!({
            "reason_code": "low_confidence_documentation_edge",
            "severity": "medium",
            "claim_effect": "read the section before treating it as authored evidence",
        }));
    }
    missing
}

fn directive_hint(role: Option<&str>, artifact_to_doc: bool) -> &'static str {
    if artifact_to_doc {
        return match role {
            Some("implements_contract") => "# dagayn: implements <doc-section>",
            Some("explained_by") => "# dagayn: explained-by <doc-section>",
            Some("has_runbook") => "# dagayn: has-runbook <doc-section>",
            Some("problem_described_by") => "# dagayn: problem-described-by <doc-section>",
            _ => "# dagayn: discussed-by <doc-section>",
        };
    }
    match role {
        Some("implemented_by") => "<!-- dagayn: implemented-by <code-symbol> -->",
        Some("describes_symbol") => "<!-- dagayn: describes-symbol <code-symbol> -->",
        Some("raises_issue_for") => "<!-- dagayn: raises-issue-for <code-symbol> -->",
        _ => "<!-- dagayn: discusses-artifact <code-symbol> -->",
    }
}

fn doc_role_weight(role: Option<&str>) -> f64 {
    match role {
        Some("implements_contract" | "implemented_by") => 0.95,
        Some("has_runbook") => 0.85,
        Some("explained_by" | "describes_symbol") => 0.75,
        Some("problem_described_by" | "raises_issue_for") => 0.65,
        Some("discussed_by" | "discusses_artifact") => 0.45,
        _ => 0.25,
    }
}

fn confidence_weight(confidence: f64, tier: &str) -> f64 {
    let tier_weight = match tier.to_uppercase().as_str() {
        "EXTRACTED" => 1.0,
        "HIGH" => 0.9,
        "MEDIUM" => 0.65,
        "LOW" => 0.35,
        _ => 0.5,
    };
    confidence.max(tier_weight)
}

/// `_documentation_update_candidates`; `heuristic` (`detail_level="verbose"`)
/// adds the markdown nodes the blast radius reaches.
fn documentation_candidates(
    store: &GraphStore,
    impact: &ImpactRadius,
    changed: &[Value],
    changed_files: &[String],
    profiles: &HashMap<String, Value>,
    heuristic: bool,
) -> Option<Vec<Value>> {
    if !changed_files.iter().any(|path| !is_markdown_path(path)) {
        return Some(Vec::new());
    }
    let qns: Vec<String> = changed
        .iter()
        .filter_map(|func| func["qualified_name"].as_str().filter(|q| !q.is_empty()))
        .map(str::to_string)
        .collect();
    let mut by_qn: HashMap<&str, &Value> = HashMap::new();
    for func in changed {
        if let Some(qn) = func["qualified_name"].as_str() {
            by_qn.insert(qn, func);
        }
    }
    let (outgoing, incoming) = store.get_edges_by_endpoints(&qns).ok()?;
    let mut candidates = Vec::new();
    let mut doc_qns: HashSet<String> = HashSet::new();
    for qn in &qns {
        let record = by_qn
            .get(qn.as_str())
            .copied()
            .cloned()
            .unwrap_or(json!({}));
        let scope = scope_key_for_record(&record);
        let (flagged, _) = stability_of(profiles.get(scope.as_deref().unwrap_or("")));
        let bonus = if flagged { 0.08 } else { 0.0 };
        for (edges, roles, artifact_to_doc) in [
            (outgoing.get(qn), ARTIFACT_TO_DOC_ROLES, true),
            (incoming.get(qn), DOC_TO_ARTIFACT_ROLES, false),
        ] {
            for edge in edges.into_iter().flatten() {
                if is_low_confidence_unresolved_markdown_code_span(edge) {
                    continue;
                }
                let role = cross_artifact_role(edge);
                if !role.is_some_and(|role| roles.contains(&role)) {
                    continue;
                }
                let contract = role.is_some_and(|role| CONTRACT_DOC_ROLES.contains(&role));
                if is_low_signal_doc_path(&edge.file_path) && !contract {
                    continue;
                }
                let tier = edge.confidence_tier.as_str();
                let (doc_qn, score, reason) = if artifact_to_doc {
                    (
                        edge.target_qualified.clone(),
                        (doc_role_weight(role) + bonus).min(1.0),
                        "documentation edge from changed code",
                    )
                } else {
                    (
                        edge.source_qualified.clone(),
                        (doc_role_weight(role)
                            + 0.08 * confidence_weight(edge.confidence, tier)
                            + bonus)
                            .min(1.0),
                        "documentation edge to changed code",
                    )
                };
                doc_qns.insert(doc_qn.clone());
                candidates.push(json!({
                    "file": edge.file_path,
                    "section": doc_qn.rsplit("::").next().unwrap_or(&doc_qn),
                    "qualified_name": doc_qn,
                    "reason": reason,
                    "source": qn,
                    "relationship_role": role,
                    "confidence": edge.confidence,
                    "confidence_tier": tier,
                    "score": round_to(score, 4),
                    "evidence_level": "cross_artifact",
                    "evidence_type": doc_evidence_type(role, tier),
                    "missingness": doc_missingness(role, tier),
                    "documentation_action": "Read this section and update the contract directive if behavior changed.",
                    "directive_hint": directive_hint(role, artifact_to_doc),
                    "scope_key": scope,
                    "stable_contract": contract,
                }));
            }
        }
    }
    if heuristic {
        let changed_set: HashSet<&str> = changed_files.iter().map(String::as_str).collect();
        for node in &impact.impacted_nodes {
            let file = node.file_path.as_str();
            if !is_markdown_path(file)
                || is_low_signal_doc_path(file)
                || changed_set.contains(file)
                || doc_qns.contains(&node.qualified_name)
            {
                continue;
            }
            candidates.push(json!({
                "file": file,
                "section": node.name,
                "qualified_name": node.qualified_name,
                "reason": "markdown node reached from changed code/doc graph",
                "score": 0.25,
                "evidence_level": "heuristic_reachable",
                "evidence_type": "heuristic_reachable",
                "missingness": [{
                    "reason_code": "heuristic_documentation_reachability",
                    "severity": "medium",
                    "claim_effect": "candidate is reachable but not an authored contract edge",
                }],
                "documentation_action": "Read this section before deciding whether docs need updates.",
                "directive_hint": directive_hint(None, false),
            }));
        }
    }
    Some(rank_and_dedupe(candidates, 10))
}

/// `_stability_contracts`.
fn stability_contracts(
    changed: &[Value],
    tests: &[Value],
    docs: &[Value],
    gaps: &[Value],
    profiles: &HashMap<String, Value>,
    density: &HashMap<String, Value>,
) -> Vec<Value> {
    let sources = |items: &[Value]| -> HashSet<String> {
        items
            .iter()
            .filter_map(|item| item["source"].as_str().map(str::to_string))
            .collect()
    };
    let (tested_sources, doc_sources) = (sources(tests), sources(docs));
    let gap_qns: HashSet<String> = gaps
        .iter()
        .filter(|gap| truthy(&gap["qualified_name"]))
        .map(|gap| match &gap["qualified_name"] {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect();
    let mut by_scope: Vec<(String, Vec<&Value>)> = Vec::new();
    for func in changed {
        if !matches!(str_of(&func["kind"]), "Function" | "Class") {
            continue;
        }
        let file = match &func["file_path"] {
            value if truthy(value) => str_of(value).to_string(),
            _ => str_of(&func["file"]).to_string(),
        };
        if truthy(&func["is_test"]) || is_test_file_path(&file) || is_markdown_path(&file) {
            continue;
        }
        if let Some(scope) = scope_key_for_record(func) {
            match by_scope.iter_mut().find(|(s, _)| *s == scope) {
                Some((_, funcs)) => funcs.push(func),
                None => by_scope.push((scope, vec![func])),
            }
        }
    }
    let mut contracts = Vec::new();
    for (scope, funcs) in by_scope {
        let Some(profile) = profiles.get(&scope).filter(|p| truthy(p)) else {
            continue;
        };
        if !truthy(&profile["stable"]) && !truthy(&profile["should_be_stable"]) {
            continue;
        }
        let measured = density.get(&scope).filter(|d| truthy(d));
        let qns: Vec<&str> = funcs
            .iter()
            .filter_map(|f| f["qualified_name"].as_str())
            .collect();
        let with_tests: Vec<&str> = qns
            .iter()
            .copied()
            .filter(|q| tested_sources.contains(*q))
            .collect();
        let with_docs: Vec<&str> = qns
            .iter()
            .copied()
            .filter(|q| doc_sources.contains(*q))
            .collect();
        let missing_tests: Vec<&str> = qns
            .iter()
            .copied()
            .filter(|q| !with_tests.contains(q) && gap_qns.contains(*q))
            .collect();
        let missing_docs: Vec<&str> = qns
            .iter()
            .copied()
            .filter(|q| !with_docs.contains(q))
            .collect();
        let supplemental =
            measured.is_some_and(|d| truthy(&d["supplemental_test_density_evaluated"]));
        // The supplemental pass only runs at `detail_level="verbose"`.
        let supplemental_density = |key: &str| match measured {
            Some(d) if supplemental => json!(float_or(&d[key], 0.0)),
            _ => Value::Null,
        };
        let observed_tests = measured.map(|d| float_or(&d["direct_test_density"], 0.0));
        let observed_docs = measured.map(|d| float_or(&d["documentation_density"], 0.0));
        let expected_tests = float_or(&profile["expected_test_density"], 0.5);
        let expected_docs = float_or(&profile["expected_doc_density"], 0.25);
        let mut reasons: Vec<Value> = profile["reason_codes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        match observed_tests {
            Some(observed) if observed < expected_tests => {
                reasons.push(json!("stable_component_low_test_density"));
            }
            None => reasons.push(json!("stable_component_test_density_unmeasured")),
            _ => {}
        }
        if observed_docs.is_some_and(|observed| observed < expected_docs)
            || !missing_docs.is_empty()
        {
            reasons.push(json!("stable_component_missing_documentation"));
        }
        let warn = reasons
            .iter()
            .any(|code| str_of(code).starts_with("stable_component_"));
        contracts.push(json!({
            "scope_key": scope,
            "status": if warn { "warn" } else { "ok" },
            "instability": profile.get("instability").cloned().unwrap_or(Value::Null),
            "ca": profile.get("ca").cloned().unwrap_or(Value::Null),
            "ce": profile.get("ce").cloned().unwrap_or(Value::Null),
            "stable": profile.get("stable").cloned().unwrap_or(Value::Null),
            "should_be_stable": profile.get("should_be_stable").cloned().unwrap_or(Value::Null),
            "expected_test_density": expected_tests,
            "observed_direct_test_density": observed_tests,
            "observed_heuristic_test_density": supplemental_density("heuristic_test_density"),
            "observed_transitive_test_density": supplemental_density("transitive_test_density"),
            "supplemental_test_density_evaluated": supplemental,
            "expected_doc_density": expected_docs,
            "observed_documentation_density": observed_docs,
            "changed_production_node_count": qns.len(),
            "changed_nodes_with_recommended_tests": with_tests.len(),
            "changed_nodes_with_docs": with_docs.len(),
            "missing_changed_tests": &missing_tests[..missing_tests.len().min(5)],
            "missing_changed_docs": &missing_docs[..missing_docs.len().min(5)],
            "reason_codes": reasons,
        }));
    }
    contracts.sort_by(|left, right| {
        let warn = |v: &Value| if v["status"] == "warn" { 0 } else { 1 };
        warn(left)
            .cmp(&warn(right))
            .then_with(|| {
                float_or(&left["instability"], 1.0)
                    .partial_cmp(&float_or(&right["instability"], 1.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| str_of(&left["scope_key"]).cmp(str_of(&right["scope_key"])))
    });
    contracts.truncate(10);
    contracts
}

/// `_hotspot_proximity`: the persisted code-scope rankings, or Python's
/// on-demand ones where none are persisted.
fn hotspot_proximity(store: &GraphStore, impact: &ImpactRadius) -> Option<Value> {
    let graph = crate::analysis::Graph::read(store)?;
    let hubs = crate::analysis::find_hubs(store, &graph, 25, Artifact::Code, false);
    let bridges = crate::analysis::find_bridges(store, &graph, 25, Artifact::Code, false);
    let qns = |nodes: &[GraphNode]| -> HashSet<String> {
        nodes
            .iter()
            .filter(|n| !n.qualified_name.is_empty())
            .map(|n| n.qualified_name.clone())
            .collect()
    };
    let (changed, impacted) = (qns(&impact.changed_nodes), qns(&impact.impacted_nodes));
    let matches = |items: &[Value], wanted: &HashSet<String>| -> Vec<Value> {
        items
            .iter()
            .filter(|item| {
                item["qualified_name"]
                    .as_str()
                    .is_some_and(|q| wanted.contains(q))
            })
            .take(5)
            .cloned()
            .collect()
    };
    Some(json!({
        "changed_hubs": matches(&hubs, &changed),
        "changed_bridges": matches(&bridges, &changed),
        "impacted_hubs": matches(&hubs, &impacted),
        "impacted_bridges": matches(&bridges, &impacted),
        "method": {"hub": "top 25 by total degree", "bridge": "top 25 by betweenness centrality"},
    }))
}

/// `_cross_artifact_proximity`.
fn cross_artifact_proximity(
    store: &GraphStore,
    impact: &ImpactRadius,
    changed: &[Value],
) -> Option<Value> {
    let mut seeds: HashSet<String> = changed
        .iter()
        .filter_map(|f| f["qualified_name"].as_str().map(str::to_string))
        .collect();
    seeds.extend(
        impact
            .changed_nodes
            .iter()
            .filter(|n| !n.qualified_name.is_empty())
            .map(|n| n.qualified_name.clone()),
    );
    seeds.remove("");
    let follow = [
        "query_graph_tool pattern=\"docs_for\"",
        "query_graph_tool pattern=\"implementations_of\"",
    ];
    if seeds.is_empty() {
        return Some(json!({
            "reportable_bridges": [],
            "low_confidence_bridges": [],
            "follow_ups": follow,
            "counts": {"reportable": 0, "low_confidence": 0},
        }));
    }
    let mut seeds: Vec<String> = seeds.into_iter().collect();
    seeds.sort();
    let (outgoing, incoming) = store.get_edges_by_endpoints(&seeds).ok()?;
    let mut seen = HashSet::new();
    let (mut reportable, mut low) = (Vec::new(), Vec::new());
    for map in [&outgoing, &incoming] {
        for edges in map.values() {
            for edge in edges {
                if edge.kind != "CROSS_ARTIFACT" {
                    continue;
                }
                if !seen.insert((edge.source_qualified.clone(), edge.target_qualified.clone())) {
                    continue;
                }
                if is_reportable_bridge(edge) {
                    reportable.push(bridge_transition_value(edge));
                } else if is_low_confidence_bridge(edge) {
                    low.push(bridge_transition_value(edge));
                }
            }
        }
    }
    let text = |v: &Value, k: &str| match &v[k] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let line = |v: &Value| v["line"].as_i64().unwrap_or(0);
    reportable.sort_by(|a, b| {
        float_or(&b["confidence"], 0.0)
            .partial_cmp(&float_or(&a["confidence"], 0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| text(a, "relationship_role").cmp(&text(b, "relationship_role")))
            .then_with(|| text(a, "source").cmp(&text(b, "source")))
            .then_with(|| text(a, "target").cmp(&text(b, "target")))
            .then_with(|| line(a).cmp(&line(b)))
    });
    low.sort_by(|a, b| {
        text(a, "relationship_role")
            .cmp(&text(b, "relationship_role"))
            .then_with(|| text(a, "source").cmp(&text(b, "source")))
            .then_with(|| text(a, "target").cmp(&text(b, "target")))
            .then_with(|| line(a).cmp(&line(b)))
    });
    let counts = json!({"reportable": reportable.len(), "low_confidence": low.len()});
    reportable.truncate(8);
    low.truncate(8);
    Some(json!({
        "reportable_bridges": reportable,
        "low_confidence_bridges": low,
        "follow_ups": [follow[0], follow[1], "follow CROSS_ARTIFACT bridge edges from changed nodes"],
        "counts": counts,
    }))
}

/// `_changed_scope_keys`.
fn changed_scope_keys(changed_files: &[String]) -> HashSet<String> {
    let mut scopes = HashSet::new();
    for file in changed_files {
        let normalized = file.replace('\\', "/");
        let normalized = normalized.trim_start_matches('/');
        if normalized.is_empty() {
            continue;
        }
        let parts: Vec<&str> = normalized.split('/').collect();
        scopes.insert(if parts.len() == 1 {
            parts[0].to_string()
        } else {
            parts[..2].join("/")
        });
        if parts.len() >= 3 {
            scopes.insert(parts[..3].join("/"));
        }
    }
    scopes
}

/// `_architecture_delta_summary`.
fn architecture_delta(
    graph: &ScopeGraph,
    sap: &[Value],
    changed_files: &[String],
) -> Option<Value> {
    let scopes = changed_scope_keys(changed_files);
    if scopes.is_empty() {
        return Some(json!({
            "mode": "current_graph_changed_scope",
            "changed_scopes": [],
            "related_violations": {},
            "counts": {},
            "note": "No changed scopes were available for architecture filtering.",
        }));
    }
    let touches = |value: &str| {
        let normalized = value.replace('\\', "/");
        scopes
            .iter()
            .any(|scope| normalized == *scope || normalized.starts_with(&format!("{scope}/")))
    };
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    };
    let adp: Vec<Value> = graph
        .adp_violations(2, 10, View::review().profile)?
        .into_iter()
        .filter(|v| {
            v["nodes"]
                .as_array()
                .is_some_and(|n| n.iter().any(|x| touches(&text(x))))
        })
        .take(5)
        .collect();
    let sdp: Vec<Value> = graph
        .sdp_violations(0.1, View::review().profile)
        .into_iter()
        .filter(|v| touches(&text(&v["source"])) || touches(&text(&v["target"])))
        .take(5)
        .collect();
    let sap: Vec<Value> = sap_violations(sap, 0.5)
        .into_iter()
        .filter(|v| touches(&text(&v["scope_key"])) || touches(&text(&v["display_name"])))
        .take(5)
        .collect();
    let mut sorted: Vec<String> = scopes.into_iter().collect();
    sorted.sort();
    Some(json!({
        "mode": "current_graph_changed_scope",
        "baseline_comparison": {
            "available": false,
            "reason": "review currently compares changed scopes against the current graph; a separate base graph is not materialized for every review call.",
        },
        "changed_scopes": sorted,
        "related_violations": {"adp": adp, "sdp": sdp, "sap": sap},
        "counts": {"adp": adp.len(), "sdp": sdp.len(), "sap": sap.len()},
        "note": "This summarizes current graph violations that touch changed scopes; it does not build a separate baseline graph.",
    }))
}

fn classify_gap(gap: &Value) -> &'static str {
    let file = str_of(&gap["file"]);
    if is_markdown_path(file) || gap["language"] == "markdown" {
        return "documentation";
    }
    let normalized = file.replace('\\', "/");
    if normalized.starts_with("tests/") || normalized.contains("/tests/") || gap["kind"] == "Test" {
        return "test_artifact";
    }
    "actionable"
}

/// `_rank_test_gaps`.
fn rank_test_gaps(gaps: &[Value]) -> Value {
    let mut buckets: [(&str, Vec<&Value>); 3] = [
        ("actionable", Vec::new()),
        ("documentation", Vec::new()),
        ("test_artifact", Vec::new()),
    ];
    for gap in gaps {
        let bucket = classify_gap(gap);
        if let Some((_, items)) = buckets.iter_mut().find(|(name, _)| *name == bucket) {
            items.push(gap);
        }
    }
    json!({
        "top_actionable": buckets[0].1.iter().take(5).copied().cloned().collect::<Vec<_>>(),
        "counts": {
            "actionable": buckets[0].1.len(),
            "documentation": buckets[1].1.len(),
            "test_artifact": buckets[2].1.len(),
        },
        "note": "actionable gaps are production-code nodes without direct or credible heuristic test evidence; documentation and test artifacts are separated to reduce review noise.",
    })
}

fn risk_level(score: f64) -> &'static str {
    if score >= 0.7 {
        "high"
    } else if score >= 0.35 {
        "medium"
    } else {
        "low"
    }
}

/// `make_guidance_item`, sealed: an evidence or missingness record drops its
/// `None` fields and an evidence `type` Python does not know becomes
/// `computed`.
pub(crate) fn guidance_item(
    claim: String,
    evidence: Value,
    confidence: &str,
    missingness: Vec<Value>,
    action: &str,
    reason_codes: Vec<Value>,
    counts: Value,
) -> Value {
    let strip = |item: Value| match item {
        Value::Object(map) => {
            Value::Object(map.into_iter().filter(|(_, v)| !v.is_null()).collect())
        }
        other => other,
    };
    let evidence: Vec<Value> = match evidence {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        other => vec![other],
    }
    .into_iter()
    .map(|item| {
        let mut item = strip(item);
        if let Some(object) = item.as_object_mut() {
            let kind = object.get("type").map(truthy_type).unwrap_or("computed");
            object.insert("type".into(), json!(kind));
        }
        item
    })
    .collect();
    let missingness: Vec<Value> = missingness
        .into_iter()
        .map(|item| {
            let mut item = strip(item);
            if let Some(object) = item.as_object_mut() {
                let severity = object.get("severity").cloned().unwrap_or(json!("low"));
                let severity = match severity.as_str() {
                    Some(s @ ("low" | "medium" | "high")) => s.to_string(),
                    _ => "low".to_string(),
                };
                object.insert("severity".into(), json!(severity));
            }
            item
        })
        .collect();
    json!({
        "claim": claim,
        "evidence": evidence,
        "confidence": confidence,
        "missingness": missingness,
        "action": action,
        "reason_codes": reason_codes,
        "counts": counts,
    })
}

/// `GuidanceEvidence.normalize_type`.
fn truthy_type(value: &Value) -> &'static str {
    let text = if truthy(value) {
        match value {
            Value::String(s) => s.as_str(),
            _ => "",
        }
    } else {
        "computed"
    };
    match text {
        "extracted" => "extracted",
        "authored" => "authored",
        "evaluated" => "evaluated",
        _ => "computed",
    }
}

/// `_review_guidance_items`.
#[allow(clippy::too_many_arguments)]
fn review_guidance(
    risk: &str,
    risk_score: f64,
    reason_codes: &[&str],
    tests: &[Value],
    docs: &[Value],
    gap_ranking: &Value,
    contracts: &[Value],
    flow_rankings: &[Value],
    hotspots: &Value,
    delta: &Value,
    signal: &Value,
    proximity: &Value,
) -> Vec<Value> {
    let mut guidance = Vec::new();
    let actionable = gap_ranking["counts"]["actionable"].as_i64().unwrap_or(0);
    if actionable > 0 || !tests.is_empty() {
        let evidence: Vec<Value> = tests
            .iter()
            .take(5)
            .map(|item| {
                let graph = str_of(&item["evidence_level"]).starts_with("graph");
                json!({
                    "type": if graph { "computed" } else { "evaluated" },
                    "source": item.get("source").cloned().unwrap_or(Value::Null),
                    "target": item.get("qualified_name").cloned().unwrap_or(Value::Null),
                    "score": item.get("score").cloned().unwrap_or(Value::Null),
                    "evidence_level": item.get("evidence_level").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        guidance.push(guidance_item(
            if actionable > 0 {
                format!("{actionable} production change(s) need focused test attention.")
            } else {
                "Graph-linked tests are available for the changed code.".to_string()
            },
            Value::Array(evidence),
            if actionable > 0 { "medium" } else { "high" },
            if actionable > 0 {
                vec![json!({
                    "reason_code": "heuristic_test_gap_detection",
                    "severity": "low",
                    "claim_effect": "test recommendations may miss naming-only coverage",
                })]
            } else {
                Vec::new()
            },
            "review_tool mode=\"context\" -- inspect changed nodes and run focused tests",
            vec![json!(if actionable > 0 {
                "test_gaps"
            } else {
                "recommended_tests"
            })],
            json!({"actionable_test_gap_count": actionable, "recommended_test_count": tests.len()}),
        ));
    }
    if let Some(top) = docs.first() {
        let evidence: Vec<Value> = docs
            .iter()
            .take(5)
            .map(|item| {
                json!({
                    "type": item.get("evidence_type").cloned().unwrap_or(json!("computed")),
                    "file": item.get("file").cloned().unwrap_or(Value::Null),
                    "section": item.get("section").cloned().unwrap_or(Value::Null),
                    "relationship_role": item.get("relationship_role").cloned().unwrap_or(Value::Null),
                    "evidence_level": item.get("evidence_level").cloned().unwrap_or(Value::Null),
                    "score": item.get("score").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        let missing: Vec<Value> = docs
            .iter()
            .flat_map(|item| item["missingness"].as_array().cloned().unwrap_or_default())
            .take(5)
            .collect();
        guidance.push(guidance_item(
            "Documentation or contract evidence is connected to this change.".to_string(),
            Value::Array(evidence),
            if top["evidence_type"] == "authored" { "high" } else { "low" },
            missing,
            "query_graph_tool pattern=\"docs_for\" -- inspect linked contract docs; also try pattern=\"implementations_of\" for inverse follow-ups",
            vec![json!("documentation_update_candidates")],
            json!({"documentation_candidate_count": docs.len()}),
        ));
    }
    let reportable = proximity["reportable_bridges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let low = proximity["low_confidence_bridges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !reportable.is_empty() {
        guidance.push(guidance_item(
            "Cross-artifact bridges are near this change and should be followed.".to_string(),
            json!({
                "type": "extracted",
                "bridges": &reportable[..reportable.len().min(5)],
                "follow_ups": proximity.get("follow_ups").cloned().unwrap_or(json!([])),
            }),
            "high",
            vec![json!({
                "reason_code": "cross_artifact_bridge_is_static_evidence",
                "severity": "low",
                "claim_effect": "bridge proximity is structural; confirm with docs_for / implementations_of or source reads",
            })],
            "query_graph_tool pattern=\"docs_for\" -- follow docs; pattern=\"implementations_of\" -- follow implementations; inspect CROSS_ARTIFACT neighbors",
            vec![json!("cross_artifact_proximity")],
            json!({"reportable_bridge_count": reportable.len(), "low_confidence_bridge_count": low.len()}),
        ));
    } else if !low.is_empty() {
        guidance.push(guidance_item(
            "Low-confidence cross-artifact bridges are caveats near this change.".to_string(),
            json!({"type": "extracted", "bridges": &low[..low.len().min(5)]}),
            "low",
            low.iter()
                .take(3)
                .map(|item| {
                    json!({
                        "reason_code": "low_confidence_cross_artifact_bridge",
                        "severity": "medium",
                        "claim_effect": "do not treat the other side as confirmed impact without verification",
                        "bridge": item,
                    })
                })
                .collect(),
            "query_graph_tool pattern=\"docs_for\" -- verify before treating as hard impact",
            vec![json!("low_confidence_cross_artifact_bridge")],
            json!({"low_confidence_bridge_count": low.len()}),
        ));
    }
    let warn: Vec<&Value> = contracts.iter().filter(|c| c["status"] == "warn").collect();
    if !warn.is_empty() {
        let field = |item: &Value, key: &str| item.get(key).cloned().unwrap_or(Value::Null);
        let evidence: Vec<Value> = warn
            .iter()
            .take(3)
            .map(|item| {
                json!({
                    "type": "computed",
                    "scope_key": field(item, "scope_key"),
                    "instability": field(item, "instability"),
                    "expected_test_density": field(item, "expected_test_density"),
                    "observed_direct_test_density": field(item, "observed_direct_test_density"),
                    "observed_heuristic_test_density": field(item, "observed_heuristic_test_density"),
                    "observed_transitive_test_density": field(item, "observed_transitive_test_density"),
                    "expected_doc_density": field(item, "expected_doc_density"),
                    "observed_documentation_density": field(item, "observed_documentation_density"),
                })
            })
            .collect();
        guidance.push(guidance_item(
            "A stable or should-be-stable component has a quality-policy gap.".to_string(),
            Value::Array(evidence),
            "medium",
            vec![json!({
                "reason_code": "stability_policy_uses_current_graph",
                "severity": "low",
                "claim_effect": "policy is calibrated by current SDP/SAP graph metrics",
            })],
            "architecture_analysis_tool mode=\"overview\" -- inspect stable component policy",
            vec![json!("stable_component_contract_gap")],
            json!({"stable_contract_warning_count": warn.len()}),
        ));
    }
    if !flow_rankings.is_empty() {
        let evidence: Vec<Value> = flow_rankings
            .iter()
            .take(3)
            .map(|flow| {
                json!({
                    "type": "computed",
                    "name": flow["name"],
                    "criticality": flow["criticality"],
                    "node_count": flow["node_count"],
                    "file_count": flow["file_count"],
                })
            })
            .collect();
        guidance.push(guidance_item(
            "Changed code intersects ranked execution flows.".to_string(),
            Value::Array(evidence),
            "medium",
            vec![json!({
                "reason_code": "flow_rank_is_not_coverage",
                "severity": "low",
                "claim_effect": "criticality ranks review leads, not sufficient coverage",
            })],
            "review_tool mode=\"affected_flows\" -- inspect affected flow paths",
            vec![json!("affected_flows")],
            json!({"affected_flow_count": flow_rankings.len()}),
        ));
    }
    let counts = delta.get("counts").cloned().unwrap_or(json!({}));
    if counts.as_object().is_some_and(|c| c.values().any(truthy)) {
        guidance.push(guidance_item(
            "Current architecture violations touch changed scopes.".to_string(),
            json!({
                "type": "computed",
                "changed_scopes": delta.get("changed_scopes").cloned().unwrap_or(json!([])),
                "counts": counts,
                "baseline_comparison": delta.get("baseline_comparison").cloned().unwrap_or(Value::Null),
            }),
            "medium",
            vec![json!({
                "reason_code": "current_graph_not_baseline_delta",
                "severity": "low",
                "claim_effect": "violation is scoped to current graph, not a new-introduced proof",
            })],
            "architecture_analysis_tool mode=\"overview\" -- inspect scoped risks",
            vec![json!("architecture_violation_in_changed_scope")],
            counts.clone(),
        ));
    }
    let hotspot_count: usize = [
        "changed_hubs",
        "changed_bridges",
        "impacted_hubs",
        "impacted_bridges",
    ]
    .iter()
    .map(|key| hotspots[*key].as_array().map_or(0, Vec::len))
    .sum();
    if hotspot_count > 0 {
        guidance.push(guidance_item(
            "The change is near graph hub or bridge nodes.".to_string(),
            json!({
                "type": "computed",
                "method": hotspots.get("method").cloned().unwrap_or(Value::Null),
                "changed_hubs": hotspots["changed_hubs"],
                "changed_bridges": hotspots["changed_bridges"],
                "impacted_hubs": hotspots["impacted_hubs"],
                "impacted_bridges": hotspots["impacted_bridges"],
            }),
            "medium",
            Vec::new(),
            "review_tool mode=\"impact\" -- inspect blast radius around hotspots",
            vec![json!("changed_hotspot"), json!("impacted_hotspot")],
            json!({"hotspot_match_count": hotspot_count}),
        ));
    }
    if matches!(risk, "medium" | "high") && guidance.is_empty() {
        guidance.push(guidance_item(
            format!("Review priority is {risk} by graph impact score."),
            json!({
                "type": "computed",
                "metric": "review_priority_score",
                "legacy_metric": "risk_score",
                "value": risk_score,
            }),
            "medium",
            Vec::new(),
            "review_tool mode=\"context\" -- inspect changed nodes before merging",
            reason_codes.iter().map(|code| json!(code)).collect(),
            json!({"graph_fact_count": signal["graph_facts"].as_array().map_or(0, Vec::len)}),
        ));
    }
    guidance
}

/// `_review_signal_quality`.
fn signal_quality(
    reason_codes: &[&str],
    docs: &[Value],
    gaps: &[Value],
    contracts: &[Value],
) -> Value {
    let mut uncertain = Vec::new();
    if docs
        .iter()
        .any(|doc| doc["evidence_level"] == "heuristic_reachable")
    {
        uncertain.push("documentation candidates are graph-reachable markdown nodes");
    }
    if !gaps.is_empty() {
        uncertain.push(
            "test gaps use direct TESTED_BY edges plus medium-confidence naming/source heuristics",
        );
    }
    if !contracts.is_empty() {
        uncertain.push("stable-component density compares current graph evidence to package-level SDP/SAP thresholds");
    }
    let facts = [
        "affected_flows",
        "critical_flow_affected",
        "wide_blast_radius",
        "changed_hotspot",
        "impacted_hotspot",
        "architecture_violation_in_changed_scope",
        "stable_component_contract_gap",
        "cross_artifact_proximity",
    ];
    let heuristics = [
        "test_gaps",
        "documentation_update_candidates",
        "stable_density_gap",
        "low_confidence_cross_artifact_bridge",
    ];
    json!({
        "graph_facts": reason_codes.iter().filter(|c| facts.contains(c)).collect::<Vec<_>>(),
        "heuristics": reason_codes.iter().filter(|c| heuristics.contains(c)).collect::<Vec<_>>(),
        "uncertain": uncertain,
    })
}

/// `_change_analysis_summary` for an analysis whose fields are `analysis`;
/// `verbose` is `detail_level="verbose"`.
pub(crate) fn change_analysis_summary(
    store: &GraphStore,
    analysis: &crate::changes::Analysis,
    impact: &ImpactRadius,
    changed_files: &[String],
    verbose: bool,
) -> Option<Value> {
    let risk_score = float_or(analysis.get("risk_score"), 0.0);
    let empty = Vec::new();
    let flows = analysis.get("affected_flows").as_array().unwrap_or(&empty);
    let gaps = analysis.get("test_gaps").as_array().unwrap_or(&empty);
    let changed = analysis
        .get("changed_functions")
        .as_array()
        .unwrap_or(&empty);
    let risk = risk_level(risk_score);

    let snapshot = Snapshot::read(store)?;
    let view = View::review();
    let graph = ScopeGraph::new(&snapshot.dependencies(&view));
    let sap = sap_metrics(&snapshot, &view, "package", None);
    let profiles = stability_profiles(&graph, &sap);
    let scopes: HashSet<String> = changed.iter().filter_map(scope_key_for_record).collect();
    let nodes = store.get_all_nodes_filtered(true).ok()?;
    let mut scan = None;
    let density = component_density(store, &mut scan, &nodes, &scopes, verbose)?;
    let tests = recommend_tests(store, &mut scan, changed, flows, &profiles)?;
    let docs = documentation_candidates(store, impact, changed, changed_files, &profiles, verbose)?;
    let hotspots = hotspot_proximity(store, impact)?;
    let proximity = cross_artifact_proximity(store, impact, changed)?;
    let delta = architecture_delta(&graph, &sap, changed_files)?;
    let gap_ranking = rank_test_gaps(gaps);
    let contracts = stability_contracts(changed, &tests, &docs, gaps, &profiles, &density);

    let mut reason_codes: Vec<&str> = Vec::new();
    for code in analysis.get("attribution")["reason_codes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(code) = code.as_str()
            && !reason_codes.contains(&code)
        {
            reason_codes.push(code);
        }
    }
    if truthy(analysis.get("unmapped_changed_files"))
        && !reason_codes.contains(&"unmapped_changed_files")
    {
        reason_codes.push("unmapped_changed_files");
    }
    match risk {
        "high" => reason_codes.push("high_risk_score"),
        "medium" => reason_codes.push("medium_risk_score"),
        _ => {}
    }
    if changed.len() >= 10 {
        reason_codes.push("many_changed_graph_nodes");
    }
    if !flows.is_empty() {
        reason_codes.push("affected_flows");
    }
    if flows
        .iter()
        .any(|flow| float_or(&flow["criticality"], 0.0) >= 0.5)
    {
        reason_codes.push("critical_flow_affected");
    }
    if !gaps.is_empty() {
        reason_codes.push("test_gaps");
    }
    if impact.impacted_nodes.len() > 20 {
        reason_codes.push("wide_blast_radius");
    }
    if !docs.is_empty() {
        reason_codes.push("documentation_update_candidates");
    }
    if truthy(&proximity["counts"]["reportable"]) {
        reason_codes.push("cross_artifact_proximity");
    }
    if truthy(&proximity["counts"]["low_confidence"]) {
        reason_codes.push("low_confidence_cross_artifact_bridge");
    }
    let has = |key: &str| truthy(&hotspots[key]);
    if has("changed_hubs") || has("changed_bridges") {
        reason_codes.push("changed_hotspot");
    } else if has("impacted_hubs") || has("impacted_bridges") {
        reason_codes.push("impacted_hotspot");
    }
    if delta["counts"]
        .as_object()
        .is_some_and(|c| c.values().any(truthy))
    {
        reason_codes.push("architecture_violation_in_changed_scope");
    }
    if contracts.iter().any(|c| c["status"] == "warn") {
        reason_codes.push("stable_component_contract_gap");
    }
    if contracts.iter().any(|c| {
        c["reason_codes"].as_array().is_some_and(|codes| {
            codes
                .iter()
                .any(|code| code == "stable_component_low_test_density")
        })
    }) {
        reason_codes.push("stable_density_gap");
    }

    let mut ranked: Vec<&Value> = flows.iter().collect();
    ranked.sort_by(|a, b| {
        float_or(&b["criticality"], 0.0)
            .partial_cmp(&float_or(&a["criticality"], 0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let flow_rankings: Vec<Value> = ranked
        .into_iter()
        .take(10)
        .map(|flow| {
            json!({
                "name": flow.get("name").cloned().unwrap_or(Value::Null),
                "criticality": flow.get("criticality").cloned().unwrap_or(json!(0.0)),
                "node_count": flow.get("node_count").cloned().unwrap_or(Value::Null),
                "file_count": flow.get("file_count").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    let signal = signal_quality(&reason_codes, &docs, gaps, &contracts);
    let guidance = review_guidance(
        risk,
        risk_score,
        &reason_codes,
        &tests,
        &docs,
        &gap_ranking,
        &contracts,
        &flow_rankings,
        &hotspots,
        &delta,
        &signal,
        &proximity,
    );
    Some(json!({
        "risk_level": risk,
        "risk_score": risk_score,
        "review_priority_score": risk_score,
        "score_semantics": {
            "risk_score": "legacy alias for review_priority_score",
            "review_priority_score": "review triage ranking, not a standalone changeability metric",
        },
        "changed_node_count": impact.changed_nodes.len(),
        "impacted_node_count": impact.impacted_nodes.len(),
        "impacted_file_count": impact.impacted_files.len(),
        "reason_codes": reason_codes,
        "recommended_tests": tests,
        "affected_flow_rankings": flow_rankings,
        "documentation_update_candidates": docs,
        "cross_artifact_proximity": proximity,
        "test_gap_ranking": gap_ranking,
        "stability_contracts": contracts,
        "signal_quality": signal,
        "guidance": guidance,
        "hotspot_proximity": hotspots,
        "architecture_delta": delta,
        "next_drill_downs": {
            "impact_radius": {"tool": "review_tool", "mode": "impact"},
            "flows": {"tool": "review_tool", "mode": "affected_flows"},
            "review_context": {"tool": "review_tool", "mode": "context"},
            "architecture": {"tool": "architecture_analysis_tool", "mode": "overview"},
            "docs_for": {"tool": "query_graph_tool", "pattern": "docs_for"},
            "implementations_of": {"tool": "query_graph_tool", "pattern": "implementations_of"},
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_follow_pathlib() {
        use crate::architecture::{file_to_package, suffix};
        assert_eq!(file_to_package("a/b/c.py"), "a/b");
        assert_eq!(file_to_package("c.py"), "<root>");
        assert_eq!(file_to_package("/r/a.py"), "/r");
        let scopes = changed_scope_keys(&["a/b/c/d.py".into(), "x.py".into(), "/y/z.py".into()]);
        let mut scopes: Vec<String> = scopes.into_iter().collect();
        scopes.sort();
        assert_eq!(scopes, vec!["a/b", "a/b/c", "x.py", "y/z.py"]);
        assert_eq!(suffix(".hidden"), "");
        assert_eq!(suffix("README.MD"), ".MD");
    }

    #[test]
    fn adp_lists_each_bounded_cycle_once_from_its_smallest_scope() {
        let deps: Vec<(String, String)> =
            [("b", "a"), ("a", "b"), ("b", "c"), ("c", "a"), ("a", "b")]
                .iter()
                .map(|(s, t)| (s.to_string(), t.to_string()))
                .collect();
        let graph = ScopeGraph::new(&deps);
        let cycles = graph
            .adp_violations(2, 10, crate::architecture::Profile::StrictStatic)
            .expect("cycles");
        let nodes: Vec<Value> = cycles.iter().map(|c| c["nodes"].clone()).collect();
        assert_eq!(nodes, vec![json!(["a", "b", "c"]), json!(["a", "b"])]);
        // a->b carries weight 2: severity 3 * (2 + 1 + 1) beats 2 * (2 + 1).
        assert_eq!(cycles[0]["severity"], 12);
        assert_eq!(cycles[1]["edge_weight"], 3);
        assert!(
            graph
                .sdp_violations(0.1, View::review().profile)
                .iter()
                .all(|v| v["delta"].as_f64() > Some(0.1))
        );
    }

    #[test]
    fn guidance_items_seal_as_pydantic_dumps_them() {
        let item = guidance_item(
            "c".into(),
            json!([{"type": "heuristic_reachable", "file": null, "score": 1}]),
            "low",
            vec![
                json!({"reason_code": "r", "severity": "info", "claim_effect": null, "bridge": {"a": null}}),
            ],
            "a",
            vec![],
            json!({"k": null}),
        );
        assert_eq!(item["evidence"], json!([{"type": "computed", "score": 1}]));
        assert_eq!(
            item["missingness"],
            json!([{"reason_code": "r", "severity": "low", "bridge": {"a": null}}])
        );
        assert_eq!(item["counts"], json!({"k": null}));
    }
}
