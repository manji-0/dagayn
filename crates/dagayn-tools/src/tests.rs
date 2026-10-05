use serde_json::json;

use super::{Context, is_placeholder, suggestion_is_callable, suggestions};

#[test]
fn placeholders_are_template_strings_only() {
    assert!(is_placeholder("${workspaceFolder}"));
    assert!(is_placeholder("  ${workspaceFolder} "));
    assert!(!is_placeholder("${}"));
    assert!(!is_placeholder("${a}b"));
    assert!(!is_placeholder("/repo"));
}

#[test]
fn suggestions_follow_the_session_surface() {
    let context = Context {
        allowed_tools: Some(["flow_tool".to_string()].into_iter().collect()),
        ..Context::default()
    };
    assert!(suggestion_is_callable(
        &context,
        "flow_tool mode=\"list\" -- flows"
    ));
    assert!(!suggestion_is_callable(&context, "review_tool -- review"));
    assert!(suggestion_is_callable(&context, "Run: dagayn update"));
    assert!(suggestion_is_callable(&context, "re-run the build"));
    let (hints, kept) = suggestions(&context, &["review_tool -- r", "flow_tool(x) -- f"]);
    assert_eq!(kept, json!(["flow_tool(x) -- f"]));
    assert_eq!(
        hints,
        json!({"next_steps": [{"tool": "flow_tool", "suggestion": "flow_tool(x) -- f"}],
               "related": [], "warnings": []})
    );
}

#[test]
fn unexpected_arguments_go_to_python() {
    let context = Context::default();
    for (name, arguments) in [
        ("list_graph_stats_tool", json!({"bogus": 1})),
        ("list_graph_stats_tool", json!({"repo_root": 3})),
        ("get_docs_section_tool", json!({})),
        (
            "get_docs_section_tool",
            json!({"section_name": "trust", "max_chars": "10"}),
        ),
        (
            "get_docs_section_tool",
            json!({"section_name": "trust", "max_chars": 0}),
        ),
        ("query_graph_tool", json!({})),
    ] {
        assert!(
            super::call(&context, name, &arguments).is_none(),
            "{name} {arguments}"
        );
    }
}
