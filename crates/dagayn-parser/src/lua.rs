use std::collections::HashMap;

use serde_json::json;

use super::types::{FilePath, ParsedEdge, ParsedNode};
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
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode {
        kind: crate::core::types::NodeKind::File,
        name: file_path.to_string(),
        file_path: file_path.clone(),
        line_start: 1,
        line_end,
        language: language.to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: is_test_file(&file_path),
        extra: json!({}),
    }];
    let mut edges = Vec::new();
    let context = LuaParseContext {
        source,
        file_path: file_path.clone(),
        language,
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        lua_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        let mut edges = resolve_lua_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct LuaParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
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
            "variable_declaration"
                if lua_handle_variable_declaration(
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
            "assignment_statement"
                if lua_emit_assigned_functions(
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
                if enclosing_func.is_none()
                    && let Some(target) = lua_require_target(child, context.source)
                {
                    edges.push(ParsedEdge {
                        kind: crate::core::types::EdgeKind::ImportsFrom,
                        source: context.file_path.to_string(),
                        target,
                        file_path: context.file_path.clone(),
                        line: child.start_position().row as i64 + 1,
                        extra: json!({}),
                    });
                    continue;
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
        if expr.kind() == "function_call"
            && let Some(target) = lua_require_target(expr, context.source)
        {
            edges.push(ParsedEdge {
                kind: crate::core::types::EdgeKind::ImportsFrom,
                source: context.file_path.to_string(),
                target,
                file_path: context.file_path.clone(),
                line: node.start_position().row as i64 + 1,
                extra: json!({}),
            });
            return true;
        }
    }

    let _ = var_name;
    lua_emit_assigned_functions(
        assign,
        context,
        enclosing_class,
        enclosing_func,
        nodes,
        edges,
    )
}

/// Emits functions bound by assignment: `f = function`, `M.a.h = function`,
/// and `t = { cb = function ... }`. Other values are walked normally.
/// Returns whether any function was bound.
fn lua_emit_assigned_functions(
    assign: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) -> bool {
    let (Some(variables), Some(values)) = (
        lua_direct_child(assign, &["variable_list"]),
        lua_direct_child(assign, &["expression_list"]),
    ) else {
        return false;
    };
    let mut cursor = variables.walk();
    let targets: Vec<_> = variables.named_children(&mut cursor).collect();
    let mut cursor = values.walk();
    let exprs: Vec<_> = values.named_children(&mut cursor).collect();
    if !exprs
        .iter()
        .any(|expr| matches!(expr.kind(), "function_definition" | "table_constructor"))
    {
        return false;
    }
    let mut bound = false;
    for (index, expr) in exprs.iter().enumerate() {
        let target = targets.get(index).copied();
        let binding = target.and_then(|target| lua_binding_path(target, context.source));
        match (expr.kind(), binding) {
            ("function_definition", Some((parent, name))) => {
                let parent = parent.as_deref().or(enclosing_class);
                lua_emit_function(*expr, context, &name, parent, nodes, edges);
                lua_walk_children(*expr, context, parent, Some(&name), nodes, edges);
                bound = true;
            }
            ("table_constructor", Some((parent, name))) => {
                let table = match parent {
                    Some(parent) => format!("{parent}.{name}"),
                    None => name,
                };
                let mut fields = expr.walk();
                for field in expr.named_children(&mut fields) {
                    let key = field
                        .child_by_field_name("name")
                        .filter(|key| key.kind() == "identifier");
                    let value = field
                        .child_by_field_name("value")
                        .filter(|value| value.kind() == "function_definition");
                    if let (Some(key), Some(value)) = (key, value) {
                        let key = node_text(key, context.source);
                        lua_emit_function(value, context, &key, Some(&table), nodes, edges);
                        lua_walk_children(value, context, Some(&table), Some(&key), nodes, edges);
                        bound = true;
                    } else {
                        lua_walk_children(
                            field,
                            context,
                            enclosing_class,
                            enclosing_func,
                            nodes,
                            edges,
                        );
                    }
                }
            }
            _ => lua_walk_children(
                *expr,
                context,
                enclosing_class,
                enclosing_func,
                nodes,
                edges,
            ),
        }
    }
    bound
}

/// `(parent, name)` for an assignment target: `f` -> (None, f),
/// `M.a.h` -> (Some("M.a"), h).
fn lua_binding_path(
    node: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<(Option<String>, String)> {
    match node.kind() {
        "identifier" => Some((None, node_text(node, source))),
        "dot_index_expression" | "method_index_expression" => {
            let table = node.child_by_field_name("table")?;
            let field = node
                .child_by_field_name("field")
                .or_else(|| node.child_by_field_name("method"))?;
            let parent = node_text(table, source).replace(':', ".");
            Some((Some(parent), node_text(field, source)))
        }
        _ => None,
    }
}

fn lua_emit_function(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
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
        language: context.language.to_string(),
        parent_name: enclosing_class.map(str::to_string),
        params: lua_first_descendant_text(node, context.source, &["parameters"]),
        return_type: None,
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

fn lua_emit_type(
    node: tree_sitter::Node<'_>,
    context: &LuaParseContext<'_>,
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
        language: context.language.to_string(),
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
        .map(|func| qualify(&context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target: call_name,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra: json!({}),
    });
    if let Some(signature) = lua_call_signature(node, context.source)
        && let Some(edge) = lua_bridge_edge(node, context, &caller, &signature)
    {
        edges.push(edge);
    }
}

fn lua_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let callee = lua_call_callee(node)?;
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        "dot_index_expression" | "method_index_expression" => {
            let (parent, name) = lua_binding_path(callee, source)?;
            Some(match parent {
                Some(parent) if parent != "self" => format!("{parent}.{name}"),
                _ => name,
            })
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

    node.children(&mut cursor)
        .find(|child| child.kind() != "arguments")
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
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
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
        if let Some((Some(parent), name)) = lua_binding_path(child, source) {
            return Some((parent, name));
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

    node.children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
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
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    // `M.f` resolves through its table path; a bare name prefers a free
    // function, then a sibling in the caller's table.
    let mut by_path = HashMap::<String, String>::new();
    let mut by_name = HashMap::<&str, Vec<(Option<&str>, String)>>::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
    {
        let qualified = qualify(file_path, &node.name, node.parent_name.as_deref());
        if let Some(parent) = node.parent_name.as_deref() {
            by_path
                .entry(format!("{parent}.{}", node.name))
                .or_insert_with(|| qualified.clone());
        }
        by_name
            .entry(node.name.as_str())
            .or_default()
            .push((node.parent_name.as_deref(), qualified));
    }
    let prefix = format!("{file_path}::");
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind != "CALLS" || edge.target.contains("::") {
                return edge;
            }
            if let Some(target) = by_path.get(&edge.target) {
                edge.target = target.clone();
                return edge;
            }
            // Unresolved `lib.fn` keeps its historical bare-name target.
            let name = match edge.target.rsplit_once('.') {
                Some((_, name)) => name.to_string(),
                None => edge.target.clone(),
            };
            let dotted = edge.target.contains('.');
            match by_name.get(name.as_str()) {
                Some(candidates) if !dotted => {
                    let caller_table = edge
                        .source
                        .strip_prefix(&prefix)
                        .and_then(|rest| rest.rsplit_once('.').map(|(table, _)| table));
                    let chosen = candidates
                        .iter()
                        .find(|(parent, _)| parent.is_none())
                        .or_else(|| {
                            candidates
                                .iter()
                                .find(|(parent, _)| *parent == caller_table)
                        })
                        .unwrap_or(&candidates[0]);
                    edge.target = chosen.1.clone();
                }
                _ => edge.target = name,
            }
            edge
        })
        .collect()
}
