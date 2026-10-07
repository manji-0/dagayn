//! The `dagayn.hints` session and `generate_hints`.
//!
//! One [`SessionState`] serves a `dagayn serve` session: the tools answered in
//! Rust update it here, and the Python server, once booted, reads and writes
//! the same state through `_core.HintSession`, which replaces
//! `dagayn.hints._session`. Hints that suppress already-called tools or
//! already-touched files then see every call, whichever side answered it.

use std::collections::{HashSet, VecDeque};
use std::sync::{LazyLock, Mutex, MutexGuard};

use serde_json::{Value, json};

/// `_MAX_TOOLS_HISTORY`, `_MAX_NODES_TRACKED`, `_MAX_PER_CATEGORY`.
const MAX_TOOLS_HISTORY: usize = 100;
const MAX_NODES_TRACKED: usize = 1000;
const MAX_PER_CATEGORY: usize = 3;

/// `dagayn.hints.SessionState`.
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub tools_called: VecDeque<String>,
    pub nodes_queried: HashSet<String>,
    pub files_touched: HashSet<String>,
    pub inferred_intent: Option<String>,
    pub last_tool_time: f64,
}

impl SessionState {
    pub fn record_tool_call(&mut self, tool_name: &str) {
        if self.tools_called.len() == MAX_TOOLS_HISTORY {
            self.tools_called.pop_front();
        }
        self.tools_called.push_back(tool_name.to_string());
        self.last_tool_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs_f64())
            .unwrap_or(0.0);
    }

    pub fn record_nodes<'a>(&mut self, node_ids: impl IntoIterator<Item = &'a str>) {
        for id in node_ids {
            if self.nodes_queried.len() >= MAX_NODES_TRACKED {
                break;
            }
            self.nodes_queried.insert(id.to_string());
        }
    }

    pub fn record_files<'a>(&mut self, files: impl IntoIterator<Item = &'a str>) {
        self.files_touched
            .extend(files.into_iter().map(str::to_string));
    }
}

/// The process's session: one `dagayn serve` per process, as Python's
/// module-level `_session`.
static SESSION: LazyLock<Mutex<SessionState>> = LazyLock::new(Mutex::default);

/// Lock the process's session. Hold it only around Rust code: a Python thread
/// waiting on it holds the GIL.
pub fn session() -> MutexGuard<'static, SessionState> {
    SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `reset_session`.
pub fn reset_session() {
    *session() = SessionState::default();
}

/// `_INTENT_TOOLS`, in Python's dict order (ties go to the first).
const INTENT_TOOLS: [(&str, &[&str]); 4] = [
    (
        "reviewing",
        &[
            "review",
            "review_tool",
            "detect_changes",
            "get_review_context",
            "get_affected_flows",
            "get_impact_radius",
        ],
    ),
    (
        "debugging",
        &[
            "query_graph",
            "query_graph_tool",
            "flow",
            "flow_tool",
            "get_flow",
            "semantic_search_nodes",
            "semantic_search_nodes_tool",
        ],
    ),
    (
        "refactoring",
        &[
            "refactor",
            "refactor_tool",
            "find_dead_code",
            "find_large_functions_tool",
            "suggest_refactorings",
        ],
    ),
    (
        "exploring",
        &[
            "architecture_analysis",
            "flow",
            "flow_tool",
            "list_flows",
            "list_graph_stats",
        ],
    ),
];

/// `infer_intent`.
pub fn infer_intent(session: &SessionState) -> &'static str {
    if session.tools_called.is_empty() {
        return "exploring";
    }
    let recent: Vec<&String> = session.tools_called.iter().rev().take(10).collect();
    let mut best = ("reviewing", 0);
    for (index, (intent, tools)) in INTENT_TOOLS.iter().enumerate() {
        let score = recent
            .iter()
            .filter(|tool| tools.contains(&tool.as_str()))
            .count();
        // `max` keeps the first of equal scores.
        if index == 0 || score > best.1 {
            best = (intent, score);
        }
    }
    if best.1 == 0 { "exploring" } else { best.0 }
}

