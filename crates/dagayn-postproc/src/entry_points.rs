//! Entry-point detection heuristics ported from ``dagayn/entry_point_heuristics.py``.

use std::sync::LazyLock;

use dagayn_graph::GraphNode;
use regex::Regex;
use serde_json::Value;

static TEST_FILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"([/\\]__tests__[/\\]|\.spec\.[jt]sx?$|\.test\.[jt]sx?$|[/\\]test_[^/\\]*\.py$)",
    )
    .expect("valid test file regex")
});

static FRAMEWORK_DECORATOR_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    const PATTERNS: &[&str] = &[
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
        r"(app|router)\.(get|post|put|delete|patch|use|all)\b",
        r"(?i)@(Override|OnLifecycleEvent|Composable)",
        r"(HiltViewModel|AndroidEntryPoint|Inject)",
        r"(?i)\w+\.(tool|tool_plain|system_prompt|result_validator)\b",
        r"(?i)^tool\b",
        r"(?i)\w+\.(middleware|exception_handler|on_exception)\b",
        r"(?i)\w+\.route\b",
    ];
    PATTERNS
        .iter()
        .map(|pattern| Regex::new(pattern).expect("valid decorator regex"))
        .collect()
});

static ENTRY_NAME_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    const PATTERNS: &[&str] = &[
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
    ];
    PATTERNS
        .iter()
        .map(|pattern| Regex::new(pattern).expect("valid entry name regex"))
        .collect()
});

pub(crate) fn is_test_file(file_path: &str) -> bool {
    TEST_FILE_RE.is_match(file_path)
}

pub(crate) fn has_framework_decorator(node: &GraphNode) -> bool {
    let Some(decorators) = node.extra.get("decorators") else {
        return false;
    };
    match decorators {
        Value::String(dec) => decorator_matches(dec),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .any(decorator_matches),
        _ => false,
    }
}

fn decorator_matches(decorator: &str) -> bool {
    FRAMEWORK_DECORATOR_PATTERNS
        .iter()
        .any(|pattern| pattern.is_match(decorator))
}

pub(crate) fn matches_entry_name(node: &GraphNode) -> bool {
    ENTRY_NAME_PATTERNS
        .iter()
        .any(|pattern| pattern.is_match(&node.name))
}
