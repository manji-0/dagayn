use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_go_with_parser(
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
        language: "go".to_string(),
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
            let root = tree.root_node();
            go_walk_children(root, source, file_path, None, &mut nodes, &mut edges);
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn go_walk_children(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_declaration" => {
                go_emit_imports(child, source, file_path, edges);
            }
            "type_declaration" => {
                go_emit_types(child, source, file_path, nodes, edges);
            }
            "function_declaration" | "method_declaration" => {
                if let Some((name, receiver)) = go_function_name_and_receiver(child, source) {
                    go_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        receiver.as_deref(),
                        nodes,
                        edges,
                    );
                    go_walk_children(child, source, file_path, Some(&name), nodes, edges);
                    continue;
                }
            }
            "call_expression" => {
                go_emit_call(child, source, file_path, enclosing_func, edges);
            }
            _ => {}
        }
        go_walk_children(child, source, file_path, enclosing_func, nodes, edges);
    }
}

fn go_emit_imports(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        go_emit_imports(child, source, file_path, edges);
        if child.kind() == "interpreted_string_literal" {
            let target = strip_matching_quotes(node_text(child, source).trim()).to_string();
            if !target.is_empty() {
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::ImportsFrom,
                    source: file_path.to_string(),
                    target,
                    file_path: file_path.to_string(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
            }
        }
    }
}

fn go_emit_types(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "type_spec" {
            continue;
        }
        let Some(name) = go_direct_child_text(child, source, "type_identifier") else {
            continue;
        };
        let qualified = qualify(file_path, &name, None);
        let extra = go_type_extra(child, source);
        nodes.push(ParsedNode {
            kind: crate::core::types::NodeKind::Class,
            name,
            file_path: file_path.to_string(),
            line_start: child.start_position().row as i64 + 1,
            line_end: child.end_position().row as i64 + 1,
            language: "go".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra,
        });
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Contains,
            source: file_path.to_string(),
            target: qualified,
            file_path: file_path.to_string(),
            line: child.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn go_type_extra(node: tree_sitter::Node<'_>, source: &[u8]) -> serde_json::Value {
    let type_role = go_type_role(node, source);
    let mut extra = json!({"type_role": type_role});
    if let Some(map) = extra.as_object_mut() {
        if type_role == "interface" {
            map.insert("is_abstract".to_string(), json!(true));
            map.insert("is_contract".to_string(), json!(true));
        }
        if type_role == "struct" {
            map.insert("container_role".to_string(), json!("data_container"));
            map.insert("value_semantics".to_string(), json!(true));
        }
    }
    extra
}

fn go_type_role(node: tree_sitter::Node<'_>, _source: &[u8]) -> &'static str {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "struct_type" => return "struct",
            "interface_type" => return "interface",
            _ => {}
        }
    }
    "class"
}

fn go_emit_function(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    name: &str,
    receiver: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(file_path, name, receiver);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Function,
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "go".to_string(),
        parent_name: receiver.map(str::to_string),
        params: go_first_parameter_list(node, source),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    let container = receiver
        .map(|receiver| qualify(file_path, receiver, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: container,
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn go_function_name_and_receiver(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, Option<String>)> {
    if node.kind() == "function_declaration" {
        return go_direct_child_text(node, source, "identifier").map(|name| (name, None));
    }
    let name = go_direct_child_text(node, source, "field_identifier")?;
    let receiver = go_receiver_name(node, source);
    Some((name, receiver))
}

fn go_receiver_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let receiver_list = node
        .children(&mut cursor)
        .find(|child| child.kind() == "parameter_list")?;
    go_last_named_descendant(receiver_list, source, &["type_identifier"])
}

fn go_first_parameter_list(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let params = node
        .children(&mut cursor)
        .find(|child| child.kind() == "parameter_list")
        .map(|child| node_text(child, source));
    params
}

fn go_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some((call_name, signature)) = go_call_name_and_signature(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, None))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = go_bridge_edge(node, source, file_path, &caller, &signature) {
        edges.push(edge);
    }
}

fn go_call_name_and_signature(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, String)> {
    let mut cursor = node.walk();
    let callee = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list")?;
    if callee.kind() == "identifier" {
        let name = node_text(callee, source);
        return Some((name.clone(), name));
    }
    if callee.kind() == "selector_expression" {
        let signature = node_text(callee, source);
        let name = go_last_named_descendant(callee, source, &["field_identifier", "identifier"])?;
        return Some((name, signature));
    }
    None
}

fn go_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "exec.Command" => ("invokes_binary", "subprocess"),
        "os.ReadFile" | "os.Open" => ("reads_file", "file_io"),
        "os.WriteFile" => ("writes_file", "file_io"),
        "plugin.Open" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match go_first_string_arg(node, source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "go",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn go_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if matches!(
            child.kind(),
            "interpreted_string_literal" | "raw_string_literal"
        ) {
            return Some(strip_matching_quotes(node_text(child, source).trim()).to_string());
        }
        return None;
    }
    None
}

fn go_direct_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

fn go_last_named_descendant(
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
        if let Some(name) = go_last_named_descendant(child, source, kinds) {
            found = Some(name);
        }
    }
    found
}