/// `_WORKFLOW`, generated from `dagayn.hints` (a test compares the two).
fn workflow(tool_name: &str) -> &'static [(&'static str, &'static str)] {
    match tool_name {
        "flow" => &[
            (
                "query_graph_tool",
                "Read an entry point or a chain step with pattern=\"source_of\"",
            ),
            (
                "review_tool",
                "See which entry points reach the change with mode=\"affected_flows\"",
            ),
            (
                "architecture_analysis_tool",
                "See the high-level architecture with mode=\"overview\"",
            ),
        ],
        "list_flows" => &[
            ("flow_tool", "Use the public dispatcher with mode=\"get\""),
            (
                "review_tool",
                "See which entry points reach the change with mode=\"affected_flows\"",
            ),
            (
                "architecture_analysis_tool",
                "See the high-level architecture with mode=\"overview\"",
            ),
        ],
        "get_flow" => &[
            (
                "query_graph_tool",
                "Inspect callers/callees of a step in this flow",
            ),
            (
                "review_tool",
                "See which entry points reach the change with mode=\"affected_flows\"",
            ),
            (
                "flow_tool",
                "Browse other execution flows with mode=\"list\"",
            ),
        ],
        "get_affected_flows" => &[
            (
                "review_tool",
                "Get risk-scored change analysis with mode=\"changes\"",
            ),
            (
                "flow_tool",
                "Find the entry points reaching one symbol with mode=\"entry_points\"",
            ),
            (
                "review_tool",
                "Build full review context with mode=\"context\"",
            ),
        ],
        "list_communities" => &[
            (
                "architecture_analysis_tool",
                "Inspect a specific community with mode=\"community\"",
            ),
            (
                "architecture_analysis_tool",
                "See cross-community coupling with mode=\"overview\"",
            ),
            (
                "flow_tool",
                "Find where a community member is entered from with mode=\"entry_points\"",
            ),
        ],
        "get_community" => &[
            (
                "query_graph_tool",
                "Explore callers/callees of community members",
            ),
            (
                "architecture_analysis_tool",
                "Browse other communities with mode=\"communities\"",
            ),
            (
                "architecture_analysis_tool",
                "See how this community fits the architecture with mode=\"overview\"",
            ),
        ],
        "architecture_analysis" => &[
            (
                "architecture_analysis_tool",
                "Drill into communities with mode=\"communities\" or mode=\"community\"",
            ),
            (
                "query_graph_tool",
                "Trace callers/callees between coupled communities (pattern=callers_of)",
            ),
            (
                "review_tool",
                "See how recent changes affect the architecture",
            ),
            (
                "flow_tool",
                "Find where a symbol is entered from with mode=\"entry_points\"",
            ),
        ],
        "get_architecture_overview" => &[
            (
                "architecture_analysis_tool",
                "Use the public dispatcher with mode=\"overview\"",
            ),
            (
                "query_graph_tool",
                "Trace callers/callees between coupled communities (pattern=callers_of)",
            ),
            (
                "review_tool",
                "See how recent changes affect the architecture",
            ),
        ],
        "review" => &[
            (
                "review_tool",
                "Fetch focused source context with mode=\"context\"",
            ),
            (
                "review_tool",
                "See which entry points reach the change with mode=\"affected_flows\"",
            ),
            ("review_tool", "Expand blast radius with mode=\"impact\""),
            (
                "flow_tool",
                "Find the entry points reaching a symbol with mode=\"entry_points\"",
            ),
        ],
        "detect_changes" => &[
            (
                "review_tool",
                "Build full review context with mode=\"context\"",
            ),
            (
                "review_tool",
                "See which entry points reach the change with mode=\"affected_flows\"",
            ),
            ("review_tool", "Expand blast radius with mode=\"impact\""),
            (
                "refactor_tool",
                "Look for refactoring opportunities in changed code",
            ),
        ],
        "refactor" => &[
            (
                "query_graph_tool",
                "Verify call sites before applying a rename",
            ),
            ("review_tool", "Check risk of the refactored code"),
            (
                "semantic_search_nodes_tool",
                "Find related symbols to also rename",
            ),
        ],
        "semantic_search_nodes" => &[
            (
                "query_graph_tool",
                "Inspect callers/callees of a search result",
            ),
            (
                "flow_tool",
                "Find the entry points reaching a matched node with mode=\"entry_points\"",
            ),
            ("review_tool", "Check the blast radius from matched nodes"),
        ],
        _ => &[],
    }
}

