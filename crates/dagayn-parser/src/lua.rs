use std::collections::HashSet;

use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, node_text_is, strip_matching_quotes};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_lua_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_lua_like_with_parser(file_path, source, "lua", parser)
}

pub(super) fn parse_luau_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_lua_like_with_parser(file_path, source, "luau", parser)
}

fn parse_lua_like_with_parser(
    file_path: &str,
    source: &[u8],
    language: &str,
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: "File".to_string(),
        name: file_path.to_string(),
        file_path: file_path.to_string(),
        line_start: 1,
        line_end,
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = LuaParseContext {
        source,
        file_path,
        language,
    };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            lua_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_lua_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct LuaParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    language: &'a str,
}

fn lua_walk_children(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "variable_declaration" => {
                if lua_handle_variable_declaration(
                    child,
                    context,
                    enclosing_class,
                    enclosing_func,
                    nodes,
                    edges,
                ) {
                    continue;
                }
            }
            "function_declaration" => {
                if let Some((parent, name)) = lua_table_function_name(child, context.source) {
                    lua_emit_function(child, context, &name, Some(&parent), nodes, edges);
                    lua_walk_children(child, context, Some(&parent), Some(&name), nodes, edges);
                    continue;
                }
                if let Some(name) = lua_direct_child_text(child, context.source, &["identifier"]) {
                    lua_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    lua_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "function_call" => {
                if enclosing_func.is_none() {
                    if let Some(target) = lua_require_target(child, context.source) {
                        edges.push(ParsedEdge {
                            kind: "IMPORTS_FROM".to_string(),
                            source: context.file_path.to_string(),
                            target,
                            file_path: context.file_path.to_string(),
                            line: child.start_position().row as i64 + 1,
                            extra: json!({}),
                        });
                        continue;
                    }
                }
                lua_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            "type_definition" if context.language == "luau" => {
                if let Some(name) = lua_direct_child_text(child, context.source, &["identifier"]) {
                    lua_emit_type(child, context, &name, nodes, edges);
                    continue;
                }
            }
            _ => {}
        }
        lua_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn lua_handle_variable_declaration(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let Some(assign) = lua_direct_child(node, &["assignment_statement"]) else {
        return false;
    };
    let Some(var_name) = lua_assignment_variable_name(assign, context.source) else {
        return false;
    };
    let Some(expr_list) = lua_direct_child(assign, &["expression_list"]) else {
        return false;
    };

    let mut cursor = expr_list.walk();
    for expr in expr_list.children(&mut cursor) {
        if expr.kind() == "function_call" {
            if let Some(target) = lua_require_target(expr, context.source) {
                edges.push(ParsedEdge {
                    kind: "IMPORTS_FROM".to_string(),
                    source: context.file_path.to_string(),
                    target,
                    file_path: context.file_path.to_string(),
                    line: node.start_position().row as i64 + 1,
                    extra: json!({}),
                });
                return true;
            }
        }
    }

    let mut cursor = expr_list.walk();
    for expr in expr_list.children(&mut cursor) {
        if expr.kind() == "function_definition" {
            lua_emit_function(node, context, &var_name, enclosing_class, nodes, edges);
            lua_walk_children(
                expr,
                context,
                enclosing_class,
                Some(&var_name),
                nodes,
                edges,
            );
            return true;
        }
    }

    let _ = enclosing_func;
    false
}

fn lua_emit_function(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    name: &str,
    enclosing_class: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, context.file_path, node, context.source);
    let qualified = qualify(context.file_path, name, enclosing_class);
    nodes.push(ParsedNode {
        kind: if is_test { "Test" } else { "Function" }.to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: lua_first_descendant_text(node, context.source, &["parameters"]),
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: enclosing_class
            .map(|class| qualify(context.file_path, class, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn lua_emit_type(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: "Class".to_string(),
        name: name.to_string(),
        file_path: context.file_path.to_string(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: context.language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge {
        kind: "CONTAINS".to_string(),
        source: context.file_path.to_string(),
        target: qualified,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
}

fn lua_emit_call(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(call_name) = lua_call_name(node, context.source) else {
        return;
    };
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: "CALLS".to_string(),
        source: caller.clone(),
        target: call_name,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(signature) = lua_call_signature(node, context.source) {
        if let Some(edge) = lua_bridge_edge(node, context, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn lua_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = lua_call_callee(node)?;
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        "dot_index_expression" | "method_index_expression" => {
            lua_last_direct_child_text(callee, source, "identifier")
        }
        _ => None,
    }
}

fn lua_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = lua_call_callee(node)?;
    let signature = match callee.kind() {
        "identifier" => node_text(callee, source),
        "dot_index_expression" | "method_index_expression" => node_text(callee, source)
            .replace(':', ".")
            .trim()
            .to_string(),
        _ => return None,
    };
    (!signature.is_empty()).then_some(signature)
}

fn lua_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "arguments");
    found
}

fn lua_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "os.execute" | "io.popen" => ("invokes_binary", "subprocess"),
        "io.open" => ("opens_file", "file_io"),
        "io.lines" | "io.read" => ("reads_file", "file_io"),
        "io.write" => ("writes_file", "file_io"),
        "package.loadlib" | "loadlib" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match lua_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{signature}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: "CROSS_ARTIFACT".to_string(),
        source: caller.to_string(),
        target,
        file_path: context.file_path.to_string(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": signature,
            "source_language": context.language,
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn lua_require_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let first = lua_call_callee(node)?;
    if first.kind() != "identifier" || !node_text_is(first, source, "require") {
        return None;
    }
    lua_first_string_arg(node, source)
}

fn lua_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = lua_direct_child(node, &["arguments"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if child.kind() == "string" {
            return Some(lua_string_text(child, source));
        }
        return None;
    }
    None
}

fn lua_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    if let Some(content) = lua_first_descendant_text(node, source, &["string_content"]) {
        return content;
    }
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn lua_table_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<(String, String)> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "dot_index_expression" | "method_index_expression"
        ) {
            let names = lua_direct_child_texts(child, source, &["identifier"]);
            if names.len() >= 2 {
                return Some((names[0].clone(), names[names.len() - 1].clone()));
            }
        }
    }
    None
}

fn lua_assignment_variable_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let variable_list = lua_direct_child(node, &["variable_list"])?;
    lua_first_descendant_text(variable_list, source, &["identifier"])
}

fn lua_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn lua_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    lua_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn lua_direct_child_texts(
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

fn lua_last_direct_child_text(
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

fn lua_first_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(node_text(child, source));
        }
        if let Some(found) = lua_first_descendant_text(child, source, kinds) {
            return Some(found);
        }
    }
    None
}

fn resolve_lua_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &str,
) -> Vec<ParsedEdge> {
    let symbols = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .map(|node| node.name.as_str())
        .collect::<HashSet<_>>();
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind == "CALLS"
                && !edge.target.contains("::")
                && symbols.contains(edge.target.as_str())
            {
                edge.target = qualify(file_path, &edge.target, None);
            }
            edge
        })
        .collect()
}
