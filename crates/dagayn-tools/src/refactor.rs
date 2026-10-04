//! `refactor_tool` (`dagayn.tools.refactor_tools.refactor_func`): the
//! `dead_code` and `suggest` modes. `rename`, whose preview
//! `apply_refactor_tool` reads back from Python's pending store, stays
//! Python's.

use serde_json::{Map, Value, json};

use crate::analysis::py_prefix;
use crate::answerability::Answerability;
use crate::{Args, Context, Ordered, Payload, explicit_repo, hints, open_graph};

const DECLARED: &[&str] = &[
    "mode",
    "old_name",
    "new_name",
    "kind",
    "file_pattern",
    "limit",
    "repo_root",
];

pub(crate) fn refactor(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    let mode = match arguments.get("mode") {
        None => "suggest",
        Some(Value::String(mode))
            if matches!(mode.as_str(), "rename" | "dead_code" | "suggest") =>
        {
            mode.as_str()
        }
        Some(_) => return None,
    };
    args.optional_string("old_name")?;
    args.optional_string("new_name")?;
    let kind = args.optional_string("kind")?;
    let file_pattern = args.optional_string("file_pattern")?;
    let limit = args.integer("limit", 50)?;
    if mode == "rename" {
        return None;
    }
    let root = explicit_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let store = &graph.store;
    let stats = store.get_stats().ok()?;
    let answerability = Answerability::recorded(store, &stats)?;
    let out = if mode == "dead_code" {
        dead_code(store, &answerability, kind, file_pattern, limit)?
    } else {
        suggest(store, &answerability, limit)?
    };
    let exposed = |tool: &str| {
        context
            .allowed_tools
            .as_ref()
            .is_none_or(|allowed| allowed.contains(tool))
    };
    // `suggest` takes its hints from its guidance when that names a step.
    let from_guidance = out
        .get("guidance")
        .and_then(Value::as_array)
        .map(|g| crate::review::guidance_actions_to_hints(g));
    let hint = match from_guidance {
        Some(hint) if hint["next_steps"].as_array().is_some_and(|s| !s.is_empty()) => hint,
        _ => hints::generate_hints("refactor", &out.value(), &mut hints::session(), &exposed),
    };
    Some(
        out.put("_hints", hint)
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}

fn dead_code(
    store: &dagayn_graph::GraphStore,
    answerability: &Answerability,
    kind: Option<&str>,
    file_pattern: Option<&str>,
    limit: i64,
) -> Option<Ordered> {
    let dead = crate::dead_code::find_dead_code(store, kind, file_pattern)?;
    let total = dead.len();
    let truncated = total as i64 > limit;
    let mut summary = format!("Found {total} dead code symbol(s).");
    if truncated {
        summary.push_str(&format!(" Showing first {limit}."));
    }
    let mut missingness = answerability.missingness();
    missingness.push(json!({
        "reason_code": "absence_evidence_requires_manual_verification",
        "severity": "medium",
        "claim_effect": "dead-code claims do not cover dynamic runtime references",
    }));
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("dead_code", Value::Array(py_prefix(&dead, limit)))
            .put("total", total)
            .put("truncated", truncated)
            .put(
                "caveats",
                json!(["Dead-code results are graph-backed candidates; verify dynamic dispatch, plugin registration, reflection, and generated entry points before deleting."]),
            )
            .put("answerability", answerability.full())
            .put("missingness", json!(missingness)),
    )
}

