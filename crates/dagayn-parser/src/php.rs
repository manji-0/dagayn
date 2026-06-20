use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{qualify, resolve_rust_call_targets};

pub(super) fn parse_php_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File.as_str().to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: "php".to_string(),
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
            php_walk_children(
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

fn php_walk_children(
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
            "namespace_use_declaration" => {
                php_emit_import(child, source, file_path, edges);
            }
            "class_declaration" | "interface_declaration" => {
                if let Some(name) = php_direct_child_text(child, source, &["name"]) {
                    php_emit_type(child, file_path, &name, enclosing_class, nodes, edges);
                    php_walk_children(child, source, file_path, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_definition" | "method_declaration" => {
                if let Some(name) = php_direct_child_text(child, source, &["name"]) {
                    php_emit_function(
                        child,
                        source,
                        file_path,
                        &name,
                        enclosing_class,
                        nodes,
                        edges,
                    );
                    php_walk_children(
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
            "function_call_expression"
            | "member_call_expression"
            | "nullsafe_member_call_expression"
            | "scoped_call_expression" => {
                php_emit_call(
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
        php_walk_children(
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

fn php_emit_import(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::ImportsFrom
            .as_str()
            .to_string(),
        source: file_path.to_string(),
        target: node_text(node, source).trim().to_string(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn php_emit_type(
    node: tree_sitter::Node<'_>,
    file_path: &str,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let (type_role, is_abstract, is_contract) = if node.kind() == "interface_declaration" {
        ("interface", true, true)
    } else {
        ("class", false, false)
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
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class.as_str().to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "php".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra,
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains.as_str().to_string(),
        source: file_path.to_string(),
        target: qualify(file_path, name, enclosing_class),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn php_emit_function(
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
        kind: crate::core::types::NodeKind::Function.as_str().to_string(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "php".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: php_direct_child_text(node, source, &["formal_parameters"]),
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains.as_str().to_string(),
        source: enclosing_class
            .map(|class| qualify(file_path, class, None))
            .unwrap_or_else(|| file_path.to_string()),
        target: qualified,
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn php_emit_call(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(call_name) = php_call_name(node, source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(file_path, func, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls.as_str().to_string(),
        source: caller.clone(),
        target: call_name.clone(),
        file_path: file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = php_bridge_edge(node, source, file_path, &caller, &call_name) {
        edges.push(edge);
    }
}

fn php_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "function_call_expression" => {
            php_direct_child_text(node, source, &["name", "qualified_name"])
                .map(|name| name.trim_start_matches('\\').to_string())
        }
        "member_call_expression" | "nullsafe_member_call_expression" => {
            php_last_direct_child_text(node, source, "name")
        }
        "scoped_call_expression" => {
            let names = php_direct_child_texts(node, source, &["name"]);
            if names.len() >= 2 {
                return Some(format!("{}::{}", names[0], names[1]));
            }
            if let Some(scope) = php_direct_child_text(node, source, &["relative_scope"]) {
                if matches!(scope.as_str(), "parent" | "self") {
                    return names.last().cloned();
                }
            }
            names.last().cloned()
        }
        _ => None,
    }
}

fn php_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "exec" | "shell_exec" | "system" | "passthru" | "proc_open" | "popen" => {
            ("invokes_binary", "subprocess")
        }
        "file_get_contents" | "fread" | "readfile" => ("reads_file", "file_io"),
        "file_put_contents" | "fwrite" => ("writes_file", "file_io"),
        "fopen" => ("opens_file", "file_io"),
        "FFI::cdef" | "FFI::load" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match php_first_string_arg(node, source) {
        Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
        _ => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact
            .as_str()
            .to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "php",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn php_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        let arg = if child.kind() == "argument" {
            php_first_non_punctuation_child(child).unwrap_or(child)
        } else {
            child
        };
        if matches!(arg.kind(), "encapsed_string" | "string") {
            return Some(php_string_text(arg, source));
        }
        return None;
    }
    None
}

fn php_first_non_punctuation_child(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    let child = node
        .children(&mut cursor)
        .find(|child| !matches!(child.kind(), "," | "(" | ")"));
    child
}

fn php_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return node_text(child, source);
        }
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn php_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
    }
    None
}

fn php_last_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kind: &str,
) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            found = Some(node_text(child, source));
        }
    }
    found
}

fn php_direct_child_texts(
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