/// `generate_hints`: update `session` for a `tool_name` answer and return the
/// hints Python would attach. `exposed` is the session's tool surface.
pub fn generate_hints(
    tool_name: &str,
    result: &Value,
    session: &mut SessionState,
    exposed: &dyn Fn(&str) -> bool,
) -> Value {
    session.record_tool_call(tool_name);
    session.inferred_intent = Some(infer_intent(session).to_string());

    // `_build_next_steps`.
    let called: HashSet<&str> = session.tools_called.iter().map(String::as_str).collect();
    let next_steps: Vec<Value> = workflow(tool_name)
        .iter()
        .filter(|(tool, _)| !called.contains(tool) && exposed(tool))
        .take(MAX_PER_CATEGORY)
        .map(|(tool, suggestion)| json!({"tool": tool, "suggestion": suggestion}))
        .collect();

    // `_extract_warnings`.
    let mut warnings: Vec<String> = Vec::new();
    if let Some(gaps) = result.get("test_gaps").and_then(Value::as_array)
        && !gaps.is_empty()
    {
        let names: Vec<String> = gaps
            .iter()
            .take(5)
            .map(|gap| match gap.get("name") {
                Some(Value::String(name)) => name.clone(),
                _ => python_str(gap),
            })
            .collect();
        warnings.push(format!("Test coverage gaps: {}", names.join(", ")));
    }
    // A bool is an int to Python's `isinstance`.
    let risk = match result.get("risk_score") {
        Some(Value::Bool(flag)) => Some(f64::from(u8::from(*flag))),
        Some(other) => other.as_f64(),
        None => None,
    };
    if let Some(risk) = risk
        && risk > 0.7
    {
        warnings.push(format!("High risk score ({risk:.2}) — review carefully"));
    }
    if let Some(items) = result.get("warnings").and_then(Value::as_array) {
        for item in items.iter().take(3) {
            match item {
                Value::String(text) => warnings.push(text.clone()),
                Value::Object(map) => {
                    if let Some(Value::String(message)) = map.get("message") {
                        warnings.push(message.clone());
                    }
                }
                _ => {}
            }
        }
    }

    // `_build_related`, before the result's own files are tracked.
    let mut related: Vec<String> = Vec::new();
    if let Some(files) = result.get("impacted_files").and_then(Value::as_array) {
        for file in files.iter().filter_map(Value::as_str) {
            if !session.files_touched.contains(file) && !related.iter().any(|seen| seen == file) {
                related.push(file.to_string());
                if related.len() >= MAX_PER_CATEGORY {
                    break;
                }
            }
        }
    }

    // `_track_result`.
    for key in ["changed_files", "impacted_files"] {
        if let Some(files) = result.get(key).and_then(Value::as_array) {
            session.record_files(files.iter().filter_map(Value::as_str));
        }
    }
    let mut node_ids: Vec<&str> = Vec::new();
    for key in ["results", "changed_nodes", "impacted_nodes"] {
        if let Some(items) = result.get(key).and_then(Value::as_array) {
            for item in items {
                if let Some(qualified) = item.get("qualified_name").and_then(Value::as_str)
                    && !qualified.is_empty()
                {
                    node_ids.push(qualified);
                }
            }
        }
    }
    session.record_nodes(node_ids);

    warnings.truncate(MAX_PER_CATEGORY);
    json!({"next_steps": next_steps, "related": related, "warnings": warnings})
}

/// `str(value)` for the JSON values a test gap can be.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionState, generate_hints, infer_intent};
    use serde_json::json;

    #[test]
    fn called_tools_leave_the_next_steps_and_set_the_intent() {
        let mut session = SessionState::default();
        let all = |_: &str| true;
        let first = generate_hints("review", &json!({}), &mut session, &all);
        assert_eq!(first["next_steps"].as_array().map(Vec::len), Some(3));
        session.record_tool_call("review_tool");
        let second = generate_hints("review", &json!({}), &mut session, &all);
        assert_eq!(
            second["next_steps"],
            json!([{"tool": "flow_tool", "suggestion": "Find the entry points reaching a symbol with mode=\"entry_points\""}])
        );
        assert_eq!(session.inferred_intent.as_deref(), Some("reviewing"));
        assert_eq!(infer_intent(&SessionState::default()), "exploring");
    }

    #[test]
    fn related_files_skip_the_touched_ones_then_track_the_result() {
        let mut session = SessionState::default();
        session.record_files(["a.py"]);
        let hints = generate_hints(
            "review",
            &json!({"impacted_files": ["a.py", "b.py"], "changed_files": ["c.py"],
                    "test_gaps": [{"name": "f"}, "g"], "risk_score": 0.9}),
            &mut session,
            &|_| true,
        );
        assert_eq!(hints["related"], json!(["b.py"]));
        assert_eq!(
            hints["warnings"],
            json!([
                "Test coverage gaps: f, g",
                "High risk score (0.90) — review carefully"
            ])
        );
        assert!(session.files_touched.contains("b.py") && session.files_touched.contains("c.py"));
    }
}
