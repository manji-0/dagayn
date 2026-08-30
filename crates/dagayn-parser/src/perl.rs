use std::collections::HashMap;

use serde_json::json;

use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, strip_matching_quotes};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_perl_with_parser(
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
        language: "perl".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = PerlParseContext {
        source,
        file_path: file_path.clone(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        perl_walk_children(tree.root_node(), &context, None, &mut nodes, &mut edges);
        let mut edges = resolve_perl_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct PerlParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
}

fn perl_walk_children(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "use_statement" | "require_expression" if enclosing_func.is_none() => {
                edges.push(ParsedEdge {
                    kind: crate::core::types::EdgeKind::ImportsFrom,
                    source: context.file_path.to_string(),
                    target: node_text(child, context.source),
                    file_path: context.file_path.clone(),
                    line: child.start_position().row as i64 + 1,
                    extra: json!({}),
                });
                continue;
            }
            "package_statement" | "class_statement" | "role_statement" => {
                if let Some(name) = perl_package_name(child, context.source) {
                    perl_emit_class(child, context, &name, nodes, edges);
                }
                continue;
            }
            "subroutine_declaration_statement" | "method_declaration_statement" => {
                if let Some(name) = perl_subroutine_name(child, context.source) {
                    perl_emit_function(child, context, &name, nodes, edges);
                    perl_walk_children(child, context, Some(&name), nodes, edges);
                }
                continue;
            }
            "function_call_expression"
            | "ambiguous_function_call_expression"
            | "method_call_expression"
            | "anonymous_function_call_expression" => {
                if let Some(call_name) = perl_call_name(child, context.source) {
                    perl_emit_call(child, context, &call_name, enclosing_func, edges);
                }
            }
            _ => {}
        }
        perl_walk_children(child, context, enclosing_func, nodes, edges);
    }
}

fn perl_emit_class(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "perl".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn perl_emit_function(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, None);
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
        language: "perl".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn perl_emit_call(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    call_name: &str,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, None))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name.to_string(),
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(edge) = perl_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

fn perl_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match call_name {
        "system" | "exec" => ("invokes_binary", "subprocess"),
        "open" => ("opens_file", "file_io"),
        "File::Slurp::read_file" => ("reads_file", "file_io"),
        "File::Slurp::write_file" => ("writes_file", "file_io"),
        "DynaLoader::dl_load_file" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match perl_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{call_name}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": call_name,
            "source_language": "perl",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn perl_package_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.is_named() && child.kind() == "package")
        .map(|child| node_text(child, source))
}

fn perl_subroutine_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    perl_direct_child_text(node, source, &["bareword", "identifier"])
}

fn perl_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "method_call_expression" {
        return perl_direct_child_text(node, source, &["method", "bareword", "identifier"]);
    }
    perl_direct_child_text(node, source, &["function", "bareword", "identifier"])
}

fn perl_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let mut skipped_callee = false;
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "function" | "method") && !skipped_callee {
            skipped_callee = true;
            continue;
        }
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if matches!(
            child.kind(),
            "interpolated_string_literal" | "string_literal" | "quoted_word_list"
        ) {
            return perl_string_text(child, source);
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn perl_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    perl_first_descendant_text(node, source, &["string_content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

fn perl_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn perl_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    perl_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn perl_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = perl_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_perl_call_targets(
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
                .or_insert_with(|| qualify(file_path, &node.name, None));
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
