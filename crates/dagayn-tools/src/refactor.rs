//! `refactor_tool` (`dagayn.tools.refactor_tools.refactor_func`): the
//! every mode. A `rename` preview goes to the shared pending store
//! ([`crate::pending`]) that the Python `apply_refactor_tool` applies from.

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
    let old_name = args.optional_string("old_name")?;
    let new_name = args.optional_string("new_name")?;
    let kind = args.optional_string("kind")?;
    let file_pattern = args.optional_string("file_pattern")?;
    let limit = args.integer("limit", 50)?;
    // A non-ASCII name is Python's: its `\w` and Rust's identifier check draw
    // the line differently.
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
        if !old.is_ascii() || !new.is_ascii() {
            return None;
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
        let out = rename(store, &answerability, old, new, &exposed)?;
        return Some(out.put("_repo", graph.repo_context()).into_payload());
    }
    let out = if mode == "dead_code" {
        dead_code(store, &answerability, kind, file_pattern, limit)?
    } else {
        suggest(store, &answerability, limit)?
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
    let report = crate::dead_code::dead_code_report(store, kind, file_pattern)?;
    let dead = report.dead;
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

fn suggest(
    store: &dagayn_graph::GraphStore,
    answerability: &Answerability,
    limit: i64,
) -> Option<Ordered> {
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

/// `_is_valid_identifier` for an ASCII name (`re.match` lets `$` sit before a
/// final newline).
fn is_valid_identifier(name: &str) -> bool {
    let name = name.strip_suffix('\n').unwrap_or(name);
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
    let Ok(pattern) = regex::Regex::new(&format!(r"\b{}\b", regex::escape(symbol))) else {
        return false;
    };
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let lines = crate::coverage::splitlines(&text);
        let idx = edge.line - 1;
        if idx >= 0 && (idx as usize) < lines.len() {
            return pattern.is_match(lines[idx as usize]);
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
) -> Option<Ordered> {
    use crate::query::sanitize;
    if !is_valid_identifier(new) {
        let message = format!("new_name is not a valid identifier: {}", python_repr(new));
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

/// `repr(text)` for an ASCII string.
pub(crate) fn python_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
