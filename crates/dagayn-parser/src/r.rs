use std::collections::HashMap;

use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_r_with_parser(
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
        language: "r".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = RParseContext { source, file_path };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            r_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_r_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct RParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
}

fn r_walk_children(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "binary_operator"
                if r_handle_binary_operator(child, context, enclosing_class, nodes, edges) =>
            {
                continue;
            }
            "call"
                if r_handle_call(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) =>
            {
                continue;
            }
            _ => {}
        }
        r_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn r_handle_binary_operator(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some((left, operator, right)) = r_binary_operator_parts(node) else {
        return false;
    };
    if !matches!(operator.kind(), "<-" | "=") || left.kind() != "identifier" {
        return false;
    }
    let name = node_text(left, context.source);
    if right.kind() == "function_definition" {
        r_emit_function(right, context, &name, enclosing_class, nodes, edges);
        r_walk_children(right, context, enclosing_class, Some(&name), nodes, edges);
        return true;
    }
    if right.kind() == "call" {
        if let Some(call_name) = r_call_name(right, context.source) {
            if matches!(
                call_name.as_str(),
                "setRefClass" | "setClass" | "setGeneric"
            ) {
                r_emit_class_call(right, context, Some(&name), enclosing_class, nodes, edges);
                return true;
            }
        }
    }
    false
}

fn r_handle_call(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(call_name) = r_call_name(node, context.source) else {
        return false;
    };

    if matches!(call_name.as_str(), "library" | "require" | "source") {
        if let Some(target) = r_import_target(node, context.source) {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::ImportsFrom,
                source: context.file_path.to_string(),
                target,
                file_path: context.file_path.to_string(),
                line: node.start_position().row as i64 + 1,
                extra: json!({}),
            });
        }
        return true;
    }

    if matches!(
        call_name.as_str(),
        "setRefClass" | "setClass" | "setGeneric"
    ) {
        r_emit_class_call(node, context, None, enclosing_class, nodes, edges);
        return true;
    }

    r_emit_call(
        node,
        context,
        &call_name,
        enclosing_class,
        enclosing_func,
        edges,
    );
    r_walk_children(node, context, enclosing_class, enclosing_func, nodes, edges);
    true
}

fn r_emit_function(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test {
            crate::core::types::NodeKind::Test
        } else {
            crate::core::types::NodeKind::Function
        },
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "r".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: r_direct_child_text(node, context.source, &["parameters"]),
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn r_emit_class_call(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    assigned_name: Option<&str>,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(class_name) = r_first_string_arg(node, context.source).or_else(|| {
        assigned_name
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    }) else {
        return;
    };
    let qualified = qualify(context.file_path, &class_name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: class_name.clone(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "r".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(methods) = r_find_named_arg(node, context.source, "methods") {
        r_extract_methods(methods, context, &class_name, nodes, edges);
    }
}

fn r_extract_methods(
    list_call: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    class_name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    for (method_name, value) in r_iter_args(list_call, context.source) {
        let Some(method_name) = method_name else {
            continue;
        };
        if value.kind() != "function_definition" {
            continue;
        }
        r_emit_function(value, context, &method_name, Some(class_name), nodes, edges);
        r_walk_children(
            value,
            context,
            Some(class_name),
            Some(&method_name),
            nodes,
            edges,
        );
    }
}

fn r_emit_call(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    call_name: &str,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name.to_string(),
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = r_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

fn r_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &RParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "system" | "system2" => ("invokes_binary", "subprocess"),
        ".Call" | ".External" => ("loads_native_module", "ffi"),
        "dyn.load" | "library.dynam" => ("loads_shared_library", "ffi"),
        "readLines" | "read.csv" | "read.table" => ("reads_file", "file_io"),
        "writeLines" | "write.csv" => ("writes_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match r_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": "r",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn r_binary_operator_parts<'a>(
    node: tree_sitter::Node<'a>,
) -> Option<(
    tree_sitter::Node<'a>,
    tree_sitter::Node<'a>,
    tree_sitter::Node<'a>,
)> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    if children.len() < 3 {
        return None;
    }
    Some((children[0], children[1], children[2]))
}

fn r_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "identifier" | "namespace_operator") {
            return Some(node_text(child, source));
        }
    }
    None
}

fn r_import_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let (_, value) = r_iter_args(node, source).into_iter().next()?;
    match value.kind() {
        "identifier" => Some(node_text(value, source)),
        "string" => r_string_text(value, source),
        _ => None,
    }
}

fn r_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let (_, value) = r_iter_args(node, source).into_iter().next()?;
    if value.kind() == "string" {
        r_string_text(value, source)
    } else {
        None
    }
}

fn r_find_named_arg<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    arg_name: &str,
) -> Option<tree_sitter::Node<'a>> {
    r_iter_args(node, source)
        .into_iter()
        .find_map(|(name, value)| (name.as_deref() == Some(arg_name)).then_some(value))
}

fn r_iter_args<'a>(
    call_node: tree_sitter::Node<'a>,
    source: &[u8],
) -> Vec<(Option<String>, tree_sitter::Node<'a>)> {
    let Some(arguments) = r_direct_child(call_node, &["arguments"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = arguments.walk();
    for argument in arguments.children(&mut cursor) {
        if argument.kind() != "argument" {
            continue;
        }
        let mut name = None;
        let mut value = None;
        let mut seen_equals = false;
        let mut arg_cursor = argument.walk();
        for child in argument.children(&mut arg_cursor) {
            if child.kind() == "=" {
                seen_equals = true;
                continue;
            }
            if !child.is_named() {
                continue;
            }
            if seen_equals {
                value = Some(child);
                break;
            }
            if name.is_none() && child.kind() == "identifier" {
                name = Some(node_text(child, source));
                continue;
            }
            if value.is_none() {
                value = Some(child);
                break;
            }
        }
        if let Some(value) = value.or_else(|| r_first_named_child(argument)) {
            out.push((seen_equals.then_some(name).flatten(), value));
        }
    }
    out
}

fn r_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    r_first_descendant_text(node, source, &["string_content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

fn r_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn r_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    r_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn r_first_named_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|child| child.is_named());
    found
}

fn r_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = r_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_r_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .fold(HashMap::<String, String>::new(), |mut symbols, node| {
            symbols
                .entry(node.name.clone())
                .or_insert_with(|| qualify(file_path, &node.name, node.parent_name.as_deref()));
            symbols
        });
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS" && !edge.target.contains("::") {
                if let Some(target) = symbols.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}
