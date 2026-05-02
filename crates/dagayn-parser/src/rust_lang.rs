use std::collections::HashSet;

use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text};
use super::{is_test_function, qualify, resolve_rust_call_targets};

pub(super) fn parse_rust_with_parser(
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
        language: "rust".to_string(),
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
            let mut defined_names = HashSet::new();
            collect_rust_defined_names(root, source, &mut defined_names);
            let context = RustParseContext {
                source,
                file_path,
                defined_names: &defined_names,
            };
            rust_walk_children(root, &context, None, None, &mut nodes, &mut edges);
            let edges = resolve_rust_call_targets(&nodes, edges, file_path);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

fn rust_walk_children(
    node: tree_sitter::Node<'_>,
    context: &RustParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "struct_item" | "enum_item" | "impl_item" => {
                if let Some(name) = rust_type_name(child, context.source) {
                    let qualified = qualify(context.file_path, &name, enclosing_class);
                    nodes.push(ParsedNode {
                        kind: "Class".to_string(),
                        name: name.clone(),
                        file_path: context.file_path.to_string(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "rust".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params: None,
                        return_type: None,
                        modifiers: None,
                        is_test: false,
                        extra: json!({"type_role": rust_type_role(child.kind())}),
                    });
                    edges.push(ParsedEdge {
                        kind: "CONTAINS".to_string(),
                        source: context.file_path.to_string(),
                        target: qualified,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    rust_walk_children(child, context, Some(&name), None, nodes, edges);
                    continue;
                }
            }
            "function_item" => {
                if let Some(name) = rust_identifier_child(child, context.source) {
                    let qualified = qualify(context.file_path, &name, enclosing_class);
                    let params = rust_child_text(child, context.source, "parameters");
                    let is_test = is_test_function(&name, context.file_path, child, context.source);
                    nodes.push(ParsedNode {
                        kind: if is_test { "Test" } else { "Function" }.to_string(),
                        name: name.clone(),
                        file_path: context.file_path.to_string(),
                        line_start: child.start_position().row as i64 + 1,
                        line_end: child.end_position().row as i64 + 1,
                        language: "rust".to_string(),
                        parent_name: enclosing_class.map(str::to_string),
                        params,
                        return_type: None,
                        modifiers: None,
                        is_test,
                        extra: json!({}),
                    });
                    let container = enclosing_class
                        .map(|name| qualify(context.file_path, name, None))
                        .unwrap_or_else(|| context.file_path.to_string());
                    edges.push(ParsedEdge {
                        kind: "CONTAINS".to_string(),
                        source: container,
                        target: qualified,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    rust_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "use_declaration" => {
                if let Some(target) = rust_use_target(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: "IMPORTS_FROM".to_string(),
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
            }
            "call_expression" | "macro_invocation" => {
                if let Some(call_name) = rust_call_name(child, context.source) {
                    let caller = enclosing_func
                        .map(|name| qualify(context.file_path, name, enclosing_class))
                        .unwrap_or_else(|| context.file_path.to_string());
                    edges.push(ParsedEdge {
                        kind: "CALLS".to_string(),
                        source: caller.clone(),
                        target: call_name.clone(),
                        file_path: context.file_path.to_string(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    if let Some(edge) = rust_bridge_edge(
                        child,
                        context.source,
                        context.file_path,
                        &caller,
                        &call_name,
                    ) {
                        edges.push(edge);
                    }
                }
            }
            "arguments" => {
                rust_emit_argument_references(
                    child,
                    context.source,
                    context.file_path,
                    enclosing_class,
                    enclosing_func,
                    context.defined_names,
                    edges,
                );
            }
            _ => {}
        }
        rust_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

struct RustParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    defined_names: &'a HashSet<String>,
}

fn collect_rust_defined_names(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    names: &mut HashSet<String>,
) {
    match node.kind() {
        "struct_item" | "enum_item" | "impl_item" => {
            if let Some(name) = rust_type_name(node, source) {
                names.insert(name);
            }
        }
        "function_item" => {
            if let Some(name) = rust_identifier_child(node, source) {
                names.insert(name);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_defined_names(child, source, names);
    }
}

fn rust_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "impl_item" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "type_identifier" {
                return Some(node_text(child, source));
            }
        }
        return None;
    }
    rust_identifier_child(node, source)
}

fn rust_identifier_child(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "type_identifier" | "field_identifier"
        ) {
            return Some(node_text(child, source));
        }
    }
    None
}

fn rust_child_text(node: tree_sitter::Node<'_>, source: &[u8], kind: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(node_text(child, source));
        }
    }
    None
}

fn rust_type_role(kind: &str) -> &'static str {
    match kind {
        "enum_item" => "enum",
        "impl_item" => "implementation",
        _ => "class",
    }
}

fn rust_use_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    Some(
        text.replace("use ", "")
            .trim_end_matches(';')
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn rust_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "scoped_identifier" => return Some(node_text(child, source)),
            "field_expression" => return rust_rightmost_identifier(child, source),
            _ => {}
        }
    }
    None
}

fn rust_rightmost_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    for child in children.into_iter().rev() {
        if matches!(
            child.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return Some(node_text(child, source));
        }
        if let Some(name) = rust_rightmost_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn rust_emit_argument_references(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    defined_names: &HashSet<String>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|name| qualify(file_path, name, enclosing_class))
        .unwrap_or_else(|| file_path.to_string());
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "identifier" {
            continue;
        }
        let name = node_text(child, source);
        if rust_should_skip_value_reference(&name) || !defined_names.contains(&name) {
            continue;
        }
        edges.push(ParsedEdge {
            kind: "REFERENCES".to_string(),
            source: caller.clone(),
            target: qualify(file_path, &name, None),
            file_path: file_path.to_string(),
            line: child.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
}

fn rust_should_skip_value_reference(name: &str) -> bool {
    matches!(
        name,
        "true"
            | "false"
            | "null"
            | "undefined"
            | "None"
            | "True"
            | "False"
            | "self"
            | "this"
            | "cls"
            | "super"
    ) || name.len() <= 1
        || name.bytes().all(|byte| !byte.is_ascii_lowercase())
}

fn rust_bridge_edge(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    file_path: &str,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let signature = rust_call_signature(node, source).unwrap_or_else(|| call_name.to_string());
    let (relationship_role, bridge_kind) = rust_bridge_pattern(&signature)?;
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match rust_first_string_arg(node, source) {
        Some(target) if !target.is_empty() => (target, 0.8, "HIGH"),
        _ => (
            format!("<dynamic:{signature}@{file_path}:{line}>"),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "rust",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn rust_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let signature = node
        .children(&mut cursor)
        .find(|child| child.kind() != "arguments")
        .map(|child| node_text(child, source).trim().to_string())
        .filter(|value| !value.is_empty());
    signature
}

fn rust_bridge_pattern(signature: &str) -> Option<(&'static str, &'static str)> {
    match signature {
        "std::process::Command::new" | "Command::new" => Some(("invokes_binary", "subprocess")),
        "std::fs::read"
        | "std::fs::read_to_string"
        | "std::fs::File::open"
        | "fs::read"
        | "fs::read_to_string"
        | "File::open" => Some(("reads_file", "file_io")),
        "std::fs::write" | "std::fs::File::create" | "fs::write" | "File::create" => {
            Some(("writes_file", "file_io"))
        }
        "libloading::Library::new" | "Library::new" => Some(("loads_shared_library", "ffi")),
        _ => None,
    }
}

fn rust_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let arguments = node
        .children(&mut cursor)
        .find(|child| child.kind() == "arguments")?;
    let mut arg_cursor = arguments.walk();
    for child in arguments.children(&mut arg_cursor) {
        if matches!(child.kind(), "," | "(" | ")" | "{" | "}" | "[" | "]") {
            continue;
        }
        if matches!(child.kind(), "string_literal" | "raw_string_literal") {
            return Some(decode_rust_string_literal(child, source));
        }
        return None;
    }
    None
}

fn decode_rust_string_literal(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "string_content" | "string_fragment") {
            return node_text(child, source);
        }
    }
    node_text(node, source)
        .trim_matches('"')
        .trim_matches('`')
        .to_string()
}
