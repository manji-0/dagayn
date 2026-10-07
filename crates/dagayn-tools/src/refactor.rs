//! `refactor_tool` (`dagayn.tools.refactor_tools.refactor_func`), every
//! mode. A `rename` preview goes to the shared pending store
//! ([`crate::pending`]) that `apply_refactor_tool` applies from.

use serde_json::{Map, Value, json};

use crate::analysis::py_prefix;
use crate::answerability::Answerability;
use crate::{Args, Context, Ordered, Payload, hints, open_graph, resolve_repo};

const DECLARED: &[&str] = &[
    "mode",
    "old_name",
    "new_name",
    "kind",
    "file_pattern",
    "limit",
    "detail_level",
    "repo_root",
];

/// Rename edits a `standard` or `minimal` preview lists; the pending store
/// keeps every edit for `apply_refactor_tool`.
const PREVIEW_EDITS: usize = 20;

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
    let old_name = args.optional_string("old_name")?;
    let new_name = args.optional_string("new_name")?;
    let kind = args.optional_string("kind")?;
    let file_pattern = args.optional_string("file_pattern")?;
    let limit = args.integer("limit", 50)?;
    let detail_level = match arguments.get("detail_level") {
        None => "standard",
        Some(Value::String(level))
            if matches!(level.as_str(), "minimal" | "standard" | "verbose") =>
        {
            level.as_str()
        }
        Some(_) => return None,
    };
    let rename_names = if mode == "rename" {
        // `RefactorRenameRequest`: a missing name is not a string, an empty
        // one is shorter than `min_length=1`.
        let problems: Vec<&str> = [old_name, new_name]
            .iter()
            .filter_map(|name| match name {
                None => Some("Input should be a valid string"),
                Some("") => Some("String should have at least 1 character"),
                Some(_) => None,
            })
            .collect();
        let (Some(old), Some(new)) = (old_name, new_name) else {
            return rename_error(context, &args, &problems.join("; "));
        };
        if !problems.is_empty() {
            return rename_error(context, &args, &problems.join("; "));
        }
        Some((old, new))
    } else {
        None
    };
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let store = &graph.store;
    let answerability = graph.answerability()?;
    if let Some((old, new)) = rename_names {
        let exposed = |tool: &str| context.exposes(tool);
        let out = rename(store, &answerability, old, new, &exposed, detail_level)?;
        return Some(out.put("_repo", graph.repo_context()).into_payload());
    }
    let out = if mode == "dead_code" {
        dead_code(store, &answerability, kind, file_pattern, limit)?
    } else {
        suggest(store, &answerability, limit, detail_level)?
    };
    let exposed = |tool: &str| context.exposes(tool);
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
    let mut report = crate::dead_code::dead_code_report(store, kind, file_pattern)?;
    // Samples under fixtures/ and testdata/ are written to be parsed, not
    // called: count them as left out instead of listing them as dead. The
    // suggest mode still ranks them, last.
    let (fixtures, dead): (Vec<Value>, Vec<Value>) = report.dead.into_iter().partition(|item| {
        item["qualified_name"]
            .as_str()
            .is_some_and(|qn| is_fixture_path(qn.split("::").next().unwrap_or(qn)))
    });
    if !fixtures.is_empty() {
        *report.suppressed.entry("test_fixture").or_default() += fixtures.len();
    }
    let total = dead.len();
    let truncated = total as i64 > limit;
    let left_out: usize = report.suppressed.values().sum();
    let mut summary = if report.verification.status == "unavailable" {
        "Could not scan the repository's sources, so no symbol is reported as dead.".to_string()
    } else {
        format!("Found {total} dead code symbol(s) that nothing in the repository refers to.")
    };
    if left_out > 0 {
        summary.push_str(&format!(
            " Left out {left_out} graph candidate(s) that may still be used (see suppressed)."
        ));
    }
    if truncated {
        summary.push_str(&format!(" Showing first {limit}."));
    }
    let mut missingness = answerability.missingness();
    missingness.push(json!({
        "reason_code": "absence_evidence_requires_manual_verification",
        "severity": "medium",
        "claim_effect": "dead-code claims do not cover dynamic runtime references",
    }));
    if report.verification.status != "complete" {
        missingness.push(json!({
            "reason_code": "source_scan_incomplete",
            "severity": "medium",
            "claim_effect": "some repository files were not searched for the reported names",
        }));
    }
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("dead_code", Value::Array(py_prefix(&dead, limit)))
            .put("total", total)
            .put("truncated", truncated)
            .put("suppressed", json!(report.suppressed))
            .put("verification", report.verification.value())
            .put(
                "caveats",
                json!(["Dead-code results are graph-backed candidates; verify dynamic dispatch, plugin registration, reflection, and generated entry points before deleting."]),
            )
            .put("answerability", answerability.full())
            .put("missingness", json!(missingness)),
    )
}

