use std::collections::HashMap;

use serde_json::json;

use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, set_namespaces_from_type_names};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_elixir_with_parser(
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
        language: "elixir".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = ElixirParseContext {
        source,
        file_path: file_path.clone(),
    };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            elixir_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            set_namespaces_from_type_names(&mut nodes);
            let mut edges = resolve_elixir_call_targets(&nodes, edges, &file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct ElixirParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
}

fn elixir_walk_children(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call"
            && elixir_handle_call(
                child,
                context,
                enclosing_module,
                enclosing_func,
                nodes,
                edges,
            )
        {
            continue;
        }
        elixir_walk_children(
            child,
            context,
            enclosing_module,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn elixir_handle_call(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(ident) = elixir_call_identifier(node, context.source) else {
        return false;
    };
    match ident.as_str() {
        "defmodule" => {
            let Some(arguments) = elixir_direct_child(node, &["arguments"]) else {
                return false;
            };
            let Some(module_name) = elixir_module_name(arguments, context.source) else {
                return false;
            };
            elixir_emit_module(node, context, &module_name, nodes, edges);
            if let Some(do_block) = elixir_direct_child(node, &["do_block"]) {
                elixir_walk_children(do_block, context, Some(&module_name), None, nodes, edges);
            }
            true
        }
        "def" | "defp" | "defmacro" | "defmacrop" => {
            let Some(arguments) = elixir_direct_child(node, &["arguments"]) else {
                return false;
            };
            let Some((function_name, params)) =
                elixir_function_name_and_params(arguments, context.source)
            else {
                return false;
            };
            elixir_emit_function(
                node,
                context,
                &function_name,
                params.as_deref(),
                enclosing_module,
                nodes,
                edges,
            );
            if let Some(do_block) = elixir_direct_child(node, &["do_block"]) {
                elixir_walk_children(
                    do_block,
                    context,
                    enclosing_module,
                    Some(&function_name),
                    nodes,
                    edges,
                );
            }
            true
        }
        "alias" | "import" | "require" | "use" => {
            if let Some(arguments) = elixir_direct_child(node, &["arguments"]) {
                if let Some(module_name) = elixir_module_name(arguments, context.source) {
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom,
                        source: context.file_path.to_string(),
                        target: module_name,
                        file_path: context.file_path.clone(),
                        line: node.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                }
            }
            true
        }
        _ => {
            elixir_emit_call(node, context, enclosing_module, enclosing_func, edges);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if matches!(child.kind(), "arguments" | "do_block") {
                    elixir_walk_children(
                        child,
                        context,
                        enclosing_module,
                        enclosing_func,
                        nodes,
                        edges,
                    );
                }
            }
            true
        }
    }
}

fn elixir_emit_module(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
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
        language: "elixir".to_string(),
        parent_name: None,
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
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn elixir_emit_function(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    name: &str,
    params: Option<&str>,
    enclosing_module: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, enclosing_module);
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
        language: "elixir".to_string(),
        parent_name: enclosing_module.map(str::to_string),
        params: params.map(str::to_string),
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Contains,
        source: enclosing_module
            .map(|module| qualify(&context.file_path, module, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn elixir_emit_call(
    node: tree_sitter::Node<'_>,
    context: &ElixirParseContext<'_>,
    enclosing_module: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(target) = elixir_call_target(node, context.source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, enclosing_module))
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

fn elixir_call_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = elixir_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "dot" => elixir_last_direct_child_text(first, source, "identifier"),
        _ => None,
    }
}

fn elixir_call_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = elixir_first_named_child(node)?;
    match first.kind() {
        "identifier" => Some(node_text(first, source)),
        "dot" => Some(node_text(first, source).replace(' ', "")),
        _ => None,
    }
}

fn elixir_module_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "alias" | "dot") {
            return Some(node_text(child, source).replace(' ', ""));
        }
    }
    None
}

fn elixir_function_name_and_params(
    arguments: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(String, Option<String>)> {
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if child.kind() == "call" {
            let name = elixir_direct_child_text(child, source, &["identifier"])?;
            let mut params_text = node_text(child, source);
            if params_text.starts_with(&name) {
                params_text = params_text[name.len()..].to_string();
            }
            return Some((name, (!params_text.is_empty()).then_some(params_text)));
        }
        if child.kind() == "identifier" {
            return Some((node_text(child, source), None));
        }
    }
    None
}

fn elixir_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn elixir_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    elixir_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn elixir_first_named_child<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|child| child.is_named());
    found
}

fn elixir_last_direct_child_text(
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

fn resolve_elixir_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    let mut module_functions = HashMap::<(String, String), String>::new();
    let mut dotted_functions = HashMap::<String, String>::new();
    let mut bare_functions = HashMap::<String, String>::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
    {
        let qualified = qualify(file_path, &node.name, node.parent_name.as_deref());
        bare_functions
            .entry(node.name.clone())
            .or_insert_with(|| qualified.clone());
        if let Some(module) = &node.parent_name {
            module_functions.insert((module.clone(), node.name.clone()), qualified.clone());
            dotted_functions.insert(format!("{module}.{}", node.name), qualified);
        }
    }

    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS" && !edge.target.contains("::") {
                if let Some(target) = dotted_functions.get(&edge.target) {
                    edge.target = target.clone();
                } else if edge.target.contains('.') {
                    edge.target = edge
                        .target
                        .rsplit('.')
                        .next()
                        .unwrap_or(edge.target.as_str())
                        .to_string();
                } else if let Some(module) = elixir_source_module(&edge.source, file_path) {
                    if let Some(target) =
                        module_functions.get(&(module.to_string(), edge.target.clone()))
                    {
                        edge.target = target.clone();
                    } else if let Some(target) = bare_functions.get(&edge.target) {
                        edge.target = target.clone();
                    }
                } else if let Some(target) = bare_functions.get(&edge.target) {
                    edge.target = target.clone();
                }
            }
            edge
        })
        .collect()
}

fn elixir_source_module<'a>(source: &'a str, file_path: &str) -> Option<&'a str> {
    let suffix = source.strip_prefix(file_path)?.strip_prefix("::")?;
    suffix.rsplit_once('.').map(|(module, _)| module)
}
