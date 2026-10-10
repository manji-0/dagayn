use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use regex::Regex;
use serde_json::{Value, json};

use crate::*;

struct TraceGraph {
    calls_out: HashMap<String, Box<[String]>>,
    nodes_by_qn: HashMap<String, Arc<GraphNode>>,
}

fn freeze_adjacency(map: HashMap<String, Vec<String>>) -> HashMap<String, Box<[String]>> {
    map.into_iter()
        .map(|(key, values)| (key, values.into_boxed_slice()))
        .collect()
}

fn index_nodes(nodes: impl IntoIterator<Item = GraphNode>) -> HashMap<String, Arc<GraphNode>> {
    nodes
        .into_iter()
        .map(|node| (node.qualified_name.clone(), Arc::new(node)))
        .collect()
}

impl GraphStore {
    pub fn detect_entry_points_json(&self, include_tests: bool) -> Result<String> {
        let entries = self.detect_entry_points(include_tests)?;
        let payload: Vec<Value> = entries
            .into_iter()
            .map(|node| {
                json!({
                    "id": node.id,
                    "name": node.name,
                    "qualified_name": node.qualified_name,
                    "file_path": node.file_path,
                    "kind": node.kind,
                    "is_test": node.is_test,
                })
            })
            .collect();
        serde_json::to_string(&payload).map_err(Into::into)
    }

    pub fn detect_entry_points(&self, include_tests: bool) -> Result<Vec<GraphNode>> {
        let graph = self.load_trace_graph()?;
        Ok(detect_entries(&graph, include_tests))
    }

    fn load_trace_graph(&self) -> Result<TraceGraph> {
        let (calls_out, _) = self.get_flow_edge_data()?;
        Ok(TraceGraph {
            calls_out: freeze_adjacency(calls_out),
            nodes_by_qn: index_nodes(self.get_all_nodes_filtered(false)?),
        })
    }
}

/// Entries come from a HashMap walk: sort them so the same graph lists them
/// in the same order.
fn sort_entries(entries: &mut [GraphNode]) {
    entries.sort_unstable_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
}

fn detect_entries(graph: &TraceGraph, include_tests: bool) -> Vec<GraphNode> {
    let called: HashSet<&str> = graph
        .calls_out
        .values()
        .flatten()
        .map(String::as_str)
        .collect();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for node in graph.nodes_by_qn.values() {
        if !is_entry_kind(node) {
            continue;
        }
        if !include_tests && (node.is_test || is_test_file(&node.file_path)) {
            continue;
        }
        if is_entry_point(node, called.contains(node.qualified_name.as_str()))
            && seen.insert(node.qualified_name.clone())
        {
            entries.push((**node).clone());
        }
    }
    sort_entries(&mut entries);
    entries
}

fn is_entry_kind(node: &GraphNode) -> bool {
    node.kind == "Function" || node.kind == "Test"
}

pub(crate) fn is_test_file(file_path: &str) -> bool {
    test_file_re().is_match(file_path)
}

/// `matches_entry_name(node) or has_framework_decorator(node)`.
pub fn is_conventional_entry_point(node: &GraphNode) -> bool {
    matches_entry_name(&node.name) || has_framework_decorator(node)
}

fn is_entry_point(node: &GraphNode, is_called: bool) -> bool {
    if !is_called {
        return true;
    }
    has_framework_decorator(node) || matches_entry_name(&node.name)
}

/// `has_framework_decorator`.
pub fn has_framework_decorator(node: &GraphNode) -> bool {
    let Some(decorators) = node.extra.get("decorators") else {
        return false;
    };
    let values = match decorators {
        Value::String(value) => vec![value.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let patterns = decorator_res();
    values
        .iter()
        .any(|value| patterns.iter().any(|pattern| pattern.is_match(value)))
}

fn matches_entry_name(name: &str) -> bool {
    entry_name_res()
        .iter()
        .any(|pattern| pattern.is_match(name))
}

fn test_file_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"([\\/]__tests__[\\/]|\.(spec|test|cy)\.[cm]?[jt]sx?$|[\\/]test_[^/\\]*\.py$)")
            .expect("test file regex")
    })
}

fn decorator_res() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        [
            r"(?i)app\.(get|post|put|delete|patch|route|websocket|on_event)",
            r"(?i)router\.(get|post|put|delete|patch|route)",
            r"(?i)blueprint\.(route|before_request|after_request)",
            r"(?i)(before|after)_(request|response)",
            r"(?i)click\.(command|group)",
            r"(?i)\w+\.(command|group)\b",
            r"(?i)(field|model)_(serializer|validator)",
            r"(?i)(celery\.)?(task|shared_task|periodic_task)",
            r"(?i)receiver",
            r"(?i)api_view",
            r"(?i)\baction\b",
            r"pytest\.(fixture|mark)",
            r"(?i)(override_settings|modify_settings)",
            r"(?i)(event\.)?listens_for",
            r"(?i)(Get|Post|Put|Delete|Patch|RequestMapping)Mapping",
            r"(?i)(Scheduled|EventListener|Bean|Configuration)",
            r"(?i)(Component|Injectable|Controller|Module|Guard|Pipe)",
            r"(?i)(Subscribe|Mutation|Query|Resolver)",
            // Keep in sync with dagayn/entry_point_heuristics.py
            // (tests/test_flows.py compares the two lists).
            r"^(Get|Post|Put|Delete|Patch|Options|Head|All)$",
            r"^(MessagePattern|EventPattern|Cron|Interval|Timeout|OnEvent|Process|Processor|SubscribeMessage|WebSocketGateway|HostListener)$",
            r"(app|router)\.(get|post|put|delete|patch|use|all)\b",
            r"(?i)@(Override|OnLifecycleEvent|Composable)",
            r"(?i)(HiltViewModel|AndroidEntryPoint|Inject)",
            r"(?i)\w+\.(tool|tool_plain|prompt|resource|system_prompt|result_validator)\b",
            r"^tool\b",
            r"(?i)\w+\.(middleware|exception_handler|on_exception)\b",
            r"(?i)\w+\.route\b",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("decorator regex"))
        .collect()
    })
}

fn entry_name_res() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        [
            r"^main$",
            r"^__main__$",
            r"^test_",
            r"^Test[A-Z]",
            r"^on_",
            r"^handle_",
            r"^handler$",
            r"^handle$",
            r"^lambda_handler$",
            r"^upgrade$",
            r"^downgrade$",
            r"^lifespan$",
            r"^get_db$",
            r"^on(Create|Start|Resume|Pause|Stop|Destroy|Bind|Receive)",
            r"^do(Get|Post|Put|Delete)$",
            r"^do_(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)$",
            r"^log_message$",
            r"^(middleware|errorHandler)$",
            r"^ng(OnInit|OnChanges|OnDestroy|DoCheck|AfterContentInit|AfterContentChecked|AfterViewInit|AfterViewChecked)$",
            r"^(transform|writeValue|registerOnChange|registerOnTouched|setDisabledState)$",
            r"^(canActivate|canDeactivate|canActivateChild|canLoad|canMatch|resolve)$",
            r"^(componentDidMount|componentDidUpdate|componentWillUnmount|shouldComponentUpdate|render)$",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("entry name regex"))
        .collect()
    })
}