/// `seal_refactor_error(attach_answerability({...}))` for a rename request
/// `RefactorRenameRequest` rejects.
fn rename_error(context: &Context, args: &crate::Args, message: &str) -> Option<Payload> {
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    crate::request_error(
        context,
        &root,
        Ordered::default()
            .put("status", "error")
            .put("error", message)
            .put("summary", message),
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

/// Findings each kind lists before counting the rest in `findings_omitted`.
const FINDINGS_PER_KIND: usize = 10;

/// `unused_symbol` findings: the verified dead-code report without test
/// fixtures (docs/plans/REFACTOR-TOOL-TARGET.md#finding-kinds).
fn unused_symbol_findings(store: &dagayn_graph::GraphStore) -> Option<(Vec<Value>, usize)> {
    let report = crate::dead_code::dead_code_report(store, None, None)?;
    if report.verification.status == "unavailable" {
        return Some((Vec::new(), 0));
    }
    let verification = report.verification.value();
    let found: Vec<Value> = report
        .dead
        .iter()
        .filter(|item| {
            !item["qualified_name"]
                .as_str()
                .is_some_and(|qn| is_fixture_path(qn.split("::").next().unwrap_or(qn)))
        })
        .map(|item| {
            json!({
                "kind": "unused_symbol",
                "qualified_name": item["qualified_name"],
                "file": item["file"],
                "line": item["line"],
                "claim": format!(
                    "Nothing in the repository refers to {}.",
                    item["qualified_name"].as_str().unwrap_or("this symbol")
                ),
                "evidence": {
                    "public_api_candidate": item["public_api_candidate"],
                    "verification": verification,
                },
                "action": "Delete it, or point to the dynamic use the graph missed.",
            })
        })
        .collect();
    let omitted = found.len().saturating_sub(FINDINGS_PER_KIND);
    Some((found.into_iter().take(FINDINGS_PER_KIND).collect(), omitted))
}

fn suggest(
    store: &dagayn_graph::GraphStore,
    answerability: &Answerability,
    limit: i64,
    detail_level: &str,
) -> Option<Ordered> {
    let (unused, unused_omitted) = unused_symbol_findings(store)?;
    let findings_summary = if unused.is_empty() {
        "Findings: nothing worth doing.".to_string()
    } else {
        format!("Findings: {} unused_symbol.", unused.len() + unused_omitted)
    };
    let mut omitted = Map::new();
    if unused_omitted > 0 {
        omitted.insert("unused_symbol".into(), json!(unused_omitted));
    }
    let findings = Value::Array(unused);
    let findings_omitted = Value::Object(omitted);
    let suggestions = crate::suggestions::ranked_suggestions(store)?;
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
    let mut summary = format!("{findings_summary} Generated {total} refactoring suggestion(s).");
    if truncated {
        summary.push_str(&format!(" Showing first {limit}."));
    }
    let mut guidance = refactor_guidance(&shown);
    if detail_level == "verbose" {
        return Some(
            Ordered::default()
                .put("status", "ok")
                .put("summary", summary)
                .put("findings", findings)
                .put("findings_omitted", findings_omitted)
                .put("suggestions", Value::Array(shown))
                .put("work_packs", Value::Array(packs))
                .put("guidance", Value::Array(guidance))
                .put("total", total)
                .put("truncated", truncated)
                .put("counts_by_type", Value::Object(counts))
                .put("answerability", answerability.full())
                .put("missingness", json!(answerability.missingness())),
        );
    }
    // Each type's plan is the same text for every suggestion of that type:
    // state it once.
    let mut plans = Map::new();
    for s in &shown {
        let kind = s["type"].as_str().unwrap_or("unknown");
        if !plans.contains_key(kind) {
            let mut plan = s.get("execution_plan").cloned().unwrap_or(json!({}));
            if let Some(object) = plan.as_object_mut() {
                object.remove("required_tests");
            }
            plans.insert(kind.to_string(), plan);
        }
    }
    let shown: Vec<Value> = shown
        .into_iter()
        .map(|mut s| {
            if let Some(object) = s.as_object_mut() {
                for key in ["execution_plan", "work_pack", "verification_steps"] {
                    object.remove(key);
                }
                if detail_level == "minimal" {
                    object.remove("evidence");
                } else if let Some(evidence) =
                    object.get_mut("evidence").and_then(Value::as_object_mut)
                {
                    evidence.remove("concern_separation");
                }
            }
            s
        })
        .collect();
    // The guidance repeats a suggestion's evidence, which `suggestions`
    // already carries.
    for item in &mut guidance {
        if let Some(evidence) = item.get_mut("evidence").and_then(Value::as_object_mut) {
            evidence.remove("raw");
        }
        if let Some(object) = item.as_object_mut() {
            object.remove("work_pack");
            object.insert(
                "action".into(),
                json!("refactor_tool mode=\"suggest\" -- read the plan for this suggestion's type in plans, then run the verification commands before editing"),
            );
        }
    }
    let mut out = Ordered::default()
        .put("status", "ok")
        .put("summary", summary)
        .put("findings", findings)
        .put("findings_omitted", findings_omitted)
        .put("suggestions", Value::Array(shown))
        .put("plans", Value::Object(plans));
    if detail_level != "minimal" {
        out = out.put("work_packs", Value::Array(packs));
    }
    Some(
        out.put("guidance", Value::Array(guidance))
            .put("total", total)
            .put("truncated", truncated)
            .put("counts_by_type", Value::Object(counts))
            .put("answerability", answerability.full())
            .put("missingness", json!(answerability.missingness())),
    )
}

/// Whether `name` can be substituted into source as an identifier:
/// `re.match(r"^[^\W\d]\w*$", name)`, a leading letter or underscore followed
/// by word characters in Python's Unicode sense (`$` also matches before a
/// final newline). The conservative shape the languages dagayn parses share:
/// language-specific extras (`$` in JS, `!`/`?` in Ruby) are deliberately
/// excluded, since a rejected valid name is a nuisance and an accepted invalid
/// one writes code that does not parse.
fn is_valid_identifier(name: &str) -> bool {
    use crate::pyunicode::{is_digit, is_word};
    let name = name.strip_suffix('\n').unwrap_or(name);
    let mut chars = name.chars();
    chars.next().is_some_and(|c| is_word(c) && !is_digit(c)) && chars.all(is_word)
}

/// `_import_statement_mentions_symbol`.
fn import_mentions(
    store: &dagayn_graph::GraphStore,
    edge: &dagayn_graph::GraphEdge,
    symbol: &str,
) -> bool {
    if edge.target_qualified == symbol || edge.target_qualified.ends_with(&format!("::{symbol}")) {
        return true;
    }
    let file = std::path::Path::new(&edge.file_path);
    let mut paths = Vec::new();
    if file.is_absolute() {
        paths.push(file.to_path_buf());
    }
    if let Ok(Some(root)) = store.get_metadata("repo_root") {
        paths.push(std::path::Path::new(&root).join(file));
    }
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let lines = crate::coverage::splitlines(&text);
        let idx = edge.line - 1;
        if idx >= 0 && (idx as usize) < lines.len() {
            // `\b` with Python's `\w`, which the `regex` crate's differs from.
            return crate::pyunicode::contains_bounded(lines[idx as usize], symbol);
        }
    }
    false
}

/// Eight hex digits, as `uuid.uuid4().hex[:8]` gives.
fn refactor_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!("{:08x}", hasher.finish() as u32)
}