/// `_refactor_guidance`.
fn refactor_guidance(suggestions: &[Value]) -> Vec<Value> {
    suggestions
        .iter()
        .take(3)
        .map(|s| {
            let work_pack = s.get("work_pack").cloned().unwrap_or(json!({}));
            let evidence = s.get("evidence").cloned().unwrap_or(json!({}));
            let evidence_type = if crate::architecture::truthy(&evidence) { "computed" } else { "evaluated" };
            let mut missingness = Vec::new();
            if matches!(s["type"].as_str(), Some("remove" | "move")) {
                missingness.push(json!({
                    "reason_code": "dynamic_dispatch_not_proven_absent",
                    "severity": "medium",
                    "claim_effect": "verify runtime registration, generated code, and public APIs",
                }));
            }
            for condition in work_pack["defer_conditions"].as_array().into_iter().flatten().take(3) {
                let text = match condition {
                    Value::String(t) => t.clone(),
                    other => other.to_string(),
                };
                missingness.push(json!({"reason_code": "defer_condition", "severity": "medium", "claim_effect": text}));
            }
            let confidence = match s.get("confidence") {
                Some(Value::String(c)) if matches!(c.as_str(), "high" | "medium" | "low" | "unknown") => c.as_str(),
                _ => "unknown",
            };
            let claim = match s.get("description") {
                Some(Value::String(d)) => d.clone(),
                Some(other) => other.to_string(),
                None => "Review refactor suggestion.".to_string(),
            };
            let mut item = crate::review_summary::guidance_item(
                claim,
                json!({
                    "type": evidence_type,
                    "suggestion_type": s.get("type").cloned().unwrap_or(Value::Null),
                    "symbols": s.get("symbols").cloned().unwrap_or(json!([])),
                    "reason_codes": s.get("reason_codes").cloned().unwrap_or(json!([])),
                    "raw": evidence,
                }),
                confidence,
                missingness,
                "refactor_tool mode=\"suggest\" -- inspect work_pack, then run the verification commands before editing",
                s.get("reason_codes").and_then(Value::as_array).cloned().unwrap_or_default(),
                work_pack.get("blast_radius").cloned().unwrap_or(json!({})),
            );
            let subset: Map<String, Value> = [
                "safe_first_commit",
                "required_tests",
                "documentation_obligations",
                "rollback_path",
                "defer_conditions",
            ]
            .iter()
            .map(|k| (k.to_string(), work_pack.get(*k).cloned().unwrap_or(Value::Null)))
            .collect();
            if let Some(object) = item.as_object_mut() {
                object.insert("work_pack".into(), Value::Object(subset));
            }
            item
        })
        .collect()
}

fn suggest(
    store: &dagayn_graph::GraphStore,
    answerability: &Answerability,
    limit: i64,
) -> Option<Ordered> {
    let mut suggestions = crate::suggestions::suggest_refactorings(store)?;
    let snapshot = crate::architecture::Snapshot::read(store)?;
    let view = crate::architecture::View::review();
    let scopes = crate::architecture::ScopeGraph::new(&snapshot.dependencies(&view));
    let profiles = crate::review_summary::stability_profiles(
        &scopes,
        &crate::architecture::sap_metrics(&snapshot, &view, "package", None),
    );
    crate::suggestions::apply_stability_policy(&mut suggestions, &profiles);
    let total = suggestions.len();
    let truncated = total as i64 > limit;
    let mut counts = Map::new();
    for s in &suggestions {
        let key = s["type"].as_str().unwrap_or("unknown").to_string();
        let count = counts.get(&key).and_then(Value::as_i64).unwrap_or(0) + 1;
        counts.insert(key, json!(count));
    }
    let shown = py_prefix(&suggestions, limit);
    let packs: Vec<Value> = py_prefix(&suggestions, limit.min(5))
        .iter()
        .map(|s| {
            let mut pack = Map::new();
            pack.insert(
                "symbols".into(),
                s.get("symbols").cloned().unwrap_or(json!([])),
            );
            pack.insert("type".into(), s.get("type").cloned().unwrap_or(Value::Null));
            for (k, v) in s["work_pack"].as_object().into_iter().flatten() {
                pack.insert(k.clone(), v.clone());
            }
            Value::Object(pack)
        })
        .collect();
    let mut summary = format!("Generated {total} refactoring suggestion(s).");
    if truncated {
        summary.push_str(&format!(" Showing first {limit}."));
    }
    let guidance = refactor_guidance(&shown);
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("suggestions", Value::Array(shown))
            .put("work_packs", Value::Array(packs))
            .put("guidance", Value::Array(guidance))
            .put("total", total)
            .put("truncated", truncated)
            .put("counts_by_type", Value::Object(counts))
            .put("answerability", answerability.full())
            .put("missingness", json!(answerability.missingness())),
    )
}
