use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_dart_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "dart".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            dart_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
                &mut nodes,
                &mut edges,
            );
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn dart_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    dart_emit_calls_from_children(node, source, file_path, edges);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_or_export" => {
                dart_emit_import(child, source, file_path, edges);
            }
            "class_definition" | "mixin_declaration" | "enum_declaration" => {
                if let Some(name) = dart_direct_child_text(child, source, &["identifier"]) {
                    dart_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    dart_walk_children(child, source, file_path, Some(&name), nodes, edges);
                    continue;
                }
            }
            "function_signature" => {
                if let Some(name) = dart_direct_child_text(child, source, &["identifier"]) {
                    dart_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            _ => {}
        }
        dart_walk_children(child, source, file_path, enclosing_class, nodes, edges);
    }
}

fn dart_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = dart_first_descendant_text(node, source, &["string_literal"]) else {
        return;
    };
    let target = strip_matching_quotes(target.trim()).to_string();
    if target.is_empty() {
        return;
    }
    edges.push(ParsedEdge {
        kind: "IMPORTS_FROM".to_string(),
        source: file_path.to_string(),
        target,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn dart_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract) = match node.kind() {
        "mixin_declaration" => ("mixin", false),
        "enum_declaration" => ("enum", false),
        _ if dart_has_direct_child_kind(node, "abstract") => ("abstract_class", true),
        _ => ("class", false),
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if dart_is_value_container(type_role) {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "dart".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: file_path.to_string(),
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for target in dart_inheritance_targets(node, source) {
        edges.push(ParsedEdge {
            kind: "INHERITS".to_string(),
            source: qualified.clone(),
            target,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": "extends",
                "syntax_source": "class_definition",
            }),
        });
    }
}

fn dart_is_value_container(type_role: &str) -> bool {
    matches!(type_role, "enum")
}

fn dart_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: "Function".to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "dart".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: dart_direct_child_text(node, source, &["formal_parameter_list"]),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn dart_emit_calls_from_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut call_name = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                call_name = Some(node_text(child, source));
            }
            "selector" => {
                if let Some(method_name) = dart_selector_method_name(child, source) {
                    call_name = Some(method_name);
                }
                if dart_selector_has_arguments(child) {
                    if let Some(target) = call_name.take() {
                        edges.push(ParsedEdge {
                            kind: "CALLS".to_string(),
                            source: file_path.to_string(),
                            target,
                            file_path: file_path.to_string(),
                            line: node.start_position().row as i64 + 1,
                            extra: json!({}),
                        });
                    }
                }
            }
            "return" | "await" | "yield" | "this" | "const" | "new" => {}
            _ => {
                call_name = None;
            }
        }
    }
}

fn dart_selector_method_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "unconditional_assignable_selector" {
            return dart_first_descendant_text(child, source, &["identifier"]);
        }
    }
    None
}

fn dart_selector_has_arguments(node: tree_sitter::Node<'_>) -> bool {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|child| child.kind() == "argument_part");
    found
}

fn dart_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "superclass" | "interfaces") {
            dart_collect_type_identifiers(child, source, &mut out);
        }
    }
    out
}

fn dart_collect_type_identifiers(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    out: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_identifier" {
            out.push(node_text(child, source));
        } else {
            dart_collect_type_identifiers(child, source, out);
        }
    }
}

fn dart_has_direct_child_kind(node: tree_sitter::Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|child| child.kind() == kind);
    found
}

fn dart_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn dart_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    dart_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn dart_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = dart_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}