/// `refactor_func(mode="rename")` with `rename_preview`.
fn rename(
    store: &dagayn_graph::GraphStore,
    answerability: &Answerability,
    old: &str,
    new: &str,
    exposed: &dyn Fn(&str) -> bool,
    detail_level: &str,
) -> Option<Ordered> {
    use crate::query::sanitize;
    if !is_valid_identifier(new) {
        let message = format!(
            "new_name is not a valid identifier: {}",
            crate::pyunicode::repr(new)
        );
        return Some(
            Ordered::default()
                .put("status", "error")
                .put("error", message.as_str())
                .put("summary", message.as_str())
                .put("old_name", old)
                .put("new_name", new),
        );
    }
    let candidates = store.search_nodes(old, 10).ok()?;
    let node = candidates
        .iter()
        .find(|c| c.name == old)
        .or_else(|| candidates.first());
    let Some(node) = node.cloned() else {
        let mut missingness = answerability.missingness();
        missingness.push(json!({
            "reason_code": "rename_target_not_found_in_graph",
            "severity": "medium",
            "claim_effect": "absence is graph-limited, not proof the symbol does not exist",
        }));
        return Some(
            Ordered::default()
                .put("status", "not_found")
                .put(
                    "summary",
                    format!("No node found matching '{old}' in the current graph."),
                )
                .put("answerability", answerability.full())
                .put("missingness", json!(missingness)),
        );
    };
    let exact: Vec<&dagayn_graph::GraphNode> =
        candidates.iter().filter(|c| c.name == old).collect();
    let mut edits: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<(String, i64)> = std::collections::HashSet::new();
    let mut add = |file: &str, line: i64, confidence: &str, source: &str, kind: Option<&str>| {
        if !seen.insert((file.to_string(), line)) {
            return;
        }
        let mut edit = json!({"file": file, "line": line, "old": old, "new": new, "confidence": confidence, "source": source});
        if let Some(kind) = kind {
            edit["edge_kind"] = json!(kind);
        }
        edits.push(edit);
    };
    add(&node.file_path, node.line_start, "high", "definition", None);
    for edge in store.get_edges_by_target(&node.qualified_name).ok()? {
        match edge.kind.as_str() {
            "CALLS" => add(&edge.file_path, edge.line, "high", "call", Some("CALLS")),
            "REFERENCES" => add(
                &edge.file_path,
                edge.line,
                "high",
                "reference",
                Some("REFERENCES"),
            ),
            _ => {}
        }
    }
    for edge in store.search_edges_by_target_name(old, "CALLS").ok()? {
        add(
            &edge.file_path,
            edge.line,
            "medium",
            "bare_call",
            Some(&edge.kind),
        );
    }
    for edge in store.search_import_edges_for_symbol(&node.file_path).ok()? {
        if import_mentions(store, &edge, old) {
            add(
                &edge.file_path,
                edge.line,
                "high",
                "import",
                Some(&edge.kind),
            );
        }
    }
    for edge in store
        .search_edges_by_target_name(old, "IMPORTS_FROM")
        .ok()?
    {
        if import_mentions(store, &edge, old) {
            add(
                &edge.file_path,
                edge.line,
                "medium",
                "import",
                Some(&edge.kind),
            );
        }
    }
    let count = |level: &str| edits.iter().filter(|e| e["confidence"] == level).count();
    let stats = json!({"high": count("high"), "medium": count("medium"), "low": count("low")});
    let id = refactor_id();
    let graph_limited = json!({
        "reason_code": "rename_edits_graph_limited",
        "severity": "medium",
        "claim_effect": "edit list covers graph-known call, reference, and import sites only; string-based access, getattr, re-exports, generated code, and non-code files are not included",
    });
    let preview = Ordered::default()
        .put("refactor_id", id.as_str())
        .put("type", "rename")
        .put("old_name", sanitize(old))
        .put("new_name", sanitize(new))
        .put(
            "target",
            json!({
                "name": sanitize(&node.name),
                "qualified_name": sanitize(&node.qualified_name),
                "kind": node.kind,
                "file": node.file_path,
                "line": node.line_start,
                "language": node.language,
            }),
        )
        .put("ambiguous", exact.len() > 1)
        .put("candidate_count", exact.len())
        .put(
            "candidates",
            json!(
                exact
                    .iter()
                    .map(|c| json!({
                        "qualified_name": sanitize(&c.qualified_name),
                        "kind": c.kind,
                        "file": c.file_path,
                        "line": c.line_start,
                        "language": c.language,
                    }))
                    .collect::<Vec<_>>()
            ),
        )
        .put("edits", json!(edits.clone()))
        .put("stats", stats)
        .put("created_at", crate::pending::now())
        .put("missingness", json!([graph_limited.clone()]))
        .put(
            "warnings",
            if exact.len() > 1 {
                json!(["Multiple exact symbol matches were found; preview uses the first match."])
            } else {
                json!([])
            },
        );
    crate::pending::cleanup_expired();
    crate::pending::set(&id, preview.value().to_string());

    let mut missingness = answerability.missingness();
    missingness.push(graph_limited);
    let mut out = Ordered::default().put("status", "ok").put(
        "summary",
        format!(
            "Rename preview: {old} -> {new}, {} edit(s). Apply with apply_refactor_tool in the same `dagayn serve` MCP session (refactor_id is session-scoped, expires after 10 min) using refactor_id='{id}'.",
            edits.len()
        ),
    );
    for (key, value) in preview.into_entries() {
        out = out.put(&key, value);
    }
    if detail_level != "verbose" {
        // Per-file counts and the first edits; the pending store keeps all
        // of them for `apply_refactor_tool`.
        let mut files: Vec<(String, usize)> = Vec::new();
        for edit in &edits {
            let file = edit
                .get("file")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match files.iter_mut().find(|(f, _)| *f == file) {
                Some((_, count)) => *count += 1,
                None => files.push((file, 1)),
            }
        }
        out = out
            .replace(
                "edits",
                json!(edits.iter().take(PREVIEW_EDITS).collect::<Vec<_>>()),
            )
            .put("edits_omitted", edits.len().saturating_sub(PREVIEW_EDITS))
            .put(
                "files",
                json!(
                    files
                        .iter()
                        .map(|(file, count)| json!({"file": file, "edit_count": count}))
                        .collect::<Vec<_>>()
                ),
            );
    }
    let out = out
        .put("answerability", answerability.full())
        .replace("missingness", json!(missingness))
        .put(
            "next_tool_suggestions",
            json!([
                format!("apply_refactor_tool(refactor_id='{id}', dry_run=true) in the same session -- preview unified diff before writing files"),
                format!("apply_refactor_tool(refactor_id='{id}') in the same session -- apply the rename"),
            ]),
        );
    let hint = hints::generate_hints("refactor", &out.value(), &mut hints::session(), exposed);
    Some(out.put("_hints", hint))
}

/// A path under a `fixtures/` or `testdata/` directory.
pub(crate) fn is_fixture_path(path: &str) -> bool {
    path.split(['/', '\\'])
        .rev()
        .skip(1)
        .any(|part| matches!(part, "fixtures" | "testdata"))
}

#[cfg(test)]
mod fixture_tests {
    use super::is_fixture_path;

    #[test]
    fn fixture_paths_are_recognised() {
        assert!(is_fixture_path("tests/fixtures/sample.py"));
        assert!(is_fixture_path("pkg/testdata/in.go"));
        assert!(!is_fixture_path("dagayn/fixture_loader.py"));
        assert!(!is_fixture_path("src/fixtures.rs"));
    }
}
