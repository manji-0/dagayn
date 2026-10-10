//! What the review summary used to share with other tools: each scope's
//! stability profile (`dagayn.stability_policy`) and the `guidance` item
//! shape.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::architecture::{ScopeGraph, float_or, str_of, truthy};
use crate::suggestions::round_to;

const STABLE_INSTABILITY_MAX: f64 = 0.35;
const SHOULD_BE_STABLE_CA_MIN: i64 = 3;
const STABLE_TEST_DENSITY_TARGET: f64 = 0.8;
const STABLE_DOC_DENSITY_TARGET: f64 = 0.5;
const DEFAULT_TEST_DENSITY_TARGET: f64 = 0.5;
const DEFAULT_DOC_DENSITY_TARGET: f64 = 0.25;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::architecture::View;

    #[test]
    fn scopes_follow_pathlib() {
        use crate::architecture::{file_to_package, suffix};
        assert_eq!(file_to_package("a/b/c.py"), "a/b");
        assert_eq!(file_to_package("c.py"), "<root>");
        assert_eq!(file_to_package("/r/a.py"), "/r");
        assert_eq!(suffix(".hidden"), "");
        assert_eq!(suffix("README.MD"), ".MD");
    }

    #[test]
    fn sdp_violations_exceed_the_minimum_delta() {
        let deps: Vec<(String, String)> =
            [("b", "a"), ("a", "b"), ("b", "c"), ("c", "a"), ("a", "b")]
                .iter()
                .map(|(s, t)| (s.to_string(), t.to_string()))
                .collect();
        let graph = ScopeGraph::new(&deps);
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
