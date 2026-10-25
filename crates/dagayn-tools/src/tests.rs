use serde_json::json;

use super::{Context, is_placeholder};

#[test]
fn placeholders_are_template_strings_only() {
    assert!(is_placeholder("${workspaceFolder}"));
    assert!(is_placeholder("  ${workspaceFolder} "));
    assert!(!is_placeholder("${}"));
    assert!(!is_placeholder("${a}b"));
    assert!(!is_placeholder("/repo"));
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
        ("query_graph_tool", json!({})),
    ] {
        assert!(
            super::call(&context, name, &arguments).is_none(),
            "{name} {arguments}"
        );
    }
}
