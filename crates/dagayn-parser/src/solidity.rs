use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_solidity_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "solidity".to_string(),
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
            solidity_walk_children(
                tree.root_node(),
                source,
                file_path,
                None,
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

fn solidity_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_directive" => {
                solidity_emit_import(child, source, file_path, edges);
            }
            "contract_declaration"
            | "interface_declaration"
            | "library_declaration"
            | "struct_declaration"
            | "enum_declaration"
            | "error_declaration"
            | "user_defined_type_definition" => {
                if let Some(name) = solidity_direct_child_text(child, source, &["identifier"]) {
                    solidity_emit_type(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    solidity_walk_children(
                        child,
                        source,
                        file_path,
                        Some(&name),
                        None,
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "constant_variable_declaration" => {
                if solidity_emit_constant(child, source, file_path, enclosing_class, nodes, edges) {
                    continue;
                }
            }
            "state_variable_declaration" if enclosing_class.is_some() => {
                if solidity_emit_state_variable(
                    child,
                    source,
                    file_path,
                    enclosing_class.unwrap_or_default(),
                    nodes,
                    edges,
                ) {
                    continue;
                }
            }
            "function_definition"
            | "constructor_definition"
            | "modifier_definition"
            | "event_definition"
            | "fallback_receive_definition" => {
                if let Some(name) = solidity_function_name(child, source) {
                    solidity_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    solidity_emit_modifier_invocation_calls(
                        child,
                        source,
                        file_path,
                        &qualify(file_path, &name, enclosing_class),
                        edges,
                    );
                    solidity_walk_children(
                        child,
                        source,
                        file_path,
                        enclosing_class,
                        Some(&name),
                        nodes,
                        edges,
                    );
                    continue;
                }
            }
            "using_directive" => {
                solidity_emit_using(child, source, file_path, enclosing_class, edges);
                continue;
            }
            "emit_statement" => {
                solidity_emit_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            "call_expression" => {
                solidity_emit_call(
                    child,
                    source,
                    file_path,
                    enclosing_class,
                    enclosing_func,
                    edges,
                );
            }
            _ => {}
        }
        solidity_walk_children(
            child,
            source,
            file_path,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn solidity_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string" {
            let target = strip_matching_quotes(node_text(child, source).trim()).to_string();
            if !target.is_empty() {
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::ImportsFrom,
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.to_string(),
                    line: node.start_position().row as i64 + 1,
                    extra: json!({}),
                });
            }
        }
    }
}

fn solidity_emit_type(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = match node.kind() {
        "interface_declaration" => ("interface", true, true),
        "struct_declaration" => ("struct", false, false),
        "enum_declaration" => ("enum", false, false),
        _ => ("class", false, false),
    };
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if is_abstract {
            map.insert("is_abstract".to_string(), json!(true));
        }
        if is_contract {
            map.insert("is_contract".to_string(), json!(true));
        }
    }
    let qualified = qualify(file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "solidity".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: file_path.to_string(),
        target: qualified.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    for target in solidity_inheritance_targets(node, source) {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Inherits,
            source: qualified.clone(),
            target,
            file_path: file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({
                "relationship_role": "extends",
                "syntax_source": "contract_declaration",
            }),
        });
    }
}

fn solidity_emit_constant(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(name) = solidity_direct_child_text(node, source, &["identifier"]) else {
        return false;
    };
    let qualified = qualify(file_path, &name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.clone(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "solidity".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: solidity_direct_child_text(node, source, &["type_name"]),
        modifiers: None,
        is_test: false,
        extra: json!({"solidity_kind": "constant"}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    true
}

fn solidity_emit_state_variable(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(name) = solidity_direct_child_text(node, source, &["identifier"]) else {
        return false;
    };
    let qualified = qualify(file_path, &name, Some(enclosing_class));
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.clone(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "solidity".to_string(),
        parent_name: Some(enclosing_class.to_string()),
        params: None,
        return_type: solidity_direct_child_text(node, source, &["type_name"]),
        modifiers: solidity_direct_child_text(node, source, &["visibility"]),
        is_test: false,
        extra: json!({
            "solidity_kind": "state_variable",
            "mutability": solidity_direct_child_kind(node, &["constant", "immutable"]),
        }),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: qualify(file_path, enclosing_class, None),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    true
}

fn solidity_emit_function(
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
        kind: crate::core::types::NodeKind::Function,
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "solidity".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: solidity_params(node, source),
        return_type: solidity_direct_child_text(node, source, &["return_type_definition"]),
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn solidity_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(call_name) = solidity_call_name(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller,
        target: call_name,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn solidity_emit_modifier_invocation_calls(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier_invocation" {
            if let Some(name) = solidity_first_descendant_text(child, source, &["identifier"]) {
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::Calls,
                    source: caller.to_string(),
                    target: name,
                    file_path: file_path.to_string(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
            }
        }
    }
}

fn solidity_emit_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(name) = solidity_first_descendant_text(node, source, &["identifier"]) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller,
        target: name,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn solidity_emit_using(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = solidity_first_descendant_text(node, source, &["identifier"]) else {
        return;
    };
    let source_name = enclosing_class
        .map(|class| qualify(file_path, class, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::DependsOn,
        source: source_name,
        target,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn solidity_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "constructor_definition" => Some("constructor".to_string()),
        "fallback_receive_definition" => {
            solidity_direct_child_kind(node, &["receive", "fallback"]).map(str::to_string)
        }
        _ => solidity_direct_child_text(node, source, &["identifier"]),
    }
}

fn solidity_params(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let params = solidity_direct_child_texts(node, source, &["parameter"]);
    (!params.is_empty()).then(|| format!("({})", params.join(", ")))
}

fn solidity_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = solidity_call_callee(node)?;
    let callee = if callee.kind() == "expression" {
        solidity_first_non_punctuation_child(callee).unwrap_or(callee)
    } else {
        callee
    };
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        "member_expression" => solidity_last_descendant_text(callee, source, &["identifier"]),
        _ => None,
    }
}

fn solidity_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "call_arguments" | "arguments"));
    found
}

fn solidity_inheritance_targets(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "inheritance_specifier" {
            if let Some(target) = solidity_first_descendant_text(child, source, &["identifier"]) {
                out.push(target);
            }
        }
    }
    out
}

fn solidity_first_non_punctuation_child(
    node: tree_sitter::Node<'_>,
) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]"));
    found
}

fn solidity_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn solidity_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    solidity_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn solidity_direct_child_kind<'a>(node: tree_sitter::Node<'a>, kinds: &[&str]) -> Option<&'a str> {
    solidity_direct_child(node, kinds).map(|child| child.kind())
}

fn solidity_direct_child_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(node_text(child, source));
        }
    }
    out
}

fn solidity_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = solidity_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn solidity_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            found = Some(node_text(child, source));
        }
        if let Some(value) = solidity_last_descendant_text(child, source, kinds) {
            found = Some(value);
        }
    }
    found
}
