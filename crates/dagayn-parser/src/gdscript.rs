use std::collections::HashMap;

use serde_json::json;

use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_gdscript_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: "gdscript".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = GdscriptParseContext {
        source,
        file_path: file_path.clone(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        gdscript_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        let mut edges = resolve_gdscript_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct GdscriptParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
}

fn gdscript_walk_children(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "extends_statement" if enclosing_func.is_none() => {
                if let Some(target) = gdscript_extends_target(child, context.source) {
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom,
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
                continue;
            }
            "class_name_statement" => {
                if let Some(name) = gdscript_direct_child_text(child, context.source, &["name"]) {
                    gdscript_emit_class(child, context, &name, None, nodes, edges);
                }
                continue;
            }
            "class_definition" => {
                if let Some(name) = gdscript_direct_child_text(child, context.source, &["name"]) {
                    gdscript_emit_class(child, context, &name, enclosing_class, nodes, edges);
                    if let Some(body) = gdscript_direct_child(child, &["class_body"]) {
                        gdscript_walk_children(body, context, Some(&name), None, nodes, edges);
                    }
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = gdscript_direct_child_text(child, context.source, &["name"]) {
                    gdscript_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    if let Some(body) = gdscript_direct_child(child, &["body"]) {
                        gdscript_walk_children(
                            body,
                            context,
                            enclosing_class,
                            Some(&name),
                            nodes,
                            edges,
                        );
                    }
                    continue;
                }
            }
            "call" | "attribute_call" => {
                gdscript_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        gdscript_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn gdscript_emit_class(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "gdscript".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn gdscript_emit_function(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test {
            crate::core::types::NodeKind::Test
        } else {
            crate::core::types::NodeKind::Function
        },
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "gdscript".to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: gdscript_direct_child_text(node, context.source, &["parameters"]),
        return_type: gdscript_direct_child_text(node, context.source, &["type"]),
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_class
            .map(|class| qualify(&context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn gdscript_emit_call(
    node: tree_sitter::Node<'_>,
    context: &GdscriptParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = gdscript_call_name(node, context.source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller,
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn gdscript_extends_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let type_node = gdscript_direct_child(node, &["type"])?;
    gdscript_first_descendant_text(type_node, source, &["identifier"])
        .or_else(|| Some(node_text(type_node, source).trim().to_string()))
        .filter(|target| !target.is_empty())
}

fn gdscript_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    gdscript_direct_child_text(node, source, &["identifier"])
}

fn gdscript_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn gdscript_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    gdscript_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn gdscript_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = gdscript_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_gdscript_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
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
            if edge.kind == "CALLS"
                && !edge.target.contains("::")
                && let Some(target) = symbols.get(&edge.target)
            {
                edge.target = target.clone();
            }
            edge
        })
        .collect()
}
