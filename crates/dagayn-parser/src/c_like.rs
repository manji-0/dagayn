use std::collections::HashMap;

use serde_json::json;

use super::types::{ParsedEdge, ParsedNode};
use super::util::{is_test_file, line_count, node_text, node_text_bytes, strip_matching_quotes};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_c_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "c", parser)
}

pub(super) fn parse_cpp_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "cpp", parser)
}

pub(super) fn parse_objc_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    parse_c_like_with_parser(file_path, source, "objc", parser)
}

fn parse_c_like_with_parser(
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
    let context = CParseContext {
        source,
        file_path,
        language,
    };

    if let Some(parser) = parser {
        if let Some(tree) = parser.parse(source, None) {
            c_walk_children(
                tree.root_node(),
                &context,
                None,
                None,
                &mut nodes,
                &mut edges,
            );
            let mut edges = resolve_c_call_targets(&nodes, edges, file_path);
            add_tested_by_edges(&nodes, &mut edges);
            return (nodes, edges);
        }
    }

    (nodes, edges)
}

struct CParseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    language: &'a str,
}

fn c_walk_children(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "preproc_include" if enclosing_func.is_none() => {
                if let Some(target) = c_include_target(child, context.source) {
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
            "type_definition" | "struct_specifier" | "class_specifier"
                if enclosing_func.is_none() =>
            {
                if let Some(name) = c_type_name(child, context.source) {
                    c_emit_type(child, context, &name, nodes, edges);
                    c_emit_inheritance(child, context, &name, edges);
                    if context.language == "cpp" {
                        c_walk_children(child, context, Some(&name), enclosing_func, nodes, edges);
                    }
                    continue;
                }
            }
            "class_interface"
            | "class_implementation"
            | "category_interface"
            | "protocol_declaration"
                if context.language == "objc" && enclosing_func.is_none() =>
            {
                if let Some(name) = c_direct_child_text(child, context.source, &["identifier"]) {
                    c_emit_type(child, context, &name, nodes, edges);
                    if child.kind() == "class_implementation" {
                        c_walk_children(child, context, Some(&name), None, nodes, edges);
                    }
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = c_function_name(child, context.source) {
                    c_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    c_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "method_definition" if context.language == "objc" => {
                if let Some(name) = c_direct_child_text(child, context.source, &["identifier"]) {
                    c_emit_function(child, context, &name, enclosing_class, nodes, edges);
                    c_walk_children(child, context, enclosing_class, Some(&name), nodes, edges);
                    continue;
                }
            }
            "call_expression" => {
                c_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            "message_expression" if context.language == "objc" => {
                c_emit_call(child, context, enclosing_class, enclosing_func, edges);
            }
            _ => {}
        }
        c_walk_children(
            child,
            context,
            enclosing_class,
            enclosing_func,
            nodes,
            edges,
        );
    }
}

fn c_emit_type(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
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

fn c_emit_function(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
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
        params: None,
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

fn c_emit_call(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    enclosing_class: Option<&str>,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(context.file_path, func, enclosing_class))
        .unwrap_or_else(|| context.file_path.to_string());
    if let Some(call_name) = c_call_name(node, context.source) {
        edges.push(ParsedEdge {
            kind: "CALLS".to_string(),
            source: caller.clone(),
            target: call_name,
            file_path: context.file_path.to_string(),
            line: node.start_position().row as i64 + 1,
            extra: json!({}),
        });
    }
    if let Some(signature) = c_call_signature(node, context.source) {
        if let Some(edge) = c_bridge_edge(node, context, &caller, &signature) {
            edges.push(edge);
        }
    }
}

fn c_emit_inheritance(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    name: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    if context.language != "cpp" {
        return;
    }
    let Some(base_clause) = c_direct_child(node, &["base_class_clause"]) else {
        return;
    };
    let Some(base) = c_last_descendant_text(base_clause, context.source, &["type_identifier"])
    else {
        return;
    };
    edges.push(ParsedEdge {
        kind: "INHERITS".to_string(),
        source: qualify(context.file_path, name, None),
        target: base,
        file_path: context.file_path.to_string(),
        line: node.start_position().row as i64 + 1,
        extra: json!({
            "relationship_role": "extends",
            "syntax_source": "class_specifier",
        }),
    });
}

fn c_call_signature(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "message_expression" {
        return c_message_selector(node, source);
    }
    let callee = c_call_callee(node)?;
    match callee.kind() {
        "identifier" | "qualified_identifier" => {
            Some(node_text(callee, source).replace(" :: ", "::"))
        }
        "field_expression" => c_last_descendant_text(callee, source, &["field_identifier"]),
        "message_expression" => c_message_selector(callee, source),
        _ => None,
    }
}

fn c_call_callee<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() != "argument_list");
    found
}

fn c_include_target(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text_bytes(node, source);
    if text.starts_with(b"#import") {
        return Some(match std::str::from_utf8(text) {
            Ok(text) => text.trim().to_string(),
            Err(_) => String::from_utf8_lossy(text).trim().to_string(),
        });
    }
    let target = c_direct_child(node, &["system_lib_string", "string_literal"])?;
    Some(
        strip_matching_quotes(
            node_text(target, source)
                .trim()
                .trim_matches(['<', '>'].as_ref()),
        )
        .to_string(),
    )
}

fn c_type_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    c_direct_child_text(node, source, &["type_identifier"])
}

fn c_function_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let declarator = c_first_descendant(node, &["function_declarator"])?;
    c_direct_child_text(declarator, source, &["identifier"])
}

fn c_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "message_expression" {
        return c_message_selector(node, source);
    }
    let callee = c_call_callee(node)?;
    match callee.kind() {
        "identifier" => Some(node_text(callee, source)),
        "field_expression" => c_last_descendant_text(callee, source, &["field_identifier"]),
        "message_expression" => c_message_selector(callee, source),
        _ => None,
    }
}

fn c_message_selector(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut skipped_receiver = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "[" | "]" | ":") {
            continue;
        }
        if !skipped_receiver {
            skipped_receiver = true;
            continue;
        }
        if child.kind() == "identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn c_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &CParseContext<'_>,
    caller: &str,
    signature: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match signature {
        "system" | "popen" | "execvp" | "execv" | "execl" | "posix_spawn" => {
            ("invokes_binary", "subprocess")
        }
        "fopen" | "open" => ("opens_file", "file_io"),
        "fread" => ("reads_file", "file_io"),
        "fwrite" => ("writes_file", "file_io"),
        "dlopen" | "LoadLibrary" => ("loads_shared_library", "ffi"),
        "std::system" | "boost::process::child" => ("invokes_binary", "subprocess"),
        "std::ifstream" | "std::ofstream" | "std::fstream" => ("opens_file", "file_io"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match c_first_string_arg(node, context.source) {
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

fn c_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = c_direct_child(node, &["argument_list"])?;
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return Some(c_string_text(child, source));
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn c_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    strip_matching_quotes(node_text(node, source).trim()).to_string()
}

fn c_direct_child<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    found
}

fn c_direct_child_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    c_direct_child(node, kinds).map(|child| node_text(child, source))
}

fn c_first_descendant<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
        if let Some(found) = c_first_descendant(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn c_collect_descendant_texts(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
    found: &mut Option<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            *found = Some(node_text(child, source));
        }
        c_collect_descendant_texts(child, source, kinds, found);
    }
}

fn c_last_descendant_text(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    kinds: &[&str],
) -> Option<String> {
    let mut found = None;
    c_collect_descendant_texts(node, source, kinds, &mut found);
    found
}

fn resolve_c_call_targets(
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
