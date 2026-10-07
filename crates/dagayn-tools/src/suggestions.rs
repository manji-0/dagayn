//! `dagayn.refactor.suggestions.suggest_refactorings` with its function
//! concern profiles (`dagayn.refactor.concerns`), and the stability guard
//! `refactor_tools` applies afterwards.

use std::collections::{HashMap, HashSet};

use dagayn_graph::{GraphEdge, GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use crate::architecture::{float_or, posix_parts, scope_key_for_file, str_of, truthy};
use crate::dead_code::{enclosing_block, external_api_candidate, find_dead_code, source_lines};
use crate::query::sanitize;

const PRODUCTION_LANGUAGES: &[&str] = &[
    "python",
    "rust",
    "javascript",
    "typescript",
    "tsx",
    "java",
    "go",
    "ruby",
    "php",
    "c",
    "cpp",
    "csharp",
    "swift",
    "kotlin",
    "scala",
    "dart",
    "lua",
    "julia",
    "zig",
    "r",
    "elixir",
    "vue",
    "bash",
    "terraform",
];
const AMBIGUOUS_NAMES: &[&str] = &[
    "activate", "build", "create", "execute", "handle", "main", "process", "register", "run",
    "sync", "update",
];
const BOOLEAN_PREFIXES: &[&str] = &[
    "allow_", "dry_", "enable_", "has_", "include_", "is_", "should_", "skip_", "use_", "with_",
];
const BOUNDARY_PARTS: &[&str] = &[
    "/api/",
    "/cli/",
    "/commands/",
    "/handlers/",
    "/routes/",
    "/server/",
    "/tools/",
];
const COORDINATOR_PREFIXES: &[&str] = &[
    "activate",
    "build",
    "configure",
    "dispatch",
    "handle",
    "install",
    "register",
    "run",
    "serve",
    "sync",
    "update",
];
const SIDE_EFFECTS: &[(&str, &[&str])] = &[
    (
        "filesystem_io",
        &[
            ".read_text(",
            ".write_text(",
            " open(",
            "Path(",
            "std::fs",
            "fs::",
            "read_to_string",
            "write_all",
        ],
    ),
    (
        "database_io",
        &[
            ".execute(",
            ".executemany(",
            ".commit(",
            ".rollback(",
            ".transaction(",
            " insert ",
            " update ",
            " delete ",
            " select ",
        ],
    ),
    (
        "network_io",
        &[
            "fetch(",
            "requests.",
            "urlopen(",
            "http://",
            "https://",
            "Client::",
        ],
    ),
    (
        "process_or_environment",
        &[
            "os.environ",
            "process.env",
            "std::env",
            "subprocess",
            "Command::new",
            "exec(",
            "spawn(",
        ],
    ),
    (
        "time_or_random",
        &[
            "datetime(",
            "datetime.",
            "Instant::",
            "random",
            "time.",
            "uuid",
        ],
    ),
    (
        "logging_or_console",
        &[
            "console.",
            "eprintln!",
            "logger.",
            "logging.",
            "println!",
            "tracing::",
        ],
    ),
];

/// Python's `round(value, ndigits)` (correctly rounded, ties to even on the
/// exact binary value, as Rust's formatting rounds).
pub(crate) fn round_to(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// `branch_count`.
fn branch_count(lines: &[String]) -> i64 {
    const TOKENS: &[&str] = &[
        " if ", " elif ", " else ", " for ", " while ", " match ", " case ", " switch ", " catch ",
        " except ", "&&", "||", "?",
    ];
    lines
        .iter()
        .map(|line| {
            let padded = format!(" {} ", line.trim());
            TOKENS.iter().filter(|t| padded.contains(**t)).count() as i64
        })
        .sum()
}

/// `comment_line_count`.
fn comment_line_count(lines: &[String]) -> i64 {
    let (mut count, mut in_block) = (0, false);
    for line in lines {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        if in_block {
            count += 1;
            if stripped.contains("*/") {
                in_block = false;
            }
            continue;
        }
        if ["#", "//", "///", "//!", "\"\"\"", "'''"]
            .iter()
            .any(|p| stripped.starts_with(p))
        {
            count += 1;
            continue;
        }
        if stripped.starts_with("/*") {
            count += 1;
            in_block = !stripped.contains("*/");
        }
    }
    count
}

/// `_parameter_names`.
fn parameter_names(params: Option<&str>) -> Vec<String> {
    let Some(params) = params.filter(|p| !p.is_empty()) else {
        return Vec::new();
    };
    let mut text = params.trim();
    if text.starts_with('(') && text.ends_with(')') && text.len() >= 2 {
        text = &text[1..text.len() - 1];
    }
    let mut names = Vec::new();
    for raw in text.split(',') {
        let mut part = raw.trim().to_string();
        if part.is_empty() {
            continue;
        }
        part = part.split('=').next().unwrap_or("").trim().to_string();
        part = part.split(':').next().unwrap_or("").trim().to_string();
        part = part.replace('&', "").replace("mut ", "").trim().to_string();
        if part == "self" || part == "cls" {
            continue;
        }
        if part.contains(' ') {
            part = part.rsplit(' ').next().unwrap_or("").to_string();
        }
        if !part.is_empty() {
            names.push(part);
        }
    }
    names
}

/// `_callee_scope`: the target's file's parent directory.
fn callee_scope(target: &str) -> Option<String> {
    if target.is_empty() || target.starts_with('<') {
        return None;
    }
    let file = target.split("::").next().unwrap_or(target);
    let (absolute, parts) = posix_parts(file);
    let parent = &parts[..parts.len().saturating_sub(1)];
    Some(match (absolute, parent.is_empty()) {
        (true, _) => format!("/{}", parent.join("/")),
        (false, true) => ".".to_string(),
        (false, false) => parent.join("/"),
    })
}

/// `function_concern_profile`, or `{}` for a non-function.
fn concern_profile(
    node: &GraphNode,
    span: &[String],
    outgoing: &[GraphEdge],
    communities: &HashMap<String, i64>,
    branches: i64,
    comments: i64,
) -> Value {
    if node.kind != "Function" {
        return json!({});
    }
    let calls: Vec<&str> = outgoing
        .iter()
        .filter(|e| e.kind == "CALLS")
        .map(|e| e.target_qualified.as_str())
        .collect();
    let call_count = calls.len() as i64;
    let line_count = (node.line_end - node.line_start + 1).max(0);
    let callee_communities: HashSet<i64> = calls
        .iter()
        .filter_map(|t| communities.get(*t).copied())
        .collect();
    let callee_scopes: HashSet<String> = calls.iter().filter_map(|t| callee_scope(t)).collect();
    let dynamic = calls.iter().filter(|t| t.starts_with('<')).count();
    let haystack = span.join("\n").to_lowercase();
    let targets = calls.join("\n").to_lowercase();
    let side_effects: Vec<&str> = SIDE_EFFECTS
        .iter()
        .filter(|(_, patterns)| {
            patterns.iter().any(|p| {
                let p = p.to_lowercase();
                haystack.contains(&p) || targets.contains(&p)
            })
        })
        .map(|(reason, _)| *reason)
        .collect();
    let side_count = side_effects.len() as i64;
    let params = parameter_names(node.params.as_deref());
    let flags = params
        .iter()
        .filter(|p| {
            let lower = p.to_lowercase();
            BOOLEAN_PREFIXES.iter().any(|b| lower.starts_with(b))
                || lower == "flag"
                || lower == "flags"
        })
        .count() as i64;
    let param_count = params.len() as i64;
    let missing_return = node.return_type.as_deref().is_none_or(str::is_empty);
    let ambiguous = AMBIGUOUS_NAMES.contains(&node.name.to_lowercase().as_str());
    let f = |v: i64| v as f64;
    let responsibility = (((f((callee_communities.len() as i64 - 1).max(0)) / 3.0).min(1.0))
        * 0.35
        + ((f((callee_scopes.len() as i64 - 2).max(0)) / 5.0).min(1.0)) * 0.25
        + ((f(branches) / 12.0).min(1.0)) * 0.2
        + ((f(call_count) / 22.0).min(1.0)) * 0.2)
        .min(1.0);
    let side_pressure = (f(side_count) / 4.0).min(1.0);
    let context = (((f((param_count - 4).max(0)) / 4.0).min(1.0)) * 0.45
        + ((f(flags) / 2.0).min(1.0)) * 0.25
        + if missing_return && line_count >= 60 {
            0.2
        } else {
            0.0
        }
        + if ambiguous { 0.1 } else { 0.0 })
    .min(1.0);
    let mixed = side_count > 0 && (branches >= 12 || call_count >= 22);
    let score = (responsibility * 0.45
        + side_pressure * 0.25
        + context * 0.3
        + if mixed { 0.1 } else { 0.0 })
    .min(1.0);
    let mut reasons = Vec::new();
    if callee_communities.len() >= 3 {
        reasons.push("many_callee_communities");
    }
    if callee_scopes.len() >= 5 {
        reasons.push("many_callee_scopes");
    }
    if branches >= 12 {
        reasons.push("branch_heavy");
    }
    if call_count >= 22 {
        reasons.push("many_collaborators");
    }
    if side_count >= 2 {
        reasons.push("side_effect_pressure");
    }
    if mixed {
        reasons.push("side_effect_mixed_with_decision_logic");
    }
    if param_count >= 6 || flags >= 2 {
        reasons.push("implicit_context");
    }
    if context >= 0.5 {
        reasons.push("low_context_clarity");
    }
    let mut confidence = "medium";
    let mut missingness = Vec::new();
    if span.is_empty() {
        confidence = "low";
        missingness.push("source_unavailable");
    }
    if call_count > 0 && communities.is_empty() {
        confidence = "low";
        missingness.push("community_assignments_unavailable");
    }
    // `_function_role`.
    let path = format!("/{}", node.file_path.replace('\\', "/"));
    let name = node.name.to_lowercase();
    let role = if path.contains("/tests/") || path.contains("/__tests__/") {
        "test_helper"
    } else if BOUNDARY_PARTS.iter().any(|p| path.contains(p)) {
        "boundary"
    } else if COORDINATOR_PREFIXES.iter().any(|p| name.starts_with(p)) {
        "coordinator"
    } else if side_count >= 2 && call_count >= 3 {
        "boundary"
    } else if side_count == 0 && branches <= 4 && call_count <= 6 {
        "transformer"
    } else if side_count == 0 && context < 0.35 {
        "pure_candidate"
    } else {
        "unknown"
    };
    let action = if score >= 0.65 {
        "Extract one cohesive decision or transformation helper before moving IO code."
    } else if context >= 0.5 {
        "Clarify parameters, return contract, or naming before broader refactoring."
    } else if side_pressure >= 0.5 {
        "Isolate side effects from pure decision logic where possible."
    } else {
        "No concern-separation action needed from this profile alone."
    };
    json!({
        "role": role,
        "score": round_to(score, 3),
        "confidence": confidence,
        "reason_codes": reasons,
        "evidence": {
            "line_count": line_count,
            "branch_count": branches,
            "outgoing_call_count": call_count,
            "callee_community_count": callee_communities.len(),
            "callee_scope_count": callee_scopes.len(),
            "dynamic_or_unresolved_call_count": dynamic,
            "side_effect_reason_codes": side_effects,
            "side_effect_count": side_count,
            "parameter_count": param_count,
            "boolean_flag_parameter_count": flags,
            "comment_line_count": comments,
            "missing_return_type": missing_return,
            "ambiguous_name": ambiguous,
            "responsibility_pressure": round_to(responsibility, 3),
            "side_effect_pressure": round_to(side_pressure, 3),
            "context_pressure": round_to(context, 3),
            "purity_likelihood": round_to(1.0 - side_pressure, 3),
            "split_score_threshold": 0.65,
        },
        "missingness": missingness,
        "action": action,
    })
}

/// `_is_test_file_path`.
fn is_test_file_path(file_path: &str) -> bool {
    let normalized = file_path.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or("");
    normalized.contains("/tests/")
        || normalized.contains("/__tests__/")
        || name == "tests.rs"
        || name == "test.rs"
        || name.ends_with("_tests.rs")
        || name.ends_with("_test.rs")
        || name.starts_with("test_")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// `_is_test_artifact` for a record's test flag, file, language, and line.
fn is_test_artifact(
    is_test: bool,
    file: &str,
    language: &str,
    line: Option<i64>,
    lines: &[String],
) -> bool {
    if is_test || is_test_file_path(file) {
        return true;
    }
    match line {
        Some(line) if language == "rust" && line > 0 && !lines.is_empty() => {
            enclosing_block(lines, line, |idx| {
                lines[idx].contains("mod tests")
                    && lines[idx.saturating_sub(3)..=idx]
                        .join("\n")
                        .contains("#[cfg(test)]")
            })
        }
        _ => false,
    }
}

/// `_source_span`.
fn source_span(lines: &[String], start: i64, end: i64) -> Vec<String> {
    if lines.is_empty() || start <= 0 || end < start {
        return Vec::new();
    }
    let from = (start - 1) as usize;
    let to = (end as usize).min(lines.len());
    if from >= to {
        Vec::new()
    } else {
        lines[from..to].to_vec()
    }
}

/// `_split_metrics`.
fn split_metrics(
    kind: &str,
    length: i64,
    branches: i64,
    calls: i64,
    concern: &Value,
) -> Option<Value> {
    let f = |v: i64| v as f64;
    if kind == "Function" {
        let length_ratio = f(length) / 60.0;
        let branch_ratio = f(branches) / 12.0;
        let call_ratio = f(calls) / 22.0;
        let concern_ratio = float_or(&concern["score"], 0.0) / 0.65;
        let secondary = branch_ratio.max(call_ratio).max(concern_ratio);
        if length_ratio < 1.0 || secondary < 1.0 {
            return None;
        }
        let mut reasons: Vec<Value> = vec![json!("large_function")];
        if branch_ratio >= 1.0 {
            reasons.push(json!("branch_heavy"));
        }
        if call_ratio >= 1.0 {
            reasons.push(json!("many_collaborators"));
        }
        if concern_ratio >= 1.0 {
            reasons.push(json!("function_concern_pressure"));
            for reason in concern["reason_codes"].as_array().into_iter().flatten() {
                if !reasons.contains(reason) {
                    reasons.push(reason.clone());
                }
            }
        }
        return Some(json!({
            "line_count": length,
            "branch_count": branches,
            "outgoing_call_count": calls,
            "line_threshold": 60,
            "branch_threshold": 12,
            "outgoing_call_threshold": 22,
            "concern_score_threshold": 0.65,
            "split_pressure": round_to(length_ratio + secondary, 2),
            "reason_codes": reasons,
            "concern_separation": concern,
        }));
    }
    if kind == "Class" {
        let length_ratio = f(length) / 120.0;
        let branch_ratio = f(branches) / 20.0;
        let absolute = f(length) / 250.0;
        if !((length_ratio >= 1.0 && branch_ratio >= 1.0) || absolute >= 1.0) {
            return None;
        }
        let mut reasons = vec!["large_class"];
        if branch_ratio >= 1.0 {
            reasons.push("branch_heavy");
        }
        if absolute >= 1.0 {
            reasons.push("very_large_class");
        }
        return Some(json!({
            "line_count": length,
            "branch_count": branches,
            "outgoing_call_count": calls,
            "line_threshold": 120,
            "branch_threshold": 20,
            "absolute_line_threshold": 250,
            "split_pressure": round_to((length_ratio + branch_ratio).max(absolute), 2),
            "reason_codes": reasons,
        }));
    }
    None
}

/// `_structural_suggestions`.
fn structural(
    store: &GraphStore,
    excluded: &HashSet<String>,
    communities: &HashMap<String, i64>,
) -> Option<Vec<Value>> {
    let nodes = store
        .get_nodes_by_kind(&["Function".to_string(), "Class".to_string()], None)
        .ok()?;
    let qns: Vec<String> = nodes.iter().map(|n| n.qualified_name.clone()).collect();
    let (outgoing, _) = store.get_edges_by_endpoints(&qns).ok()?;
    let mut cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut out = Vec::new();
    let empty: Vec<GraphEdge> = Vec::new();
    for node in &nodes {
        if excluded.contains(&node.qualified_name) {
            continue;
        }
        let lines = cache
            .entry(node.file_path.clone())
            .or_insert_with(|| source_lines(store, &node.file_path));
        if is_test_artifact(
            node.is_test,
            &node.file_path,
            &node.language,
            Some(node.line_start),
            lines,
        ) {
            continue;
        }
        let span = source_span(lines, node.line_start, node.line_end);
        let length = node.line_end - node.line_start + 1;
        let edges = outgoing.get(&node.qualified_name).unwrap_or(&empty);
        let calls = edges.iter().filter(|e| e.kind == "CALLS").count() as i64;
        let branches = branch_count(&span);
        let comments = comment_line_count(&span);
        let ratio = comments as f64 / length.max(1) as f64;
        let public = external_api_candidate(&node.language, node.line_start, lines);
        let concern = concern_profile(node, &span, edges, communities, branches, comments);
        let split = split_metrics(&node.kind, length, branches, calls, &concern);
        let complex = split.is_some();
        let name = sanitize(&node.name);
        let qn = sanitize(&node.qualified_name);
        if let Some(evidence) = split {
            out.push(json!({
                "type": "split",
                "description": format!("Split large {} '{name}'", node.kind.to_lowercase()),
                "symbols": [qn],
                "rationale": "The code unit is large and has complexity or concern-separation pressure, so extraction or decomposition may reduce maintenance risk.",
                "priority": "medium",
                "confidence": "medium",
                "category": "executable",
                "estimated_risk": "medium",
                "affected_files": [node.file_path],
                "reason_codes": evidence["reason_codes"],
                "evidence": evidence,
                "verification_steps": [
                    "Identify cohesive sub-responsibilities before extracting code.",
                    "Run tests that cover the affected code path after splitting.",
                ],
            }));
        }
        if (public || complex) && length >= 60 && ratio <= 0.01 {
            let mut reasons = vec!["low_explanation_density"];
            if public {
                reasons.push("public_api_candidate");
            }
            if complex {
                reasons.push("complexity_candidate");
            }
            out.push(json!({
                "type": "document",
                "description": format!("Document intent and invariants for '{name}'"),
                "symbols": [qn],
                "rationale": "The code unit is public or complex, but has very low explanation density.",
                "priority": "low",
                "confidence": "medium",
                "category": if public { "public_api" } else { "executable" },
                "estimated_risk": "low",
                "affected_files": [node.file_path],
                "reason_codes": reasons,
                "evidence": {
                    "line_count": length,
                    "comment_line_count": comments,
                    "comment_ratio": round_to(ratio, 3),
                    "line_threshold": 60,
                    "comment_ratio_threshold": 0.01,
                    "public_api_candidate": public,
                    "complexity_candidate": complex,
                },
                "verification_steps": [
                    "Add comments only for contracts, invariants, and non-obvious edge cases.",
                    "Avoid comments that restate the implementation line by line.",
                ],
            }));
        }
    }
    Some(out)
}

/// `_execution_plan_for_suggestion`.
fn execution_plan(suggestion: &Value) -> Value {
    let files: Vec<String> = suggestion["affected_files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| str_of(p).to_string())
        .collect();
    let mut tests: Vec<String> = files
        .iter()
        .take(3)
        .map(|p| format!("Run tests covering {p}"))
        .collect();
    if tests.is_empty() {
        tests.push("Run the narrowest tests that cover the affected symbol".to_string());
    }
    match str_of(&suggestion["type"]) {
        "split" => json!({
            "why_now": "The symbol crosses size or complexity thresholds in the evidence block.",
            "minimum_steps": ["Identify one cohesive responsibility to extract first.", "Move that responsibility behind a private helper or collaborator.", "Run focused tests and inspect detect_changes before continuing."],
            "safety_checks": ["Keep public names and call signatures stable in the first pass.", "Avoid moving unrelated logic while extracting the first responsibility."],
            "required_tests": tests,
            "rollback": "Revert the extraction commit if focused tests or impact review widen unexpectedly.",
            "defer_if": ["The symbol is public API and no caller contract is documented.", "No focused tests or reliable manual checks exist for the behavior."],
        }),
        "move" => json!({
            "why_now": "Callers are concentrated in another community according to graph evidence.",
            "minimum_steps": ["Inspect all listed callers and imports.", "Move the symbol without changing behavior.", "Run tests for both source and target communities."],
            "safety_checks": ["Check for dynamic imports or framework registration.", "Preserve public re-export paths when downstream callers may exist."],
            "required_tests": tests,
            "rollback": "Move the symbol back if imports, packaging, or external callers break.",
            "defer_if": ["Unknown callers are present.", "The target community boundary is not stable."],
        }),
        "remove" => json!({
            "why_now": "The graph found no callers, importers, references, tests, or subclasses.",
            "minimum_steps": ["Search for runtime registration, reflection, generated references, and docs mentions.", "Delete the smallest candidate first.", "Run focused tests and detect_changes before deleting more candidates."],
            "safety_checks": ["Treat public API, fixtures, and plugin entry points as high-risk.", "Verify generated code and downstream package exports manually."],
            "required_tests": tests,
            "rollback": "Restore the symbol if any dynamic or downstream reference appears.",
            "defer_if": ["The symbol is public API.", "The only evidence is absence from the graph and dynamic use is plausible."],
        }),
        "document" => json!({
            "why_now": "The symbol is public or complex but has low explanation density.",
            "minimum_steps": ["Document contracts, invariants, and non-obvious edge cases.", "Avoid comments that restate individual statements.", "Run docs or lint checks if the repository provides them."],
            "safety_checks": ["Keep documentation close to the behavior it constrains.", "Update related Markdown references when the contract is user-facing."],
            "required_tests": ["Run formatting or lint checks for touched files"],
            "rollback": "Remove or tighten comments that drift from behavior during review.",
            "defer_if": ["The code is about to be rewritten.", "The intended contract is still unresolved."],
        }),
        _ => json!({
            "why_now": "The suggestion has graph evidence but no specialized plan.",
            "minimum_steps": ["Inspect evidence, make the smallest safe change, then run focused tests."],
            "safety_checks": ["Verify public APIs, generated code, and dynamic dispatch before editing."],
            "required_tests": tests,
            "rollback": "Revert if impact review grows beyond the intended scope.",
            "defer_if": ["Evidence is ambiguous or the affected contract is unknown."],
        }),
    }
}

/// `_attach_execution_plan` with `_work_pack_for_suggestion`.
fn attach_plan(mut suggestion: Value) -> Value {
    let plan = execution_plan(&suggestion);
    let files: Vec<String> = suggestion["affected_files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| str_of(p).to_string())
        .collect();
    let pressure = suggestion["evidence"]
        .get("split_pressure")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let stype = str_of(&suggestion["type"]).to_string();
    let mut size = "small";
    if files.len() > 1 || pressure >= 10.0 || stype == "split" {
        size = "medium";
    }
    if pressure >= 25.0 || (stype == "split" && files.len() > 2) {
        size = "large";
    }
    let primary = files.first().cloned();
    let owner = primary.as_ref().map(|p| {
        let (absolute, parts) = posix_parts(p);
        let parent = &parts[..parts.len().saturating_sub(1)];
        match (absolute, parent.is_empty()) {
            (true, _) => format!("/{}", parent.join("/")),
            (false, true) => ".".to_string(),
            (false, false) => parent.join("/"),
        }
    });
    let mut obligations = Vec::new();
    if matches!(stype.as_str(), "move" | "remove" | "split") {
        obligations
            .push("Check docs_for/implementations_of for authored contracts before editing.");
    }
    if stype == "document" {
        obligations
            .push("Prefer contract, invariant, and edge-case documentation over line commentary.");
    }
    let first_step = plan["minimum_steps"]
        .get(0)
        .cloned()
        .unwrap_or(json!("Inspect evidence first."));
    let work_pack = json!({
        "owner_scope": owner,
        "primary_file": primary,
        "estimated_size": size,
        "blast_radius": {
            "affected_file_count": files.len(),
            "affected_files": files.iter().take(5).collect::<Vec<_>>(),
            "symbol_count": suggestion["symbols"].as_array().map_or(0, Vec::len),
            "estimated_risk": suggestion.get("estimated_risk").cloned().unwrap_or(json!("medium")),
        },
        "required_tests": plan["required_tests"],
        "documentation_obligations": obligations,
        "safe_first_commit": first_step,
        "rollback_path": plan.get("rollback").cloned().unwrap_or(Value::Null),
        "defer_conditions": plan["defer_if"],
        "first_commit": first_step,
        "verification_commands": plan["required_tests"],
        "success_criteria": [
            "Public behavior and call signatures are unchanged unless the suggestion explicitly says otherwise.",
            "Focused tests pass for the affected file or package.",
            "review_tool(mode=\"changes\") does not show unexpected blast-radius growth.",
        ],
        "risk_controls": plan.get("safety_checks").cloned().unwrap_or(json!([])),
    });
    if let Some(object) = suggestion.as_object_mut() {
        object.insert("execution_plan".into(), plan);
        object.insert("work_pack".into(), work_pack);
    }
    suggestion
}

/// `_suggestion_sort_key`.
fn sort_key(s: &Value) -> (i64, i64, i64, i64, i64, f64, String) {
    let rank = |value: &Value, table: &[(&str, i64)], default: i64| -> i64 {
        value
            .as_str()
            .and_then(|v| table.iter().find(|(k, _)| *k == v).map(|(_, r)| *r))
            .unwrap_or(default)
    };
    let field = |key: &str, default: &str| s.get(key).cloned().unwrap_or(json!(default));
    let evidence = &s["evidence"];
    let value = match str_of(&s["type"]) {
        "split" if evidence.is_object() => -evidence
            .get("split_pressure")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        "document" if evidence.is_object() => evidence
            .get("comment_ratio")
            .and_then(Value::as_f64)
            .unwrap_or(1.0),
        _ => 0.0,
    };
    let symbol = s["symbols"]
        .as_array()
        .and_then(|a| a.first())
        .map(|v| match v {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        });
    (
        rank(
            &field("category", "unknown"),
            &[
                ("executable", 0),
                ("unknown", 1),
                ("fixture", 2),
                ("test", 3),
                ("public_api", 4),
                ("documentation", 5),
            ],
            1,
        ),
        rank(
            &field("priority", "medium"),
            &[("high", 0), ("medium", 1), ("low", 2)],
            1,
        ),
        rank(
            &field("confidence", "medium"),
            &[("high", 0), ("medium", 1), ("low", 2)],
            1,
        ),
        rank(
            &field("estimated_risk", "medium"),
            &[("low", 0), ("medium", 1), ("high", 2)],
            1,
        ),
        rank(
            &field("type", "unknown"),
            &[("split", 0), ("move", 1), ("document", 2), ("remove", 3)],
            2,
        ),
        value,
        symbol.unwrap_or_default(),
    )
}

/// `suggest_refactorings(store)`.
pub(crate) fn suggest_refactorings(store: &GraphStore) -> Option<Vec<Value>> {
    let mut suggestions = Vec::new();
    let rows = store.get_communities_list().ok()?;
    let mut node_community: HashMap<String, i64> = HashMap::new();
    if !rows.is_empty() {
        let members = store.get_all_community_member_qns().ok()?;
        for (cid, _) in &rows {
            for qn in members.get(cid).into_iter().flatten() {
                node_community.insert(qn.clone(), *cid);
            }
        }
        // Communities feed the split concern profile; they no longer place
        // a function (docs/plans/REFACTOR-TOOL-TARGET.md#decisions-2026-10-07).
    }

    let dead = find_dead_code(store, None, None)?;
    let dead_qns: HashSet<String> = dead
        .iter()
        .filter_map(|d| d["qualified_name"].as_str().map(str::to_string))
        .collect();
    let mut cache: HashMap<String, Vec<String>> = HashMap::new();
    for record in dead {
        let file = str_of(&record["file"]).to_string();
        let lines = cache
            .entry(file.clone())
            .or_insert_with(|| source_lines(store, &file));
        let language = str_of(&record["language"]).to_string();
        let line = record["line"].as_i64();
        // Fixtures are test inputs: deleting one breaks its test.
        if is_test_artifact(false, &file, &language, line, lines)
            || crate::refactor::is_fixture_path(&file)
        {
            continue;
        }
        let mut record = record;
        if external_api_candidate(&language, line.unwrap_or(0), lines) {
            let mut reasons = record["reason_codes"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if !reasons.iter().any(|r| r == "public_api_candidate") {
                reasons.push(json!("public_api_candidate"));
            }
            record["confidence"] = json!("low");
            record["public_api_candidate"] = json!(true);
            record["reason_codes"] = json!(reasons);
        }
        // `_dead_code_category`.
        let category = if language == "markdown" {
            "documentation"
        } else if truthy(&record["public_api_candidate"]) {
            "public_api"
        } else if truthy(record.get("test_artifact").unwrap_or(&Value::Null))
            || is_test_file_path(&file)
        {
            "test"
        } else if file.contains("/fixtures/") || file.starts_with("tests/fixtures/") {
            "fixture"
        } else if PRODUCTION_LANGUAGES.contains(&language.as_str()) {
            "executable"
        } else {
            "unknown"
        };
        let mut steps = vec![
            "Search for runtime registration or dynamic dispatch before deleting.",
            "Run the tests that cover the affected file or package.",
        ];
        if category == "public_api" {
            steps.insert(
                0,
                "Verify crate-level and downstream API consumers before deleting.",
            );
        }
        suggestions.push(json!({
            "type": "remove",
            "description": format!("Remove unused {} '{}'", str_of(&record["kind"]).to_lowercase(), str_of(&record["name"])),
            "symbols": [record["qualified_name"]],
            "rationale": "No callers, test references, importers, references, or subclasses were found in the graph.",
            "priority": "low",
            "confidence": record.get("confidence").cloned().unwrap_or(json!("medium")),
            "category": category,
            "estimated_risk": if category == "public_api" { "high" } else { "medium" },
            "affected_files": [file],
            "reason_codes": record.get("reason_codes").cloned().unwrap_or(json!([])),
            "evidence": record.get("evidence").cloned().unwrap_or(json!({})),
            "verification_steps": steps,
        }));
    }
    suggestions.extend(structural(store, &dead_qns, &node_community)?);
    let mut suggestions: Vec<Value> = suggestions.into_iter().map(attach_plan).collect();
    suggestions.sort_by(|a, b| {
        let (ka, kb) = (sort_key(a), sort_key(b));
        ka.0.cmp(&kb.0)
            .then(ka.1.cmp(&kb.1))
            .then(ka.2.cmp(&kb.2))
            .then(ka.3.cmp(&kb.3))
            .then(ka.4.cmp(&kb.4))
            .then(ka.5.partial_cmp(&kb.5).unwrap_or(std::cmp::Ordering::Equal))
            .then(ka.6.cmp(&kb.6))
    });
    Some(suggestions)
}

/// `_apply_stability_policy_to_suggestions`.
/// The suggestions `refactor_tool(mode="suggest")` lists: every one, with
/// the stable-component policy applied.
pub(crate) fn ranked_suggestions(store: &GraphStore) -> Option<Vec<Value>> {
    let mut suggestions = suggest_refactorings(store)?;
    let snapshot = crate::architecture::Snapshot::read(store)?;
    let view = crate::architecture::View::review();
    let scopes = crate::architecture::ScopeGraph::new(&snapshot.dependencies(&view));
    let profiles = crate::review_summary::stability_profiles(
        &scopes,
        &crate::architecture::sap_metrics(&snapshot, &view, "package", None),
    );
    apply_stability_policy(&mut suggestions, &profiles);
    Some(suggestions)
}

pub(crate) fn apply_stability_policy(suggestions: &mut [Value], profiles: &HashMap<String, Value>) {
    for suggestion in suggestions.iter_mut() {
        let stable: Vec<Value> = suggestion["affected_files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| scope_key_for_file(str_of(p)))
            .filter_map(|scope| {
                let profile = profiles.get(&scope)?;
                (truthy(&profile["stable"]) || truthy(&profile["should_be_stable"]))
                    .then(|| profile.clone())
            })
            .collect();
        if stable.is_empty() {
            continue;
        }
        let Some(object) = suggestion.as_object_mut() else {
            continue;
        };
        object.insert(
            "stability_policy".into(),
            json!({
                "status": "stable_component_guard",
                "profiles": stable.iter().take(3).map(|p| json!({
                    "scope_key": p.get("scope_key"),
                    "instability": p.get("instability"),
                    "reason_codes": p.get("reason_codes").cloned().unwrap_or(json!([])),
                    "thresholds": p.get("thresholds").cloned().unwrap_or(json!({})),
                })).collect::<Vec<_>>(),
            }),
        );
        let push = |object: &mut Map<String, Value>, key: &str, list: &str, text: &str| {
            let holder = object.entry(key.to_string()).or_insert_with(|| json!({}));
            if let Some(map) = holder.as_object_mut() {
                let entry = map.entry(list.to_string()).or_insert_with(|| json!([]));
                if let Some(items) = entry.as_array_mut() {
                    items.push(json!(text));
                }
            }
        };
        push(
            object,
            "work_pack",
            "defer_conditions",
            "The affected component is stable or should be stable by shared policy.",
        );
        push(
            object,
            "execution_plan",
            "defer_if",
            "Stable component policy requires contract and test evidence first.",
        );
        if matches!(
            object.get("type").and_then(Value::as_str),
            Some("remove" | "move" | "split")
        ) {
            let high = object.get("confidence").and_then(Value::as_str) == Some("high");
            object.insert(
                "confidence".into(),
                json!(if high { "medium" } else { "low" }),
            );
            let split = object.get("type").and_then(Value::as_str) == Some("split");
            let codes = object.entry("reason_codes").or_insert_with(|| json!([]));
            if let Some(items) = codes.as_array_mut()
                && !items.iter().any(|c| c == "stable_component_guard")
            {
                items.push(json!("stable_component_guard"));
                // A split's `reason_codes` is its evidence's list in Python,
                // so the guard shows up there too.
                if split
                    && let Some(evidence) = object
                        .get_mut("evidence")
                        .and_then(|e| e.get_mut("reason_codes"))
                        .and_then(Value::as_array_mut)
                {
                    evidence.push(json!("stable_component_guard"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use dagayn_graph::{ConfidenceTier, GraphEdge, GraphNode};
    use serde_json::Value;

    use super::{branch_count, callee_scope, comment_line_count, concern_profile, parameter_names};

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    fn function(name: &str, file: &str, line_end: i64, params: &str) -> GraphNode {
        GraphNode {
            id: 0,
            kind: "Function".to_string(),
            name: name.to_string(),
            qualified_name: format!("{file}::{name}"),
            file_path: file.to_string(),
            line_start: 1,
            line_end,
            language: "python".to_string(),
            parent_name: None,
            params: Some(params.to_string()),
            return_type: None,
            is_test: false,
            file_hash: None,
            extra: Value::Null,
            signature: None,
        }
    }

    fn call(source: &str, target: &str) -> GraphEdge {
        GraphEdge {
            id: 0,
            kind: "CALLS".to_string(),
            source_qualified: source.to_string(),
            target_qualified: target.to_string(),
            file_path: String::new(),
            line: 1,
            extra: Value::Null,
            confidence: 1.0,
            confidence_tier: ConfidenceTier::Exact,
        }
    }

    #[test]
    fn lightweight_source_counters() {
        assert_eq!(
            branch_count(&lines(&[
                "if enabled:",
                "    for item in items:",
                "        value = a && b",
                "return value",
            ])),
            3
        );
        assert_eq!(
            comment_line_count(&lines(&[
                "# module note",
                "value = 1",
                "/* block start",
                "block body",
                "*/",
                "\"\"\"docstring\"\"\"",
            ])),
            5
        );
    }

    #[test]
    fn parameters_and_callee_scopes() {
        assert_eq!(
            parameter_names(Some(
                "(self, user_id: str, include_history=False, mut payload, flags)"
            )),
            ["user_id", "include_history", "payload", "flags"]
        );
        assert_eq!(
            callee_scope("src/orders/service.py::save_order").as_deref(),
            Some("src/orders")
        );
        assert_eq!(callee_scope("<dynamic:save_order>"), None);
    }

    #[test]
    fn a_small_pure_helper_is_a_transformer() {
        let node = function("normalize_order", "src/domain/orders.py", 2, "order");
        let span = lines(&["def normalize_order(order):", "    return order.strip()"]);
        let profile = concern_profile(&node, &span, &[], &HashMap::new(), 1, 0);
        assert_eq!(profile["role"], "transformer");
        assert_eq!(profile["evidence"]["side_effect_count"], 0);
    }

    #[test]
    fn mixed_concerns_score_as_a_refactoring_lead() {
        let mut span = lines(&[
            "def handle_order(user_id, payload, include_history, skip_cache, should_notify, dry_run, request_id, logger):",
            "    config = os.environ.get('ORDER_CONFIG')",
            "    logger.info(config)",
            "    raw = open('/tmp/orders.json').read()",
            "    response = requests.post('https://example.test/orders', json=payload)",
            "    db.execute('INSERT INTO orders VALUES (?)', [user_id])",
            "    service_a()",
            "    service_b()",
            "    service_c()",
        ]);
        span.extend((1..65).map(|idx| format!("    value_{idx} = {idx}")));
        let node = function(
            "handle_order",
            "src/commands/orders.py",
            span.len() as i64,
            "user_id, payload, include_history, skip_cache, should_notify, dry_run, request_id, logger",
        );
        let targets = [
            ("src/accounting/service_a.py::service_a", 1),
            ("src/notifications/service_b.py::service_b", 2),
            ("src/orders/service_c.py::service_c", 3),
        ];
        let edges: Vec<GraphEdge> = targets
            .iter()
            .map(|(target, _)| call(&node.qualified_name, target))
            .collect();
        let communities: HashMap<String, i64> = targets
            .iter()
            .map(|(target, community)| (target.to_string(), *community))
            .collect();
        let profile = concern_profile(
            &node,
            &span,
            &edges,
            &communities,
            branch_count(&span),
            comment_line_count(&span),
        );
        assert_eq!(profile["role"], "boundary");
        assert!(
            profile["score"].as_f64().unwrap()
                >= profile["evidence"]["split_score_threshold"]
                    .as_f64()
                    .unwrap()
        );
        assert_eq!(profile["confidence"], "medium");
        let reasons = profile["reason_codes"].as_array().unwrap();
        for code in [
            "many_callee_communities",
            "side_effect_pressure",
            "implicit_context",
        ] {
            assert!(reasons.iter().any(|r| r == code), "{code}: {reasons:?}");
        }
        let effects: Vec<&str> = profile["evidence"]["side_effect_reason_codes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for code in [
            "filesystem_io",
            "database_io",
            "network_io",
            "logging_or_console",
        ] {
            assert!(effects.contains(&code), "{code}: {effects:?}");
        }
        assert_eq!(profile["evidence"]["purity_likelihood"], 0.0);
        assert_eq!(profile["missingness"], serde_json::json!([]));
        assert!(
            profile["action"]
                .as_str()
                .unwrap()
                .starts_with("Extract one cohesive")
        );
    }
}
